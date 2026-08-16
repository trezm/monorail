#!/usr/bin/env bash
#
# Invoked by rust-analyzer, not by you.
#
# rust-analyzer's `workspace.discoverConfig` calls this whenever it needs the
# crate graph: on opening a Rust file, and again whenever a watched BUILD file
# changes. It hands off to the rules_rust discover tool, which runs the
# rust_analyzer aspect over the relevant targets and streams the project back as
# JSONL on stdout:
#
#   {"kind":"progress","message":"..."}      zero or more
#   {"kind":"finished","buildfile":"...","project":{...}}
#   {"kind":"error","error":"...","source":"..."}
#
# Nothing is written to disk, so there is no rust-project.json to regenerate or
# go stale.
#
# stdout MUST be pure JSONL. Bazel writes progress to stderr, so `bazel run` is
# safe here; do not add anything that echoes to stdout.
#
# The single argument is JSON from rust-analyzer, either {"path":"/abs/file.rs"}
# or {"buildfile":"/abs/BUILD"}.
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

stderr_log=$(mktemp -t discover-rust-project)
trap 'rm -f "$stderr_log"' EXIT

bazel run --noshow_progress \
    @rules_rust//tools/rust_analyzer:discover_bazel_rust_project \
    -- "$@" 2>"$stderr_log"
status=$?

if [ $status -ne 0 ]; then
    # Report failure in-band so rust-analyzer can surface it, rather than just
    # closing the pipe and leaving the editor with no explanation.
    python3 -c '
import json, sys
tail = open(sys.argv[1]).read()[-2000:]
print(json.dumps({"kind": "error",
                  "error": "bazel project discovery failed",
                  "source": tail}))
' "$stderr_log"
    cat "$stderr_log" >&2
fi

exit $status
