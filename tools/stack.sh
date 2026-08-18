#!/usr/bin/env bash
# Bring up the local test stack: Postgres in Docker, the API on the host.
#
#   tools/stack.sh up       # Postgres, then the API in the foreground
#   tools/stack.sh db       # Postgres alone, detached
#   tools/stack.sh down     # stop Postgres; the data volume survives
#   tools/stack.sh reset    # stop Postgres and delete the data volume
#   tools/stack.sh psql     # a psql shell on the running database
#   tools/stack.sh logs     # follow Postgres logs
#   tools/stack.sh status   # what is running
#
# Only the database is containerized. Building the API in a container means a
# full non-incremental Cargo compile on every edit, which is minutes against
# Bazel's seconds — so `up` starts Postgres, waits for it, and then execs
# `bazel run //api` in the foreground. Ctrl-C stops the API and leaves the
# database running; `down` stops that too.

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
    echo
    echo "Postgres ${URL}"
    echo "Starting the API on http://localhost:8080 — Ctrl-C to stop it."
    echo
    # Migrations are unambiguously safe here: one process, and it owns the
    # database. exec so Ctrl-C reaches the server's own shutdown handler.
    exec env API_DATABASE_MIGRATE_ON_START=true bazel run //api
    ;;
db)
    start_postgres
    echo
    echo "Postgres is up at ${URL}"
    echo "Run the API against it with:"
    echo
    echo "    bazel run //api"
    echo
    echo "Nothing to export: that URL is the development default. Apply"
    echo "migrations on the way up with API_DATABASE_MIGRATE_ON_START=true."
    ;;
down)
    docker compose down
    ;;
reset)
    docker compose down --volumes
    echo "Data volume deleted; the next start re-runs initdb."
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
