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
# shard is a new `shardN` variable, with N added to `shards`.
shard1="apupdate backup forward_auth homepill minesweeper packs upgrade"
shard2="quiet load season_actions"
shard3="catalog supervisor season_import common"
# The demo seeds its seasons by playing them, which takes several minutes.
shard4="demo"
shards="1 2 3 4"
native=""
for n in $shards; do eval "native=\"\${native:+\$native }\$shard$n\""; done
case "${1:-native}" in
    native) suites=$native ;;
    shard)
        # Only a number from `shards` reaches the eval.
        n=${2:-}
        case "$n" in ''|*[!0-9]*) n= ;; esac
        case " $shards " in *" $n "*) ;; *) n= ;; esac
        [ -n "$n" ] || { echo "run.sh: no shard '${2:-}'" >&2; exit 2; }
        eval "suites=\$shard$n" ;;
    shards) echo "[$(echo $shards | sed 's/ /, /g')]"; exit 0 ;;
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
