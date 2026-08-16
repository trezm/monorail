# monorail

A Bazel monorepo. Currently one service: [`api/`](api) — a Rust/axum HTTP API.

## Prerequisites

Bazel is fetched automatically at the version pinned in `.bazelversion`; you
only need the launcher:

```bash
brew install bazelisk
```

Nothing else is required. A local Rust toolchain is optional — Bazel downloads
its own, pinned in `MODULE.bazel`.

## Everyday commands

```bash
bazel build //...          # build everything
bazel test //...           # run every test
bazel run //api            # run the API on :8080
bazel build //:clippy      # lint
bazel test //:rustfmt      # check formatting
```

Run a single test target with output:

```bash
bazel test //api:integration_tests --config=verbose
```

Optimized build:

```bash
bazel build --config=release //api
```

## Worktrees and disk usage

Each git worktree is a separate directory, so Bazel treats each as its own
workspace with its own output base — the same duplication problem as one
`target/` per worktree. Three things in `.bazelrc` prevent that:

| Cache | Location | Holds |
|---|---|---|
| Disk cache | `~/.cache/bazel/monorail/disk` | Compiled rlibs, binaries, test logs |
| Repository cache | `~/.cache/bazel/monorail/repo` | Rust toolchains, crate tarballs |
| Action env | `--incompatible_strict_action_env` | Keeps cache keys identical across worktrees |

All three live outside every worktree, so the second worktree to build a target
gets a cache hit rather than a rebuild. Measured on this repo: a cold build took
441s; the same build in a second workspace took 24s with **327/327 compile
actions served from the disk cache**.

What is still per-worktree is the output base — the materialized `bazel-out`
tree, a few hundred MB. Bazel never reclaims one when its worktree is deleted,
so:

```bash
tools/cache.sh           # report worktree and cache sizes
tools/cache.sh --prune   # delete output bases whose workspace is gone
```

Pruning is driven by each output base's `DO_NOT_BUILD_HERE` marker and only
removes bases whose workspace directory no longer exists.

The disk cache is capped at 25G in `.bazelrc` and garbage-collects itself once
the Bazel server has been idle for five minutes.

Machine-specific settings (a bigger cache, a remote cache, `--jobs`) go in
`.bazelrc.user`, which is gitignored.

## IDE setup

rust-analyzer can read the build graph straight from Bazel, which avoids a
`target/` directory entirely. Two ways to wire it up.

### Emacs (eglot)

Preferred: rust-analyzer asks Bazel for the crate graph on demand via
`workspace.discoverConfig`, calling [`scripts/discover-rust-project.sh`](scripts/discover-rust-project.sh).
Nothing is written to disk, so there is no `rust-project.json` to regenerate or
let go stale.

```elisp
(defun monorail/rust-analyzer-init-options (server)
  "Enable Bazel project discovery for SERVER when the repo supports it."
  (let* ((root (project-root (eglot--project server)))
         (script (expand-file-name "scripts/discover-rust-project.sh" root)))
    (when (and (file-exists-p (expand-file-name "MODULE.bazel" root))
               (file-executable-p script))
      `(:workspace
        (:discoverConfig
         (:command [,script "{arg}"]
                   :progressLabel "bazel"
                   :filesToWatch ["BUILD.bazel" "MODULE.bazel"]))))))

(add-to-list 'eglot-server-programs
             '(rust-mode . ("rust-analyzer"
                            :initializationOptions
                            monorail/rust-analyzer-init-options)))
(add-hook 'rust-mode-hook 'eglot-ensure)
```

This has to go in `initializationOptions`: rust-analyzer reads
`workspace.discoverConfig` only at initialize and silently ignores it if it
arrives later via `didChangeConfiguration`, so `.dir-locals.el` and
`eglot-workspace-configuration` do not work for this setting.

The guard means plain Cargo checkouts get `nil` — which serialises to `{}` — so
their behaviour is unchanged and the same config is safe everywhere.

### Everything else

Generate the file once and point the editor at it:

```bash
bazel run @rules_rust//tools/rust_analyzer:gen_rust_project
```

In VS Code, `"rust-analyzer.linkedProjects": ["rust-project.json"]`. Re-run
after adding a crate or changing dependencies — unlike the discover path, this
file does go stale.

## Cargo

`Cargo.toml` is still the source of truth for third-party dependencies —
`crate_universe` reads it — so plain `cargo` works as an escape hatch:

```bash
cargo test --workspace
```

Give it a shared target directory, or every worktree grows its own multi-GB
`target/`:

```bash
export CARGO_TARGET_DIR="$HOME/.cache/monorail/cargo-target"
```

A committed [`.envrc`](.envrc) sets this automatically if you use
[direnv](https://direnv.net). The trade-off is that concurrent `cargo`
invocations across worktrees serialize on cargo's lock — correct, just not
parallel.

## Adding or changing dependencies

Edit `[workspace.dependencies]` in the root `Cargo.toml` and reference it from
the member crate, then re-pin:

```bash
cargo update -p <crate>            # refresh Cargo.lock
CARGO_BAZEL_REPIN=1 bazel mod deps # refresh the Bazel resolution
```

## Adding a crate

1. Create `<name>/Cargo.toml` and add `<name>` to `members` in the root `Cargo.toml`.
2. Add `<name>/BUILD.bazel` — copy [`api/BUILD.bazel`](api/BUILD.bazel) as a starting point.
3. `CARGO_BAZEL_REPIN=1 bazel mod deps`

Note that `rules_rust` exposes the Bazel *target* name as `CARGO_PKG_NAME`, so
keep the target name equal to the Cargo package name and set `version`
explicitly — otherwise `env!("CARGO_PKG_VERSION")` silently becomes `"0.0.0"`
under Bazel while staying correct under Cargo.
