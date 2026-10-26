# Bifrost's Archipelago fork

Bifrost runs Archipelago from a fork, [vidarrio/Archipelago], so it can carry
small compatibility patches. Each Bifrost build is an upstream release tag
with the patches in `patches/` applied on top:

- branch `bifrost/<tag>`, tag and GitHub release `<tag>-bifrost.<build>`
  (e.g. `0.6.8-bifrost.1`);
- a `BIFROST_VERSION` file at its root naming the build. Bifrost uses that
  as the install's version (`src/apinstall.rs`) and shows players the upstream
  part.

The settings page lists and installs the fork's releases (`BIFROST_AP_REPO`
defaults to the fork). The image and the native bundle pin one build
(`AP_VERSION` / `AP_SHA256` in `Dockerfile` and `packaging/bundle/Containerfile`).

## Patches

| Patch | What it does |
|---|---|
| `0001` MultiServer: no announcements for Bifrost's own clients | Clients tagged `Bifrost` join, leave, change tags and use `!admin` without a broadcast to everyone. Bifrost's tools (board tracker, points, hint and admin relays, goal reporter, in-logic tracker) all send the tag (`QUIET_TAG` in `src/ap.rs`). Hints and goals are still announced as usual. |

Keep patches small and local, so they carry forward across upstream releases.
On an upstream server everything still works: the tag just shows up in the
announcements.

## Cutting a build

```sh
packaging/archipelago/release.sh <upstream-tag> [<build>]
```

The script applies `patches/` to the upstream tag, writes `BIFROST_VERSION`,
pushes the branch and tag to the fork, creates the release (a prerelease for
rc tags) and prints the source tarball's sha256 for the bundle's `AP_SHA256`.
Builds are immutable: when the patches change, cut `<build>+1`. Upstream
releases don't show up in Bifrost until a build of them is cut.

To add a patch, commit it on a `bifrost/<tag>` checkout and export the series:

```sh
git format-patch --zero-commit --no-signature -o packaging/archipelago/patches <tag>
```

The fork is public, so commit as your GitHub noreply address (`release.sh`
sets that up in its clone).

[vidarrio/Archipelago]: https://github.com/vidarrio/Archipelago
