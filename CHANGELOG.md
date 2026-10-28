# Changelog

All notable changes to Bifrost are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security

- Season URLs accept only plain numeric ids, and requests whose path has
  percent-escapes, `.` or `..` segments or empty segments are refused, so a
  forward-auth proxy and Bifrost always read a path the same way. Upgrading is
  enough; the user guide's proxy rule now also matches any season id, which
  protects instances that haven't upgraded yet.
- Each signup's YAML is stored in a file of its own. A refused signup, or one
  whose name makes the same file name, can no longer replace another player's
  YAML.

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

[Unreleased]: https://github.com/vidarrio/bifrost/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/vidarrio/bifrost/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/vidarrio/bifrost/releases/tag/v0.1.0
