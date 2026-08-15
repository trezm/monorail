#!/usr/bin/env bash
# Report and prune the build caches shared across worktrees.
#
#   tools/cache.sh          # report only
#   tools/cache.sh --prune  # also delete output bases of deleted workspaces
#
# Bazel gives every workspace path its own output base and never reclaims one
# when the directory disappears, so deleted worktrees leave a few hundred MB
# behind each. Each output base records its workspace in DO_NOT_BUILD_HERE;
# an output base is dead exactly when that path no longer exists.

set -euo pipefail

PRUNE=false
[[ "${1:-}" == "--prune" ]] && PRUNE=true

cd "$(git rev-parse --show-toplevel)"

human() { du -sh "$1" 2>/dev/null | cut -f1 || echo "?"; }

# `output_user_root` is not an info key; it is the parent of this workspace's
# output base, and holds one output base per workspace path on this machine.
output_base=$(bazel info output_base)
output_user_root=$(dirname "$output_base")

echo "Worktrees of this repo"
while IFS= read -r path; do
    marker=$(grep -rlxF "$path" "$output_user_root"/*/DO_NOT_BUILD_HERE 2>/dev/null | head -1 || true)
    if [[ -n "$marker" ]]; then
        printf '  %-8s %s\n' "$(human "$(dirname "$marker")")" "$path"
    else
        printf '  %-8s %s (never built)\n' "-" "$path"
    fi
done < <(git worktree list --porcelain | awk '/^worktree /{print $2}')

# --- Output bases whose workspace directory is gone --------------------------
declare -a orphans=()
for base in "$output_user_root"/*/; do
    base=${base%/}
    [[ "$(basename "$base")" =~ ^[0-9a-f]{32}$ ]] || continue

    workspace=$(cat "$base/DO_NOT_BUILD_HERE" 2>/dev/null || true)
    # No marker means Bazel never finished setting the output base up; leave it
    # alone rather than guess.
    [[ -n "$workspace" ]] || continue
    [[ -d "$workspace" ]] && continue

    orphans+=("$base|$workspace")
done

echo
if [[ ${#orphans[@]} -eq 0 ]]; then
    echo "Output bases with no workspace: none"
else
    echo "Output bases whose workspace is gone (${#orphans[@]})"
    for entry in "${orphans[@]}"; do
        printf '  %-8s %s\n' "$(human "${entry%%|*}")" "${entry#*|}"
    done
    echo
    if $PRUNE; then
        for entry in "${orphans[@]}"; do
            echo "  removing $(basename "${entry%%|*}") (${entry#*|})"
            rm -rf "${entry%%|*}"
        done
    else
        echo "  re-run with --prune to delete them"
    fi
fi

# --- Shared caches -----------------------------------------------------------
echo
echo "Shared caches (one copy, never per-worktree)"
for dir in "$HOME/.cache/bazel/monorail/disk" \
    "$HOME/.cache/bazel/monorail/repo" \
    "$HOME/.cache/monorail/cargo-target"; do
    [[ -d "$dir" ]] && printf '  %-8s %s\n' "$(human "$dir")" "$dir"
done

echo
echo "The disk cache is size-bounded by .bazelrc and garbage-collects itself."
