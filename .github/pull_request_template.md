## What and why

<!-- What changes, and why. -->

## How it was tested

<!-- Commands run, suites passed, anything checked by hand. -->

## Checklist

- [ ] Title is an imperative subject of at most ~65 characters (it becomes the commit on `main`, plus " (#N)")
- [ ] Branch is up to date with `main`
- [ ] `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass locally
- [ ] `CHANGELOG.md` has an entry under `[Unreleased]` (or: no user-visible change)
- [ ] CI passes
- [ ] Reviewed by a separate agent; every finding fixed or answered in this PR

See [CONTRIBUTING.md](https://github.com/vidarrio/bifrost/blob/main/CONTRIBUTING.md) for the process.
