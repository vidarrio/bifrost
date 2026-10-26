#!/bin/sh
# Build the native single-file bundle into dist/ (see Containerfile).
#   packaging/bundle/build.sh [extra podman/docker build args]
set -eu
cd "$(dirname "$0")/../.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
${CONTAINER:-podman} build -f packaging/bundle/Containerfile --target bundle \
    --build-arg BIFROST_VERSION="$version" --output type=local,dest=dist "$@" .
ls -l dist
