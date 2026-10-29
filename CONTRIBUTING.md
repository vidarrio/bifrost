# Contributing to Bifrost

This is how changes get into Bifrost and how releases are cut. In short:

- `main` is always in a releasable state.
- Every change lands through a short-lived branch and a pull request.
- A pull request merges only once CI passes and every point from its review
  has been handled.
- Releases are cut from `main` when the maintainer decides.

## Branches

Start each change from the latest `main`, on its own branch, one topic per
branch. Prefix the branch name with the kind of change:

| Prefix | For |
|---|---|
| `feat/` | new features or behaviour |
| `fix/` | bug fixes |
| `refactor/` | restructuring with no behaviour change |
| `docs/` | documentation only |
| `ci/` | CI and tooling |
| `chore/` | dependencies, formatting, housekeeping |
| `release/` | release cuts only (see [Releases](#releases)) |

For example: `feat/season-rehearsal`, `fix/hint-credit-race`.

The kind of change also shows on the pull request: its title starts with it
(see [Pull requests](#pull-requests)), and it's labelled with it
automatically, from the branch prefix.

Keep branches short-lived. If `main` moves on while you work, rebase onto it
and update the pull request with `git push --force-with-lease`.

`main` is protected: nobody pushes to it directly, the maintainer included.
Changes reach it only through a pull request with passing CI (see
[Repository settings](#repository-settings)).

## Commits

Write an imperative subject line of at most 72 characters ("Add season import",
not "Added…"). Use the body to explain why when it isn't obvious. Don't add
tool attribution trailers such as `Co-Authored-By` or "Generated with"; the
[AI disclaimer](AI_DISCLAIMER.md) covers how Bifrost is made.

## Changelog

[`CHANGELOG.md`](CHANGELOG.md) follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Every pull request
with a user-visible change adds a line under `## [Unreleased]`, in the right
section:

- **Added**: new features or documentation.
- **Changed**: changes to existing behaviour.
- **Deprecated**: features that will be removed.
- **Removed**: features that were removed.
- **Fixed**: bug fixes.
- **Security**: fixes for vulnerabilities.

Write entries for the people who run or play on Bifrost, not for developers.
Internal-only changes, such as refactors, CI or tests, don't need an entry, and
neither does a fix to something that hasn't been released yet. Say so in the
pull request instead.

## Pull requests

Open a pull request against `main` once the branch is ready for review. Its
title is `<kind>: <subject>`, where `<kind>` is the branch's prefix without the
slash and `<subject>` is an imperative summary. A scope in parentheses is
optional. For example:

- `feat: import seasons from archive bundles`
- `fix(poptracker): don't hold the engine borrow while handlers run`

A CI check (`title`) fails when the title doesn't have this form or its kind
doesn't match the branch.

The description says:

- what changes and why;
- how it was tested;
- whether it needs a changelog entry (and that it has one).

### CI

CI runs on every pull request and on every push to `main`. It checks:

| Job | Runs |
|---|---|
| `lint` | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, a syntax check of the Python helpers and tests, and that the image and the bundle pin the same Archipelago |
| `test` | `cargo test --workspace` |
| `e2e` | the native end-to-end suites (a debug build on real Archipelago installs, generating seeds and running servers under bubblewrap), split across parallel shards |
| `image` | builds the container image and runs its suite (`tests/e2e/run.sh container`) |
| `bundle` | builds the native bundle and runs its suite (`tests/e2e/run.sh bundle`) |
| `title` | the pull request title is `<kind>: <subject>`, with the branch's kind (see [Pull requests](#pull-requests)) |

`e2e`, `image` and `bundle` are skipped when a pull request only changes
documentation (Markdown files, `docs/`, `LICENSE`). The image and bundle
builds reuse a layer cache that only `main` writes, so a pull request rebuilds
only what it changed.

Run the same commands locally before pushing (see [Development](#development)).

### Review

Every pull request is reviewed by someone other than its author. In practice
that's a separate AI agent, briefed to work from the branch rather than from
the author's own conclusions; [`CLAUDE.md`](CLAUDE.md) has the instructions
agents follow.

The review happens on the pull request:

- The reviewer posts a GitHub review. Its summary says what was checked and
  the overall verdict, and each finding is an inline comment on the line it's
  about, labelled with its severity. Findings that aren't about a line in the
  diff are listed in the summary.
- The author answers every finding on the pull request: inline findings in
  their own threads, summary findings in a reply comment. Each answer says the
  finding is either
  - **fixed**, naming the commit that fixes it, or
  - **answered**: why it isn't a problem, or why it's out of scope.
    Out-of-scope findings that are still worth doing become issues, linked in
    the answer.

If the fixes change something substantial, the reviewer looks again and posts
a follow-up review. If the reviewer and author disagree about a finding, or
it's unclear whether a change is substantial, the maintainer decides.

### Merging

A pull request merges when all of these hold:

1. CI passes.
2. Every review finding has an answer on the pull request (fixed or
   answered).
3. The changelog has its entry, if one is needed.
4. The branch is up to date with `main`.
5. The pull request's checklist is fully ticked: its last items (CI, review)
   get ticked once they're true, just before merging.

Merge with **squash and merge**, the only merge method the repository allows.
The pull request title, kind included, becomes the commit subject on `main`,
with the PR number appended. Keep it to about 65 characters, so the subject
stays under 72 with the number. The branch is deleted automatically.

### Repository settings

These GitHub settings enforce the process:

- **Merging**: squash only (merge commits and rebase merges are off). The
  squash commit takes the pull request's title and an empty body, so fixup
  commits on the branch don't end up on `main`. Merged branches are deleted.
- **A ruleset on `main`**:
  - changes only through a pull request (no approvals required: the
    maintainer can't approve their own pull requests);
  - the CI jobs must pass (as reported by GitHub Actions), on a branch that is
    up to date with `main`;
  - no force pushes and no deletion;
  - nobody can bypass it, the maintainer included.
- **A `release` environment** holds the `RELEASE_SIGNING_KEY` secret (there
  is no repository-level copy). Only `v*` tags can deploy to it, and the
  maintainer is its required reviewer, so every release waits for their
  approval before it is signed: a tag runs the workflow from the tagged
  commit, so the tag rule alone wouldn't keep the key from an edited
  `release.yml`.

## Dependency updates

[Dependabot](.github/dependabot.yml) checks weekly for newer GitHub Actions,
Cargo dependencies and Rust toolchains, and opens a pull request for each.
Actions come as one grouped pull request, and so do Cargo patch updates; any
other Cargo update gets its own, since a minor bump of a `0.x` crate can be
breaking. Its
pull requests follow the same rules as any other: CI, a review, every finding
answered, squash merge.

Two pins are releases rather than packages, so Dependabot can't see them: the
bundle's Python (`PY_URL`/`PY_SHA256` in `packaging/bundle/Containerfile`)
and the Archipelago build (`AP_VERSION` in both container files, `AP_SHA256`
in the bundle's). A weekly workflow (`.github/workflows/pins.yml`) compares
them with the latest releases and keeps an issue, "Pinned releases are
behind", open while either lags, with the exact values to set; bump them in a
`chore:` pull request. It also reports an upstream Archipelago release that
Bifrost's fork has no build of yet; cutting one is the maintainer's call
(`packaging/archipelago/README.md`).

GitHub Actions are pinned to full commit SHAs, with the version in a comment
(`uses: actions/checkout@<sha> # v7.0.1`), so a moved tag can't change what
runs. Dependabot updates both. Pin new actions the same way. Note that
Dependabot's security alerts don't cover SHA-pinned actions; its weekly
version updates are what keeps them current.

## Releases

Releases are cut from `main` **only when the maintainer says so**. Changes
collect under `[Unreleased]` until then.

Versions follow [Semantic Versioning](https://semver.org/). Before 1.0, a minor
version (`0.x.0`) carries new features or breaking changes, and a patch version
(`0.x.y`) carries fixes only. Documentation-only additions, with no change in
behaviour, can also go in a patch version. The first release is `0.1.0`. Only
Bifrost's own version (the root `Cargo.toml`) is bumped; the `poptracker` crate
is internal and keeps its version.

To cut release `X.Y.Z`:

1. **Branch** `release/vX.Y.Z` from the latest `main`.
2. **Changelog**: rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` and add
   a fresh, empty `## [Unreleased]` above it. Then update the link references
   at the bottom:

   ```
   [Unreleased]: https://github.com/vidarrio/bifrost/compare/vX.Y.Z...HEAD
   [X.Y.Z]: https://github.com/vidarrio/bifrost/compare/vPREV...vX.Y.Z
   ```

   For the first release, which has no previous tag, the version's link is
   `https://github.com/vidarrio/bifrost/releases/tag/v0.1.0`. These links 404
   until step 5 pushes the tag; that's expected.
3. **Version**: set `version = "X.Y.Z"` in the root `Cargo.toml`, then run
   `cargo update --workspace` so `Cargo.lock` follows.
4. **Pull request**: open it as `release: vX.Y.Z`, with CI and review like any
   other change. Review checks that the changelog is complete and accurate for
   the release. Then merge it.
5. **Tag** the squash commit that `release: vX.Y.Z` created on `main`, and push
   the tag. Check first that `git log -1 origin/main` is that commit; if
   something else has merged since, tag the release commit's SHA instead.

   ```sh
   git fetch origin
   git tag -a vX.Y.Z -m "Bifrost X.Y.Z" origin/main
   git push origin vX.Y.Z
   ```

6. **The release workflow** (`.github/workflows/release.yml`) takes it from
   there. It checks that the tag matches `Cargo.toml` and that the changelog
   has a `[X.Y.Z]` section, then:
   - builds the container image and pushes it to
     `ghcr.io/vidarrio/bifrost` as `X.Y.Z`, `X.Y` and `latest` (a version
     with a pre-release suffix, like `0.2.0-rc.1`, gets only `X.Y.Z-…` and is
     marked as a pre-release);
   - builds the native bundle and its `SHA256SUMS`, and signs `SHA256SUMS`
     with the `RELEASE_SIGNING_KEY` secret of the `release` environment
     (which only `v*` tags can deploy to; an ed25519 key in PEM form) into
     `SHA256SUMS.sig`, after checking it matches the public key in
     `src/selfupdate.rs` (`RELEASE_KEY`). `bifrost update` refuses a release
     whose signature doesn't verify, so a release can't go out without it;
   - attests the build provenance of both;
   - creates the GitHub release with the changelog section as its notes and
     the bundle, checksums and signature attached.

   Check the run and the release page afterwards, and that the image pulls
   without logging in (`podman logout ghcr.io; podman pull
   ghcr.io/vidarrio/bifrost:X.Y.Z`). The package is public; if the pull is
   refused, make it public again in its settings on GitHub.

A fix for a released version goes through a `fix/` branch like any other
change, followed by a patch release when the maintainer calls for one.

Releases of Bifrost's Archipelago fork are separate; see
[`packaging/archipelago/README.md`](packaging/archipelago/README.md).

## Development

```sh
cargo fmt --all                                        # format
cargo clippy --workspace --all-targets -- -D warnings  # lint
cargo test --workspace                                 # unit tests
cargo build && BIFROST_DATA=./data BIFROST_AP_DIR=/path/to/Archipelago \
    BIFROST_RUNTIME_DIR=. BIFROST_INSECURE_COOKIES=1 target/debug/bifrost
```

`BIFROST_AP_DIR` must point at an Archipelago checkout with a `venv/`.

The Rust toolchain is pinned to an exact release in `rust-toolchain.toml`
(rustup installs it on first use). Bump it in a `chore:` pull request of its
own, together with the `rust:` base image in `Dockerfile` and
`packaging/bundle/Containerfile` and `rust-version` in `Cargo.toml` (the
release without its patch number), and fix whatever new clippy lints it brings.
`lint` fails until all four agree. When Dependabot proposes a toolchain bump,
push the other three changes onto its pull request.

### Database changes

The database records its schema version, and Bifrost migrates it on start
(`src/db/migrate.rs`). The tables as the first release made them are the
baseline (`baseline` in `src/db.rs`) and stay frozen. To change the schema,
append a step to `STEPS` in `src/db/migrate.rs`; never edit or remove a step
that has been released. Each step runs once, in a transaction, before
anything else reads the database. Steps run with foreign keys off, so
rebuilding a table (SQLite's way of changing a column) doesn't cascade into
the tables that reference it, and the references are checked before the step
commits. The first start of a new version takes a pre-upgrade backup before
migrating; restoring it and running the old version undoes the upgrade.

### End-to-end tests

The suites in `tests/e2e/` start real Bifrost instances on throwaway data
directories and drive them over HTTP. They need Python 3 with `requests`
(`pip install -r tests/e2e/requirements.txt`, or your distribution's package),
`bubblewrap`, and an Archipelago install, which `setup-ap.sh` provides:

```sh
tests/e2e/setup-ap.sh                # installs the Archipelago build the image ships
cargo build
tests/e2e/run.sh                     # all native suites
tests/e2e/run.sh catalog quiet       # or just some
```

`tests/e2e/run.sh packaging` runs the container and bundle suites; build the
image as `bifrost:dev` (`podman build -t bifrost:dev .`) and the bundle
(`packaging/bundle/build.sh`) first. Scratch space and paths are set by the
environment variables described at the top of `tests/e2e/lib.py`.

Two extras run only by hand, not in CI: `SHOT=<file.png> python3 load.py`
saves screenshots of the load panel (it needs a Playwright headless Chromium
under `~/.cache/ms-playwright`), and `python3 quiet.py upstream` runs the
quiet-connections control against a plain upstream Archipelago install at
`$BIFROST_E2E_WORK/archipelago`.
