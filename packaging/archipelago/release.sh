#!/bin/sh
# Cut a Bifrost build of an Archipelago release on the fork (see README.md):
# upstream tag + patches/ → branch bifrost/<tag>, tag <tag>-bifrost.<build>,
# and a GitHub release, which Bifrost's settings page then offers.
#
#   packaging/archipelago/release.sh <upstream-tag> [<build>]
#
# FORK (default vidarrio/Archipelago), UPSTREAM (ArchipelagoMW/Archipelago),
# WORK (the clone, default ~/.cache/bifrost-ap-fork). Needs git, gh, curl.
set -eu

[ $# -ge 1 ] || { sed -n '2,10p' "$0"; exit 2; }
tag=$1
build=${2:-1}
rel="$tag-bifrost.$build"
FORK=${FORK:-vidarrio/Archipelago}
UPSTREAM=${UPSTREAM:-ArchipelagoMW/Archipelago}
WORK=${WORK:-$HOME/.cache/bifrost-ap-fork}
patches=$(cd "$(dirname "$0")/patches" && pwd)

if gh release view "$rel" -R "$FORK" >/dev/null 2>&1; then
    echo "$rel is already released; builds are immutable — pass the next build number" >&2
    exit 1
fi

if [ ! -d "$WORK/.git" ]; then
    git clone -q --filter=blob:none --no-checkout "https://github.com/$FORK.git" "$WORK"
fi
cd "$WORK"
git remote get-url upstream >/dev/null 2>&1 \
    || git remote add upstream "https://github.com/$UPSTREAM.git"
# The fork is public: commit as the GitHub account's noreply address.
if [ -z "$(git config --local user.email || true)" ]; then
    login=$(gh api user -q .login)
    git config --local user.name "$login"
    git config --local user.email "$(gh api user -q .id)+$login@users.noreply.github.com"
fi
git fetch -q upstream "refs/tags/$tag:refs/tags/$tag"

git checkout -q -B "bifrost/$tag" "refs/tags/$tag"
git am -q --committer-date-is-author-date "$patches"/*.patch
echo "$rel" > BIFROST_VERSION
git add BIFROST_VERSION
git commit -q -m "Bifrost build $rel"
git tag -a "$rel" -m "Archipelago $tag with Bifrost's patches (build $build)"
git push -q --force "https://github.com/$FORK.git" "bifrost/$tag"
git push -q "https://github.com/$FORK.git" "refs/tags/$rel"

notes=$(printf 'Archipelago %s with Bifrost'"'"'s compatibility patches:\n\n' "$tag"
        git log --format='- %s' "refs/tags/$tag..HEAD~1")
pre=
case "$tag" in *rc*|*alpha*|*beta*) pre=--prerelease ;; esac
gh release create "$rel" -R "$FORK" --verify-tag $pre \
    --title "Archipelago $tag for Bifrost (build $build)" --notes "$notes" >/dev/null

sum=$(curl -fsSL "https://github.com/$FORK/archive/refs/tags/$rel.tar.gz" | sha256sum | cut -d' ' -f1)
echo "released $rel"
echo "source tarball sha256: $sum"
