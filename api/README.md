# api

HTTP API service built on [axum](https://github.com/tokio-rs/axum).

```bash
tools/stack.sh up                                # Postgres, then the API
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
| `src/error.rs` | `ApiError` and the JSON error envelope. |
| `src/extract.rs` | Extractors that reject with `ApiError`. |
| `src/state.rs` | `AppState`, one `Arc` around everything shared. |
| `src/telemetry.rs` | Tracing subscriber setup. |
| `src/shutdown.rs` | SIGINT/SIGTERM handling. |
| `src/bin/migrate.rs` | Applies pending migrations and exits. |
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

## Configuration

Every variable is `API_`-prefixed and optional — see [`.env.example`](.env.example)
for the full list with defaults. `PORT` is also honored unprefixed, since that
is what most container platforms inject.

The binary loads `.env` if present; tests never do.

## Deployment

`bazel build --config=release //api` produces a stripped, LTO'd binary at
`bazel-bin/api/api`. There is no container packaging yet; `rules_oci` is the
natural next step and can consume that binary directly. The local stack does not
containerize the API either, so nothing in the repo currently builds an image
for it.
