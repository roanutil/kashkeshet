#!/bin/sh
# dist 0.33.1 does not pass --locked itself. Put this script on PATH as cargo,
# with KKT_RELEASE_CARGO pointing to the real Cargo executable.
set -eu
case "${1:-}" in
    build|metadata|fetch)
        echo "release cargo $1 --locked" >&2
        exec "$KKT_RELEASE_CARGO" "$@" --locked
        ;;
    *) exec "$KKT_RELEASE_CARGO" "$@" ;;
esac
