# Changelog

All notable changes to Bifrost are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Held signups: a YAML for a world awaiting approval gets checked as far as it can be
  without running the world, and can be replaced while it waits. A signup that fails its
  check shows a clear notice, on the season's page and to moderators, who can replace its
  YAML or dismiss it. The approval screen lists who's waiting, and a refused generation
  names the signups.
- A season's moderators can delete it before generation, and cancel a live one back to
  signups, keeping its signups, notes and partners. Sysadmins can delete an archived
  season. Each asks you to type the season's name.
- A warning in the README and the user guide that Bifrost is in very early development,
  so read the changelog before updating and expect breaking changes.
- The minesweeper on the home page is for any account, whether or not they play in a
  season. A season's own minesweeper is for its players and those running it.
- Private seasons. A new season's board, log and game pages are for its players,
  its moderators and sysadmins, and a moderator can open them to anyone. Moderators can also make
  a season's signups private, reachable only through a link. Non-members don't see a
  private season on the home page or in past seasons. Seasons you already have, and
  imported ones, stay open after generation. Before generation, every season's pages
  now need a sign-in.
- Co-op partners join through one-use invite links from a game's host, before or after
  generation, and pick their own seat name. They can edit the game's notes and, before
  generation, rename their seat or leave. Signing up takes you to your game's page.
- *Sign up for a season* on the home page now lists the seasons open for signup. Each
  game on a season's page opens its own page, with notes for the other players and, for
  its host, a way to replace the YAML without losing the signup. A season's moderators
  can edit the notes and, until generation, replace the YAML.
- A player can sign up with several games in a season: once you have one, the
  signup page offers *Add another game*. A season's moderators can limit it on the
  Options tab (0, the default, means no limit), and a player can claim more than one
  slot in a season. Each signup's YAML now has to hold one player; a file with several
  is refused.
- PopTracker items draw with their packs' image filters, `disabled_img` and overlay
  images, and maps draw trapezoid, triangle and kite markers with their orientation
  and border, as packs ask.
- PopTracker packs' watches on item codes run when those items change, so state a
  pack derives from other items keeps up.
- PopTracker packs can read their slot's hints and Archipelago's data storage, from
  the season's save, so as often as the server saves it (by default, every minute).
  Packs that track hints or a game's
  own progress there now show them, and hinted locations are outlined on the map.
  Nothing a pack does can write to the server.
- Common games. A sysadmin keeps a catalog of co-op games they host, such as a
  Minecraft, Factorio or Terraria server, one folder each under `common/` in the data
  directory, with its YAML, any apworld, a join guide in Markdown and client files to
  download. A season can include several, including two setups of one game, each
  played as one shared slot with its own address. Players register the name they play
  under once per kind of game, and moderators download each common game's allowlist
  for the season, as text or JSON. `docs/examples/common` has a Minecraft setup to
  start from. Sysadmins can also make, edit and delete setups, with their apworld and
  downloads, under Console → Common games.
- A read-only export token for common games, made by a sysadmin on the settings page,
  so their own script can fetch every live season's common games, allowlists and seed
  files from `/export/...` with `curl`.

### Security

- When `config.toml` doesn't parse, the error gives the line and column but no longer
  quotes the line, which could hold a secret.
- Bifrost refuses to run as root, commands included, unless `BIFROST_ALLOW_ROOT=1` is set
  in the environment. If you run it as root, switch to its own user, and `chown` its data
  directory to that user first.
- Run as root, Bifrost won't start if another user could change the data directory,
  anything in it, a folder above it, or a link on the way to it. For any user, a bundle's unpacking no longer writes
  through a link left at the paths it creates.
- PopTracker packs' Lua runs in a sandboxed process of its own, stopped after
  30 seconds (`pack_timeout_secs`), so a pack can't tie up the server. Each
  slot computes its tracker once at a time, at most four run at once, and
  uploads are rate-limited. A pack that hits a limit isn't run again for a
  minute, then longer each time, or until it's replaced; a slow pack's tracker
  updates less often.

### Fixed

- A game page's figures (locations, sent, received, left) and its list of hints for the slot's
  items keep themselves current while the page is open, as the board does.
- A game page's Universal Tracker tab keeps itself current while it's open, as the
  PopTracker and activity tabs do, and the "available" count at the top updates with it.
  Before, it showed the result from when the page loaded.
- A game's status on the board reflects its players, not Bifrost. Bifrost's own short
  connections no longer make a game flash "connected", and the game Bifrost's tracker
  connects as now shows a real player connecting and clears when they leave, instead of
  staying "playing". This needs Archipelago 0.6.8-bifrost.4, now the bundled build; on
  older builds and external servers, these statuses stay as they were. On that build, a
  game's server that was killed rather than stopped no longer keeps a status other than a
  goal after it restarts.
- A URL nothing answers shows a page, rather than the browser's own error, and so do "Moderators only"
  and a game page with no game to show.
- The first game on a board no longer says "connected" just because Bifrost's own tracker joined as it.
- A season's Archive tab only offers archiving once it's live, as it couldn't be done before then.
- A game page only links to minesweeper when the season has it on.
- A game page's notes and YAML say when they've been saved.
- Long links and paths no longer widen the page, a failed generation shows as a failure, and the
  apworld approval options are laid out properly.
- A game's PopTracker pack stays with it through a re-roll. Packs are stored by game id, not by
  the slot number a re-roll can change.
- PopTracker locations honour `parent`, and map markers their own
  `restrict_visibility_rules` and `force_invisibility_rules`.
