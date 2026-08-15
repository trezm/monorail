# api

HTTP API service built on [axum](https://github.com/tokio-rs/axum).

```bash
bazel run //api                                  # http://localhost:8080
curl localhost:8080/health/live
```

## Layout

| File | Responsibility |
|---|---|
| `src/main.rs` | Process entrypoint. Thin on purpose. |
| `src/lib.rs` | Router assembly and the middleware stack. |
| `src/config.rs` | Environment parsing. Every setting has a default. |
| `src/error.rs` | `ApiError` and the JSON error envelope. |
| `src/extract.rs` | Extractors that reject with `ApiError`. |
| `src/state.rs` | `AppState`, one `Arc` around everything shared. |
| `src/telemetry.rs` | Tracing subscriber setup. |
| `src/shutdown.rs` | SIGINT/SIGTERM handling. |
| `src/widget.rs` | Sample domain type and in-memory store. |
| `src/routes/` | HTTP handlers, one module per resource. |
| `tests/api.rs` | End-to-end tests against the real router. |

The application lives in a library, not the binary, so `tests/api.rs` builds the
real `Router` and drives it in-process with `tower::ServiceExt::oneshot` — no
port binding, no flakiness, full middleware coverage.

## Endpoints

| Method | Path | Notes |
|---|---|---|
| `GET` | `/` | Service name, version, environment |
| `GET` | `/health/live` | Liveness. Never checks dependencies. |
| `GET` | `/health/ready` | Readiness. Add dependency checks here. |
| `GET` | `/api/v1/widgets` | `?limit=` (1–100, default 20) `&offset=` |
| `POST` | `/api/v1/widgets` | `201` + `Location` |
| `GET` | `/api/v1/widgets/{id}` | |
| `PATCH` | `/api/v1/widgets/{id}` | Absent field unchanged; explicit `null` clears |
| `DELETE` | `/api/v1/widgets/{id}` | `204` |

Liveness and readiness are deliberately different: a dependency outage should
take the instance out of the load balancer, not get the container killed.

## Conventions

**Errors.** Handlers return `ApiResult<T>`. Every failure — including extractor
rejections — serializes to one envelope:

```json
{ "error": { "code": "not_found", "message": "widget `…` does not exist" } }
```

Branch on `code`, not `message`. Anything converted from `anyhow::Error` logs
its full cause chain and returns an opaque `500`, so internal detail never
reaches a client.

**Extractors.** Use `crate::extract::{Json, Path, Query}` rather than axum's.
They are the same extractors with `ApiError` as the rejection type, which is
what keeps malformed input on the envelope above.

**Middleware.** Order is load-bearing and documented in `src/lib.rs`. The
request id is assigned before the tracing span opens, so every log line for a
request carries `request_id`, and it comes back as the `x-request-id` header.

**Path parameters.** axum 0.8 uses `/{id}`; the 0.7 `/:id` form no longer works.

## Configuration

Every variable is `API_`-prefixed and optional — see [`.env.example`](.env.example)
for the full list with defaults. `PORT` is also honored unprefixed, since that
is what most container platforms inject.

The binary loads `.env` if present; tests never do.

## Replacing the sample resource

`widget.rs` exists to show the shape end to end. When you swap it for a real
domain:

- `WidgetStore`'s methods already return `ApiResult`, so moving from the
  in-memory `HashMap` to a database does not change any caller.
- It is deliberately not behind a trait. Add one when a second implementation
  justifies it.
- The lock is `std::sync::RwLock`, not `tokio`'s, because no guard is held
  across an `.await`. Keep that true or switch the lock.

Adding a database means adding the driver to `[workspace.dependencies]`, a pool
to `AppState`, a real check in `/health/ready`, and a re-pin
(`CARGO_BAZEL_REPIN=1 bazel mod deps`).

## Deployment

`bazel build --config=release //api` produces a stripped, LTO'd binary at
`bazel-bin/api/api`. There is no container packaging yet; `rules_oci` is the
natural next step and can consume that binary directly.
