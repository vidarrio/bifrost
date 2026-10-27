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

For example: `feat/season-rehearsal`, `fix/hint-credit-race`.

Keep branches short-lived. If `main` moves on while you work, rebase onto it.

`main` is protected: nobody pushes to it directly, the maintainer included.
Changes reach it only through a pull request with passing CI.

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

- **Added**: new features.
- **Changed**: changes to existing behaviour.
- **Deprecated**: features that will be removed.
- **Removed**: features that were removed.
- **Fixed**: bug fixes.
- **Security**: fixes for vulnerabilities.

Write entries for the people who run or play on Bifrost, not for developers.
Internal-only changes, such as refactors, CI or tests, don't need an entry. Say
so in the pull request instead.

## Pull requests

Open a pull request against `main` once the branch is ready for review. The
description says:

- what changes and why;
- how it was tested;
- whether it needs a changelog entry (and that it has one).

### CI

CI runs on every pull request and on every push to `main`. It checks:

| Job | Runs |
|---|---|
| `lint` | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, a syntax check of the Python helpers |
| `test` | `cargo test --workspace` |

Run the same commands locally before pushing (see [Development](#development)).

### Review

Every pull request is reviewed by a separate AI agent, one that didn't write
the change. The reviewer works from the branch and a description of what to
check, not from the author's own conclusions, so its read isn't anchored on
them. It reports findings ranked by severity, and the findings are posted to
the pull request as a comment.

Every finding is then handled in one of two ways:

- **Fixed** with a commit on the branch.
- **Answered** in the pull request, saying why it isn't a problem or why it's
  out of scope. Out-of-scope findings that are still worth doing become issues.

If the fixes change something substantial, the reviewer looks again.

### Merging

A pull request merges when all of these hold:

1. CI passes.
2. Every review finding is fixed or answered.
3. The changelog has its entry, if one is needed.
4. The branch is up to date with `main`.

Merge with **squash and merge**: the pull request title becomes the commit
subject on `main`, so make it a good one. Delete the branch afterwards.

## Releases

Releases are cut from `main` **only when the maintainer says so**. Changes
collect under `[Unreleased]` until then.

Versions follow [Semantic Versioning](https://semver.org/). Before 1.0, a
minor version (`0.x.0`) carries new features or breaking changes, and a patch
version (`0.x.y`) carries fixes only.

To cut release `X.Y.Z`:

1. **Branch** `release/vX.Y.Z` from the latest `main`.
2. **Changelog**: rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD`, add a
   fresh empty `## [Unreleased]` above it, and update the comparison links at
   the bottom.
3. **Version**: set `version = "X.Y.Z"` in `Cargo.toml`, and run `cargo build`
   so `Cargo.lock` follows.
4. **Pull request**: open it as `Release vX.Y.Z`, with CI and review like any
   other change. Review checks that the changelog is complete and accurate for
   the release. Then merge it.
5. **Tag** the merge commit on `main` and push the tag:

   ```sh
   git tag -a vX.Y.Z -m "Bifrost X.Y.Z" <merge-commit>
   git push origin vX.Y.Z
   ```

6. **GitHub release** from the tag, using that version's changelog section as
   the notes:

   ```sh
   gh release create vX.Y.Z --title "Bifrost X.Y.Z" --notes-file <section.md>
   ```

   Build the native bundle from the tag (`packaging/bundle/build.sh`) and
   attach it to the release.

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
