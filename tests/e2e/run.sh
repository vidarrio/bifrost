#!/bin/sh
# Run end-to-end suites and summarize. With no arguments: the native suites
# (a debug build on this host, Archipelago from setup-ap.sh). `packaging` runs
# the container image and native bundle suites (they need podman, the image
# built as bifrost:dev and the bundle in dist/).
#   tests/e2e/run.sh [native|packaging|<suite>...]
set -u
cd "$(dirname "$0")"
native="homepill catalog load season_import backup quiet supervisor apupdate"
case "${1:-native}" in
    native) suites=$native ;;
    packaging) suites="container bundle" ;;
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
