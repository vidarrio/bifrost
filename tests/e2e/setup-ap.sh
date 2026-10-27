#!/bin/sh
# Install the Archipelago build the image ships (Dockerfile's AP_VERSION) into
# $BIFROST_E2E_AP_DIR for the end-to-end suites, with Bifrost's own installer.
# Does nothing when that version is already there.
set -eu
repo=$(cd "$(dirname "$0")/../.." && pwd)
work=${BIFROST_E2E_WORK:-$HOME/.cache/bifrost-dev}
dest=${BIFROST_E2E_AP_DIR:-$work/archipelago-bifrost}
version=$(sed -n 's/^ARG AP_VERSION=//p' "$repo/Dockerfile")
fork=${BIFROST_AP_REPO:-vidarrio/Archipelago}
if [ "$(cat "$dest/BIFROST_VERSION" 2>/dev/null)" = "$version" ] && [ -x "$dest/venv/bin/python" ]; then
    echo "Archipelago $version already at $dest"
    exit 0
fi
rm -rf "$dest"
mkdir -p "$(dirname "$dest")"
python3 "$repo/scripts/bifrost_ap_install.py" install "$fork" "$version" "$dest"
echo "Archipelago $version installed at $dest"
