#!/bin/sh
# Run end-to-end suites and summarize. With no arguments: the native suites
# (a debug build on this host, Archipelago from setup-ap.sh). `shard N` runs
# one of the CI shards of the native suites. `packaging` runs the container
# image and native bundle suites (they need podman, the image built as
# bifrost:dev and the bundle in dist/).
# `list` prints every suite CI runs, one per line; `shards` prints the shard
# numbers as a JSON array (CI builds its matrix from it).
#   tests/e2e/run.sh [native|shard <n>|packaging|list|shards|<suite>...]
set -u
cd "$(dirname "$0")"
# The native suites, in CI's parallel shards (balanced by run time, about two
# minutes each on a runner). A new native suite goes into one of them; a new
# shard also goes into `shard)` and `shards)` below.
shard1="apupdate backup homepill minesweeper packs upgrade"
shard2="quiet load"
shard3="catalog supervisor season_import"
native="$shard1 $shard2 $shard3"
case "${1:-native}" in
    native) suites=$native ;;
    shard)
        case "${2:-}" in
            1) suites=$shard1 ;;
            2) suites=$shard2 ;;
            3) suites=$shard3 ;;
            *) echo "run.sh: no shard '${2:-}'" >&2; exit 2 ;;
        esac ;;
    shards) echo '[1, 2, 3]'; exit 0 ;;
    packaging) suites="container bundle" ;;
    list) printf '%s\n' $native container bundle; exit 0 ;;
    *) suites="$*" ;;
esac
failed=""
for s in $suites; do
    echo "::group::$s"
    if python3 "$s.py"; then echo "== $s passed"; else echo "== $s FAILED"; failed="$failed $s"; fi
    echo "::endgroup::"
done
if [ -n "$failed" ]; then
    echo "failed suites:$failed"
    exit 1
fi
echo "all suites passed: $suites"
