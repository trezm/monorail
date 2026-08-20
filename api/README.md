# api

HTTP API service built on [axum](https://github.com/tokio-rs/axum).

```bash
tools/stack.sh up                                # Postgres, the UI, then the API
curl localhost:8080/health/ready
```

The service requires Postgres and refuses to start without it. See
[Database](#database) below, and `tools/stack.sh --help` for the stack.

## Layout

| File | Responsibility |
|---|---|
| `src/main.rs` | Process entrypoint. Thin on purpose. |
| `src/lib.rs` | Router assembly and the middleware stack. |
| `src/config.rs` | Environment parsing. Every setting has a default. |
| `src/constants.rs` | Environment variable names and their defaults. |
| `src/db.rs` | The Postgres pool, and the embedded migrations. |
| `src/schema.rs` | The tables, as diesel sees them. Hand-written. |
| `src/error.rs` | `ApiError` and the JSON error envelope. |
| `src/extract.rs` | Extractors that reject with `ApiError`. |
| `src/state.rs` | `AppState`, one `Arc` around everything shared. |
| `src/telemetry.rs` | Tracing subscriber setup. |
| `src/shutdown.rs` | SIGINT/SIGTERM handling. |
| `src/bin/migrate.rs` | Applies pending migrations and exits. |
| `src/secret.rs` | A wrapper that keeps a string out of logs. |
| `src/autoscaler.rs` | The horizontal autoscaling loop. |
| `src/routes/` | HTTP handlers, one module per resource. |
| `src/services/` | Business logic, one trait per capability. |
| `migrations/` | Schema history, embedded into the binary. |
| `diesel.toml` | Read by diesel-cli only. |
| `tests/api.rs` | End-to-end tests against the real router. |

The application lives in a library, not the binary, so `tests/api.rs` builds the
real `Router` and drives it in-process with `tower::ServiceExt::oneshot` — no
port binding, no flakiness, full middleware coverage.

## Conventions

**Errors.** Handlers return `ApiResult<T>`. Every failure — including extractor
rejections — serializes to one envelope:

```json
{ "error": { "code": "not_found", "message": "no route for GET /nope" } }
```

Branch on `code`, not `message`. Anything converted from `anyhow::Error` logs
its full cause chain and returns an opaque `500`, so internal detail never
reaches a client.

**Extractors.** Use `crate::extract::{Json, Path, Query}` rather than axum's.
They are the same extractors with `ApiError` as the rejection type, which is
what keeps malformed input on the envelope above. axum's own extractors reject
with `text/plain`, so reaching for one silently drops that endpoint off the
envelope — `//:clippy.toml` disallows them to make that a build failure instead.
`src/extract.rs` carries the one `allow`, since wrapping them is its job.

**Middleware.** Order is load-bearing and documented in `src/lib.rs`. The
request id is assigned before the tracing span opens, so every log line for a
request carries `request_id`, and it comes back as the `x-request-id` header.

**Path parameters.** axum 0.8 uses `/{id}`; the 0.7 `/:id` form no longer works.

## Database

Postgres, through [diesel-async](https://github.com/weiznich/diesel_async) over
a [bb8](https://github.com/djc/bb8) pool. Only diesel's `postgres_backend`
feature is enabled, never `postgres` — that one links libpq. The wire protocol
is `tokio-postgres`, so the build is pure Rust and needs no system packages,
under Bazel or in a container.

`AppState` owns one `Database`. Handlers take a connection per query and give it
straight back:

```rust
async fn handler(State(state): State<AppState>) -> ApiResult<Json<Thing>> {
    let mut conn = state.db().conn().await?;
    let thing = things::table.find(id).first(&mut conn).await?;
    Ok(Json(thing))
}
```

Do not hold a connection across an `.await` that does something else — an
upstream call, a lock. A pool of ten deadlocks on the eleventh concurrent
request that does.

`DbError` splits the two cases a caller cares about: `Unavailable` (the query
never reached Postgres — `503`, retry) and `Query` (it did, and failed —
`500`). `diesel::result::Error::NotFound` deliberately does *not* become a
`404`: whether an empty result means "no such resource" or a broken invariant
depends on the query, so match it where you know and return
`ApiError::not_found`.

`/health/ready` round-trips `SELECT 1`. A pool that has never dialled out
reports itself perfectly healthy, so pool statistics are not a readiness check.

### Migrations

Plain SQL under `migrations/`, embedded into the binary at compile time by
`embed_migrations!` — a deployed artifact carries its own schema history and
needs no diesel-cli beside it. Under Bazel that embedding depends on
`compile_data` and a `CARGO_MANIFEST_DIR` in `rustc_env`; `BUILD.bazel` explains
why, and `db::tests::migrations_are_embedded` fails if either is dropped.

Adding one only needs the files, not a database or a toolchain:

```bash
mkdir -p api/migrations/$(date -u +%Y%m%d%H%M%S)_add_things
touch api/migrations/*_add_things/{up,down}.sql
```

diesel-cli generates the same layout (`cd api && diesel migration generate
add_things`) if you have it; it is not required, and installing it pulls in
libpq.

Applying them is a separate binary, [`src/bin/migrate.rs`](src/bin/migrate.rs):

```bash
bazel run //api:migrate
```

It reads the same `API_DATABASE_URL` and the same embedded migrations as the
server, so there is no second source of truth. The server deliberately does not
migrate on startup — in a rolling deploy every replica would race to run them,
so this wants to be one job per deploy.

## Authentication

Users log in through Railway, which acts as an OAuth 2.0 and OpenID Connect
provider. [`src/services/auth.rs`](src/services/auth.rs) implements the flow:
`AuthProvider` is the trait that describes the capability, and `RailwayAuth`
is the implementation.

**The flow.** The service uses the authorization code flow with PKCE. It sends
the `S256` challenge method, because that is the only method Railway supports.
It builds every Railway endpoint from the issuer URL in the configuration, so a
test can point the issuer at a local server and run the whole flow without
reaching the network.

**ID token signatures.** Every ID token is verified with `jsonwebtoken`
against the keys the provider publishes at `{issuer}/oauth/jwks`: signature,
issuer, audience and expiry. Railway signs with ES256, and the algorithm is
pinned rather than read from the token's own header, which is how `alg`
confusion is avoided. [`src/services/jwks.rs`](src/services/jwks.rs) caches the
keys by `kid` and refetches once on an unknown one, so a key rotation does not
need a restart.

OpenID Connect Core §3.1.3.7 would let the check be skipped, because the token
set never passes through the browser — the service fetches it over TLS in a
direct call to the token endpoint and authenticates itself in that call. It is
done anyway: it costs one cached key fetch, and it does not rest on that
argument staying true. Identity itself still comes from the userinfo endpoint.

**Configuration.** The client ID, client secret and redirect URI are
required. If any of them is missing, the server fails at startup and names the
one it wanted. Nothing here works without a login, so there is no mode in which
the server runs with authentication disabled. `OAuthConfig::from_env` is read
by the server alone — `//api:migrate` needs a database and nothing else, so a
migration job is never handed credentials it would not send.

**Secrets in logs.** The code wraps tokens and the `state` value in
[`Secret`](src/secret.rs), or gives them a `Debug` that redacts them. A
`?config` field, a panic message or an error report therefore cannot print one.

### The flow

| Route | |
|---|---|
| `GET /auth/railway` | mints `state` and a PKCE pair, sets the pending cookie, redirects to Railway |
| `GET /auth/railway/callback` | checks `state`, exchanges the code, reads the identity, opens a session |
| `DELETE /auth/session` | deletes the session row and clears the cookie |
| `GET /api/v1/users/me` | the logged-in user, or `401` |

The session is the resource the first three act on. The callback keeps a path of
its own because it is the redirect URI registered on the OAuth app, which has to
match byte for byte.

The first three are outside `/api/v1` because they are browser redirects, not a
versioned API — the same reason `health` is. The profile is the one the UI
calls, so it is versioned.

Two cookies. The pending cookie carries `state` and the PKCE verifier for the
ten minutes between the redirect out and the callback back; comparing the
`state` query parameter against it is the standard double-submit check.
`SameSite=Lax` is required rather than preferred — the callback is a top-level
cross-site GET and `Strict` would withhold the cookie exactly when it is needed.
Neither uses the `__Host-` prefix, which mandates `Secure` and so cannot work
over the `http://localhost` a local checkout runs on; `Secure` is set everywhere
except development instead.

A failed login answers on the error envelope rather than redirecting somewhere
friendlier, so a user who declines consent sees JSON. Swapping that for a
redirect carrying an error code is the obvious follow-up.

### Sessions

[`src/services/session.rs`](src/services/session.rs) owns them.
`SessionStore` is the capability, `PgSessionStore` the Postgres implementation,
and the trait is what lets the route tests run with a `HashMap` and no database.

The cookie holds an opaque token; the `sessions` row holds only its SHA-256
digest, so a dump of that table yields no usable session. The digest is
unsalted and unstretched on purpose: the input is 256 bits of uniform
randomness, so there is no dictionary to defend against and a slow hash would
tax every authenticated request for nothing. Expiry is checked against the row,
never the cookie's `Max-Age`, which a client controls.

The Railway access and refresh tokens live in that row, because the point of
logging in with Railway is to act on Railway afterwards and a one-hour token has
to outlive the request that fetched it. They are stored in plaintext columns:
that table is as sensitive as the database, and encrypting the columns is the
obvious next step. Expired rows are not swept yet either.

`CurrentSession` in [`src/extract.rs`](src/extract.rs) is how an endpoint
requires a login — it costs one query, and rejects with `401` whether the cookie
is absent, unknown or expired. `CurrentUser` is the same extractor narrowed to
the account; take it unless the handler needs the Railway tokens too.

## Railway

Logging in with Railway is only the means; acting on Railway afterwards is the
point. [`src/services/railway.rs`](src/services/railway.rs) is that half:
`RailwayApi` is the capability, `RailwayGraphQl` talks to
`{issuer}/graphql/v2`, and the same trait-plus-stub arrangement keeps the route
tests off the network.

`GET /api/v1/projects` returns the caller's projects with their services nested
inside, because one GraphQL query returns exactly that and asking twice would be
two round trips for a shape the server already assembles.

`GET /api/v1/projects/{id}/environments` lists a project's durable
environments, and
`GET /api/v1/services/{id}/instance?environment={id}` returns how that service
is configured and deployed in one of them — `404` when it has no instance
there. These are separate requests rather than more nesting because they are
read on demand, and an instance exists per service *and* environment: fetching
every combination up front is the oversized query this surface has answered
with a `503` before.

`POST /api/v1/services/{id}/spin-down?environment={id}` removes the service's
latest deployment there — a spin-down rather than a delete, because the service
and its configuration survive. `404` when there is no instance, `422` when
nothing is running, `204` on success: removal leaves nothing to describe.

`POST /api/v1/services/{id}/spin-up?environment={id}` is the inverse: it
redeploys what a spin-down removed, answering `201` with the fresh deployment.
`422` when the service is not spun down; the other answers match.

**Token renewal.** A session lasts two weeks and the access token it was opened
with lasts about an hour, so something has to renew the second without ending
the first. `Credentials` in
[`src/services/session.rs`](src/services/session.rs) is that something:
`state.credentials().access_token(..)` hands back a token that is good now,
refreshing and writing back through `SessionStore::renew` when it is not. It
composes the store and the auth provider rather than living on either — neither
knows about the other, and it would be the wrong dependency in both directions.

A provider that returns no new refresh token has not revoked the old one, so the
old one is kept; the alternative ends the session at the next expiry. A login
that never got a refresh token, or one the provider has stopped honouring, is
`CredentialError::Spent` and reaches the client as a `401`, which sends the
browser back through a login rather than a `400` it can do nothing with.

Renewal is per-request and unsynchronised: two requests arriving on the same
expired session both refresh, and the second write wins. Both tokens work, so
the cost is a wasted call rather than a broken session.

## Autoscaling

`POST /api/v1/services/{id}/autoscaling` creates a horizontal autoscaling rule
for a service: a metric (`CPU`, `MEMORY`, `NETWORK_RX`, `NETWORK_TX`), a
min/max threshold band in the metric's unit (vCPU cores, or gigabytes), a poll
frequency, and the environment to scale in. One rule per service and metric;
`GET` lists the caller's, `DELETE .../{rule_id}` removes one.

[`src/services/autoscaling.rs`](src/services/autoscaling.rs) is the store —
`AutoscaleStore` the capability, `PgAutoscaleStore` the implementation — and
[`src/autoscaler.rs`](src/autoscaler.rs) is the loop that acts on it. Each
tick (`API_AUTOSCALER_TICK_SECS`) it takes the rules whose poll frequency has
elapsed, averages the metric over the last five minutes via Railway's
`metrics` query, and moves the replica count by one when the average leaves
the band — never below one replica. Replica changes go out as
`serviceInstanceUpdate` plus `serviceInstanceDeployV2`, because Railway stages
instance updates until a deploy applies them.

The loop authenticates as the rule's owner, reading their freshest live
session and renewing its access token through the same refresh flow requests
use — written back by session row, since the loop holds no cookie. An owner
with no live session has rules that wait, not rules that break.

The loop is its own module with its dependencies passed in as handles, so it
can detach into its own service; `API_AUTOSCALER_ENABLED=false` is that
future's flag and today's off switch. Known simplifications, noted in the
module: every sweep hits Postgres where a pass-through KV cache would do, and
the aggregate is a plain mean where a trimmed mean or percentile would resist
outliers better.

## Configuration

Every variable is `API_`-prefixed and optional — see [`.env.example`](.env.example)
for the full list with defaults. `PORT` is also honored unprefixed, since that
is what most container platforms inject.

The binaries load `api/.env` if present, whatever directory they were started
from — `cargo run` from the workspace root and `bazel run //api`, whose working
directory is its runfiles tree, both read this crate's file and not one at the
workspace root. Real environment wins over it, and tests never load it at all.

## Deployment

`bazel build --config=release //api` produces a stripped, LTO'd binary at
`bazel-bin/api/api`. There is no container packaging yet; `rules_oci` is the
natural next step and can consume that binary directly. The local stack does not
containerize the API either, so nothing in the repo currently builds an image
for it.
