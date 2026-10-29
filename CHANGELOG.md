# Changelog

All notable changes to Bifrost are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `bind` (`BIFROST_BIND`) sets the address Bifrost listens on, for example
  `127.0.0.1` behind a reverse proxy on the same host. The user guide explains
  when that matters for forward-auth.

### Changed

- Seasonal minesweeper earns at most five bonus hints per player per season in
  any 24 hours, and new boards are rate-limited.

### Security

- Releases' `SHA256SUMS` are signed, and `bifrost update` only installs a
  release whose signature verifies against the key built into Bifrost, so a
  file swapped into a release is refused. Releases before signing started
  can't be installed with `bifrost update`.
- Sign-in backoff can't be raced by parallel attempts, and counts an IPv6
  client by its /64, as rate limits now do too. Guessing one account from many
  addresses gets a short wait per attempt (at most a minute); a browser that
  has signed in to that account before (with its current password) skips it.
  Under a flood of sign-ins to many accounts, new browsers may also wait up to
  a minute. The backoff's and the rate limiter's memory is bounded, and
  password hashing runs a few at a time.
- A bonus hint is spent before the free hint is placed, so parallel requests
  can't place several for one, and it's refunded only when the server refuses
  the hint. The free hint names one exact location, so it can't reveal several
  items or, through a slot name with spaces, target another player. An item
  name that can't be matched exactly isn't hinted for free.
- Over HTTPS the session cookie is now `__Host-bifrost_session`, which
  browsers only accept from this host, so a sibling subdomain can't plant one.
  Everyone signs in once more after upgrading.
- Switching the sign-in mode (`bifrost auth mode`) ends every session, so old
  sessions don't come back after switching back.
- Sandboxed jobs run under a syscall filter that refuses new namespaces,
  mounts, kernel keyrings, BPF, `ptrace`, io_uring and similar calls, and
  sockets other than Unix, IP and netlink ones, in both sandbox tiers; the
  settings page shows it. Under Docker with
  `seccomp=unconfined`, apworld code is still filtered.
- PopTracker packs are unpacked from unguessable, owner-only temp files.

## [0.1.2] - 2026-10-29

### Security

- Season URLs accept only plain numeric ids, and requests whose path has
  percent-escapes, `.` or `..` segments or empty segments are refused, so a
  forward-auth proxy and Bifrost always read a path the same way. Upgrading is
  enough; the user guide's proxy rule now also matches any season id, which
  protects instances that haven't upgraded yet.
- Each signup's YAML is stored in a file of its own. A refused signup, or one
  whose name makes the same file name, can no longer replace another player's
  YAML. Signups made before upgrading keep their files: on an instance with
  signups open, check that no two of them share one (names like `Bo b` and
  `Bo_b`).
- Data embedded in pages' scripts, such as item names from uploaded worlds, is
  escaped so it can't end the script and inject markup. The page header no
  longer builds its sign-in links from markup.
- Creating a season's default `host.yaml` no longer loads its uploaded worlds,
  and that step can only write to a scratch folder.
- Sandboxed jobs are capped in how many processes and open files they can
  have. The process cap counts all of Bifrost's user's processes, so run
  Bifrost as a dedicated, non-root user (see Isolation in the user guide).

## [0.1.1] - 2026-10-28

### Added

- A guide to hosting Bifrost on Windows through WSL2, with the container or
  the single-file build.

### Fixed

- The user guide no longer suggests installing the single-file build in a
  root-owned folder such as `/usr/local/bin`, where `bifrost update` can't
  replace it.
- The user guide's Docker flags now include `seccomp=unconfined` (and
  `apparmor=unconfined` where AppArmor is in use). Without them, Docker's
  defaults block bubblewrap, and Bifrost falls back to the weaker sandbox.

## [0.1.0] - 2026-10-28

### Added

- Seasons that go draft → signups → live → archived, with several live at
  once, each on its own Archipelago server and port.
- A three-step signup wizard that checks each YAML with Archipelago's own code
  before accepting it, including co-op signups.
- An apworld catalog curated by sysadmins, optional moderator approval for
  other uploads, hash-pinned worlds, and sandboxed helpers (bubblewrap).
- One-click generation and re-rolls that keep claims.
- A season console for its admins: the connect password, the Archipelago
  server console with auto-completion, and archiving. Admins can also mark a
  slot finished from its game page.
- Summaries of archived seasons on the home page, under *Past seasons*.
- A live board and per-slot game pages: hints with search and prioritization,
  Universal Tracker's in-logic list, PopTracker maps, and activity logs.
- Each player's patch or output files, picked out of the generated seed and
  offered for download on their game page.
- Minesweeper for bonus hints, and an optional shared Minecraft common game.
- Roles (player, moderator, sysadmin), invite-only accounts with built-in
  passwords, or sign-in through an identity provider via forward-auth.
- Installing releases of Bifrost's Archipelago fork (`vidarrio/Archipelago`)
  from the settings page, tested before use, with each live season pinned to
  the version it was generated with.
- Nightly backups with retention, live load monitoring, and season bundles that
  can be imported on another instance.
- A container image and a single-file native Linux build, published with each
  release (the image on `ghcr.io`, the file on the GitHub release), both with
  build provenance.
- Requests to GitHub (Archipelago release checks and installs, the Bifrost
  update check and `bifrost update`) authenticate with `GITHUB_TOKEN` when
  it's set, avoiding GitHub's anonymous rate limit.
- Sysadmins see a banner in the console when a newer Bifrost is released
  (checked daily; `BIFROST_UPDATE_CHECK=off` turns it off).
- Upgrading Bifrost takes a backup before the new version changes anything,
  and the log says how to go back. A data directory a newer Bifrost has
  already upgraded is refused rather than run on by an older one.
- A command line for sysadmins: password-reset links, creating a sysadmin,
  setting roles, switching the sign-in mode, backups (`backup now`, `list`,
  `restore`) and importing season bundles.
- `bifrost update` replaces the single-file build with the newest release (or
  a given version), checked against the release's checksums. Going back means
  restoring the pre-upgrade backup and the previous file, which it keeps.

[Unreleased]: https://github.com/vidarrio/bifrost/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/vidarrio/bifrost/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/vidarrio/bifrost/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/vidarrio/bifrost/releases/tag/v0.1.0