- PopTracker packs that also read an emulator's memory, such as Super Metroid's,
  no longer lose their received items: only Archipelago reads as connected.
- PopTracker items behave as in PopTracker: their starting stage, `allow_disabled`,
  `inherit_codes`, `static` and `composite_toggle` items, several items sharing a
  code, custom items counting toward rules, sections that host an item, and counts
  and stages kept within their limits.
- PopTracker access rules work as they do in PopTracker: `{}` inspect rules,
  `^$` levels, `[optional]` references, `@location/section` references including
  locations that aren't drawn, section `ref`, locations with the same name, nested
  rules, and how a location's rules join its parent's. Many locations change
  colour, and large logic packs that never finished computing now do.
- Many more PopTracker packs load. Bifrost picks the pack's Archipelago map
  variant and finds files in its folder, and supplies more of what packs call,
  such as overlays and `os.time`. An error in one of a pack's handlers no longer
  blanks its tracker: it carries on, and the tab says tracking may be incomplete.
- PopTracker packs that load their scripts with `require`, or use custom
  items, frame handlers or change watches, can be uploaded and track (for
  example the Satisfactory pack). `require` only loads files from the pack.
- PopTracker access rules that pass arguments to one of the pack's functions
  (`$name|arg`) give it those arguments.
- PopTracker packs get the slot's slot data, the seed name and the other
  players' names and games, as they would from Archipelago on connect. Packs
  that read the seed's options there now track them, such as Satisfactory's
  starting recipes.

### Changed

- `config.toml` is read as TOML 1.1, so its strings can also use `\e` and `\xHH` escapes.
- A signup's `name:` has to be one plain name of at most 16 characters, without
  placeholders. Those worked until generation, but the host then didn't get their slot.
- Slot names ignore case when checking they're free, as Archipelago does.
- The data directory is kept to Bifrost's user. A service reading
  `connect_password.request` needs to run as Bifrost's user or root.
- An account that has played can't be deleted, so history stays complete.
- The database runs in write-ahead mode, so `bifrost.db-wal` sits beside `bifrost.db`
  while it runs. Keep the data directory on a local filesystem, and copy the database
  through `bifrost backup` or with Bifrost stopped.
- A season's apworlds are locked once it's generated.
- If you use forward auth, add `/seat` and `/common` to your proxy's signed-in paths when you upgrade
  (`/common` replaces `/mc`).
  To reach private seasons' pages, also add `/board`, `/log` and `/season`, then set
  `season_pages = true`. Until you do, nobody can read a private season's pages or any
  season's before generation, sysadmins included, and new seasons are private. Don't set
  it before the paths are covered: those pages would believe identity headers sent by
  anyone. See the user guide.
- Connection details moved from the home page to each season's live board, with its own
  address and, for its players, moderators and sysadmins, its connect password. The instance's
  password note setting is gone.
- Players no longer claim or unclaim slots. Moderators assign slots in seasons with no
  signups. A signup can't be removed once its season is generated, and nobody is removed
  from a seat after that.

### Removed

- The built-in Minecraft common game, its `[minecraft] address` setting and
  `BIFROST_MC_ADDR`, and `mc-whitelist.json`. Set it up as a common game instead, from
  `docs/examples/common/minecraft-26.2`: its address goes in its `game.toml` or on the
  season console, and its allowlist is downloaded from the season console. A
  `config.toml` that still has `[minecraft]` is refused, with a note saying so.
- The season buttons for clearing players, the save or the world, and the full reset:
  they could leave a running season broken. Cancel generation and deleting replace them.
- Upgrading from 0.2 or earlier. Seasons, games, slots and seats now have generated ids,
  in URLs and in the database and data directory, so old URLs stop working and older data
  can't be carried over: start with an empty data directory. This is a one-time break;
  from 0.3.0 upgrades keep working.

## [0.2.0] - 2026-10-29

Upgrading from 0.1.x:

- Install 0.2.0 by hand: `bifrost update` in 0.1.x can't check the new
  signatures (user guide, Updating Bifrost).
- The first start backs up the database and then upgrades it, so 0.1.x can no
  longer open it. To go back, restore that pre-upgrade backup before starting
  0.1.x (user guide, Going back); anything done on 0.2.0 since is lost.
- Over HTTPS, everyone signs in once more (the session cookie is renamed).

### Added

- `bind` (`BIFROST_BIND`) sets the address Bifrost listens on, for example
  `127.0.0.1` behind a reverse proxy on the same host. The user guide explains
  when that matters for forward-auth.

### Changed

- Seasonal minesweeper earns a limited number of bonus hints per player in
  any 24 hours: five unless the season's moderators change it on the
  season's Options tab (0 means no limit). New boards are rate-limited.

### Fixed

- Clearing a minesweeper board says whether it earned a bonus hint: free play
  and wins past the daily limit no longer claim one.

### Security

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
- Releases' `SHA256SUMS` are signed, and `bifrost update` only installs a
  release whose signature verifies against the key built into Bifrost, so a
  file swapped into a release is refused. Releases before signing started
  can't be installed with `bifrost update`. Since `bifrost update` in 0.1.0 to
  0.1.2 doesn't check signatures, install this release by hand and verify it
  (user guide, Updating Bifrost).

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

[Unreleased]: https://github.com/vidarrio/bifrost/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/vidarrio/bifrost/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/vidarrio/bifrost/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/vidarrio/bifrost/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/vidarrio/bifrost/releases/tag/v0.1.0
