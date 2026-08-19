#!/usr/bin/env bash
# Bring up the local test stack: Postgres in Docker, the UI and API on the host.
#
#   tools/stack.sh up       # Postgres, migrations, then the UI and the API
#   tools/stack.sh db       # Postgres alone, detached
#   tools/stack.sh ui       # the Astro dev server on :4321
#   tools/stack.sh down     # stop Postgres; the data volume survives
#   tools/stack.sh reset    # stop Postgres and delete the data volume
#   tools/stack.sh migrate  # apply pending migrations and exit
#   tools/stack.sh psql     # a psql shell on the running database
#   tools/stack.sh logs     # follow Postgres logs
#   tools/stack.sh status   # what is running
#
# Only the database is containerized. Building the API in a container means a
# full non-incremental Cargo compile on every edit, which is minutes against
# Bazel's seconds — so `up` starts Postgres, migrates, and then runs the UI and
# the API on the host. Ctrl-C stops both and leaves the database running;
# `down` stops that too.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

readonly URL="postgres://monorail:monorail@localhost:5432/monorail"

# The header comment above is the help text; print it back rather than keeping a
# second copy that drifts.
usage() {
    awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"
    exit "${1:-0}"
}

docker compose version >/dev/null 2>&1 || {
    echo "docker compose is not available; install Docker Desktop or the compose plugin" >&2
    exit 1
}

# --wait returns when the healthcheck passes rather than when the container
# starts, so the API never races initdb.
start_postgres() { docker compose up --detach --wait postgres; }

case "${1:-}" in
up)
    start_postgres
    bazel run //api:migrate
    # `bazel run` holds the workspace lock for as long as the process it starts,
    # so a second one would block until the first exits. The UI gets a generated
    # launcher instead, leaving the lock free for the API.
    ui_script="$(mktemp -t monorail-ui)"
    bazel run --script_path="${ui_script}" //ui:dev
    # `kill 0` and not the job's pid: the launcher does not pass signals on to
    # the astro process it spawns, so that one outlives a targeted kill. The
    # whole process group goes down together instead.
    trap 'rm -f "${ui_script}"; trap - EXIT; kill 0' EXIT
    "${ui_script}" &
    echo
    echo "Postgres ${URL}"
    echo "Starting the UI on http://localhost:4321 and the API on"
    echo "http://localhost:8080 — Ctrl-C to stop both."
    echo
    bazel run //api
    ;;
ui)
    exec bazel run //ui:dev
    ;;
db)
    start_postgres
    echo
    echo "Postgres is up at ${URL}"
    echo "Run the API against it with:"
    echo
    echo "    bazel run //api:migrate    # once, after adding a migration"
    echo "    bazel run //api"
    echo
    echo "Nothing to export: that URL is the development default."
    ;;
down)
    docker compose down
    ;;
reset)
    docker compose down --volumes
    echo "Data volume deleted; the next start re-runs initdb."
    ;;
migrate)
    start_postgres
    bazel run //api:migrate
    ;;
psql)
    docker compose exec postgres psql -U monorail -d monorail
    ;;
logs)
    shift
    docker compose logs --follow "$@"
    ;;
status)
    docker compose ps
    ;;
"" | -h | --help | help)
    usage
    ;;
*)
    echo "unknown command: $1" >&2
    usage 1 >&2
    ;;
esac
