# Instructions for Claude

How Claude (or any AI agent) works on this repository. The process itself is
in [CONTRIBUTING.md](CONTRIBUTING.md); this file adds what an agent needs on
top of it. Where the two disagree, CONTRIBUTING.md wins.

## Ground rules

- Follow CONTRIBUTING.md: one prefixed branch per change, a pull request,
  CI and a review before merging, squash merges.
- Never push to `main`. A ruleset enforces this anyway; don't try to get
  around it.
- Don't add attribution trailers or "Generated with" lines to commits, pull
  requests or comments.
- Cut a release only when the maintainer asks for one.
- Before pushing, run `cargo fmt --all`,
  `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace`. For changes that touch behaviour, also run the
  end-to-end suites that cover it (`tests/e2e/run.sh <suite>…`).
- Add a `CHANGELOG.md` entry for user-visible changes, under the rules in
  CONTRIBUTING.md.
- Pin any GitHub Action you add to a full commit SHA, with its version in a
  comment (`@<sha> # vX.Y.Z`); resolve the SHA from the release tag
  (`gh api repos/<owner>/<repo>/commits/<tag> -q .sha`).
- The `gh` CLI posts as the maintainer's account. Anything an agent posts has
  to say which agent wrote it (see below).

## Reviewing a pull request

The author starts a separate agent as reviewer. The brief gives it the pull
request number, the branch, what the change is meant to do (the issue or the
maintainer's request), and what to check. It doesn't give the author's own
conclusions. The reviewer works read-only: it doesn't touch the author's
working tree, and it commits or pushes nothing. If it needs to build, it uses
a worktree and target directory of its own.

The reviewer posts one GitHub review on the pull request's head commit. It
writes the review to a file outside the repository, named after the pull
request (e.g. `pr<n>-review.json` in a scratch directory), so that it neither
touches the author's tree nor overwrites another review:

```sh
gh api -X POST repos/vidarrio/bifrost/pulls/<n>/reviews --input pr<n>-review.json
```

```json
{
  "commit_id": "<head sha>",
  "event": "COMMENT",
  "body": "**Review by a separate agent (Claude), not the author.**\n\n<verdict, what was checked, findings not tied to a diff line>",
  "comments": [
    { "path": "src/x.rs", "line": 42, "side": "RIGHT",
      "body": "**Medium.** <what's wrong>. <suggested fix>" }
  ]
}
```

- **Event: always `COMMENT`.** GitHub rejects `APPROVE` and `REQUEST_CHANGES`
  on the account's own pull requests, and every agent posts from the
  maintainer's account.
- **Anchors.** Every inline comment must sit on a line inside one of the diff's
  hunks, on the side the line belongs to (`RIGHT` for added or unchanged
  lines). A single comment outside the hunks makes GitHub reject the whole
  review. Findings about lines outside the diff go in the body.
- **Severity.** Label each comment (**Blocker**, **High**, **Medium**,
  **Low**, **Nit**). GitHub shows comments in file order, so the body lists
  the findings from most to least severe.
- **Body.** Open with the line above. Then give the verdict, what was checked
  and found fine (briefly), and any findings not tied to a diff line. If there
  are no findings, say so plainly.

## Answering a review

The author answers every finding on the pull request.

- **Inline findings** get a reply in their own thread, even when a later
  commit has made the thread outdated. `<comment id>` is the id of the
  thread's first comment (the review's inline comment), not of a reply.

  ```sh
  gh api -X POST repos/vidarrio/bifrost/pulls/<n>/comments/<comment id>/replies -F body=@reply.md
  ```

- **Findings in the review body** get one reply comment on the pull request,
  with one item per finding, in the body's order:

  ```sh
  gh pr comment <n> -R vidarrio/bifrost --body-file reply.md
  ```

  Write the text to a file (outside the repository) rather than quoting it on
  the command line, where the first apostrophe would end it.

- Each answer opens with "**Reply from the author (Claude).**". It then says
  **Fixed in `<sha>`**, with a sentence on how, or gives the reason the
  finding isn't a problem or is out of scope. For out-of-scope findings worth
  doing, open an issue and link it.
- Leave resolving threads to the maintainer.

## Follow-up reviews

When the fixes are substantial, ask the reviewer to look again. It reads the
existing threads and replies first, then posts a new review the same way, on
the new head commit. The follow-up covers whether each fix works, anything
new, and any answer it disagrees with. It raises a disagreement as a reply in
that finding's thread, opening with "**Reply from the reviewer (Claude).**".
The maintainer settles disagreements.

## Merging

When CI passes and every review finding has an answer, finish the pull
request's description before merging:

1. Tick its remaining checklist boxes, typically "CI passes" and the review
   item, but only the ones that are now true. If one isn't, the pull request
   isn't ready to merge.

   ```sh
   gh pr view <n> -R vidarrio/bifrost --json body -q .body | sed 's/^- \[ \]/- [x]/' > body.md
   # check body.md: every ticked item must hold
   gh pr edit <n> -R vidarrio/bifrost --body-file body.md
   ```

2. Merge with `gh pr merge <n> -R vidarrio/bifrost --squash`. Never use
   `--admin`. If the merge is refused, a required check is still running or
   failing (editing the title or body reruns `title`): wait for it.

## If posting is blocked

Claude Code's permission settings may block writes to GitHub. Don't look for a
way around a block. Tell the maintainer what was blocked and give them the
exact command, so they can allow it or run it themselves.
