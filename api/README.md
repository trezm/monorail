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
| `src/constants.rs` | Environment variable names and their defaults. |
| `src/error.rs` | `ApiError` and the JSON error envelope. |
| `src/extract.rs` | Extractors that reject with `ApiError`. |
| `src/state.rs` | `AppState`, one `Arc` around everything shared. |
| `src/telemetry.rs` | Tracing subscriber setup. |
| `src/shutdown.rs` | SIGINT/SIGTERM handling. |
| `src/routes/` | HTTP handlers, one module per resource. |
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

## Deployment

`bazel build --config=release //api` produces a stripped, LTO'd binary at
`bazel-bin/api/api`. There is no container packaging yet; `rules_oci` is the
natural next step and can consume that binary directly.
