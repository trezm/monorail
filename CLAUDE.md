# CLAUDE.md

Guidance for coding agents working in this repo. Read `README.md` for the long
form; this file is the short one.

## Writing rules

**Be terse.** Documentation, commit messages, PR descriptions and review
comments: state the thing and stop. No preamble, no summary of what you just
said, no restating the diff in prose. A review comment is a claim and its
consequence, not an essay.

**No inline comments.** Do not annotate code line by line. Rust doc comments
(`//!` on a module, `///` on an item) are the documentation surface — use them
to say *why* a thing exists and what a caller must know. A non-doc `//` comment
is justified only when the code is genuinely surprising (a load-bearing
ordering, a workaround for upstream behaviour) and the reason cannot live in a
doc comment. Existing comments stay; this applies to new code.

## Layout

```
Cargo.toml           Workspace root. Single source of truth for dependencies.
MODULE.bazel         bzlmod: rules_rust, toolchain, crate_universe.
.bazelrc             Cross-worktree caches, --config=release, --config=verbose.
clippy.toml          Repo-wide lint policy (disallowed types).
BUILD.bazel          //:clippy and //:rustfmt over every Rust target.
api/                 The one service so far: Rust/axum HTTP API.
scripts/             rust-analyzer Bazel project discovery.
tools/cache.sh       Report/prune Bazel output bases across worktrees.
```

`api/src` splits as: `main.rs` (process wiring only) → `lib.rs` (router +
middleware stack) → `routes/` (one module per resource, each exporting
`router()`) → `services/` (capabilities behind traits, HTTP-agnostic) →
`dao/` (models per table, the only layer that names a table or column) with
`config.rs`, `constants.rs`, `error.rs`, `extract.rs`, `state.rs`,
`telemetry.rs`, `shutdown.rs` as support and `testing.rs` (`cfg(test)` only) for
fixtures. See `api/README.md` for per-file responsibilities.

## Running it

Bazel is the build system; Cargo is an escape hatch that must keep working.

```bash
bazel build //...                  # build everything
bazel test //...                   # every test
bazel run //api                    # serve on :8080
bazel build //:clippy              # lint
bazel test //:rustfmt              # format check
bazel run @rules_rust//tools/rustfmt  # format in place
```

Before calling work done: `bazel test //...`, `bazel build //:clippy`,
`bazel test //:rustfmt`.

Single target with output: `bazel test //api:integration_tests --config=verbose`.
Optimized binary: `bazel build --config=release //api` → `bazel-bin/api/api`.

Dependency changes go in `[workspace.dependencies]` in the root `Cargo.toml`,
then:

```bash
cargo update -p <crate>
CARGO_BAZEL_REPIN=1 bazel mod deps
```

A new crate needs a `BUILD.bazel` (copy `api/BUILD.bazel`), a `members` entry,
and a repin. Keep the Bazel target name equal to the Cargo package name —
`rules_rust` derives `CARGO_PKG_NAME` from it — and set `version` explicitly.

## Conventions

- Handlers return `ApiResult<T>`; every failure serializes to the
  `{"error":{"code","message"}}` envelope. Clients branch on `code`.
- Use `crate::extract::{Json, Path, Query}`, never axum's. `clippy.toml` makes
  the mistake a build failure.
- Services expose traits (see `services::container`) so handlers depend on
  behaviour, not on a backend. Service errors are independent of `ApiError`;
  one `From` impl decides how each case surfaces.
- Every service and DAO trait carries `#[cfg_attr(test, mockall::automock)]`,
  above `#[async_trait]` — mockall requires that order. Add it with the trait,
  not later.
- A DAO trait method takes its own pooled connection, so it is the transaction
  boundary. A write spanning tables is therefore one method, not two composed
  by the service above — see `SessionDao::open_login`.
- Collaborators reach a handler through `AppState`, never as a concrete type,
  so a test can swap any of them for a mock.
- Middleware order in `lib.rs` is load-bearing. Changing it changes what gets
  logged and redacted.
- Config is `API_`-prefixed and every setting has a default; add new ones to
  `constants.rs`, `config.rs`, and `api/.env.example` together.
- axum 0.8 path syntax is `/{id}`.
- Tests live in `#[cfg(test)]` modules beside the code. For anything touching
  HTTP that means a route test: build the whole app with `testing::app`, drive
  one request, and mock the services under it. No database, and the assertion
  is on what the handler asked for. `api/tests/api.rs` keeps only what proves
  the crate assembles from outside its own boundary — not behaviour.
- `testing::state()` leaves every collaborator a mock with no expectations, so
  reaching an unarranged one fails rather than passes quietly.
- A test that must exercise real SQL — the only way `schema.rs` drift is
  caught — is `#[ignore]`d, so `bazel test //...` needs no Postgres.
- `unsafe_code` is forbidden; clippy runs at `pedantic`.
