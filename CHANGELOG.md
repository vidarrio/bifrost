# Changelog

All notable changes to Bifrost are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0] - 2026-11-24

**Upgrading:** the database gains two tables (notifications, and the moderator
log). The first start of 0.6.0 takes a pre-upgrade backup, then migrates; to go
back to 0.5.x, restore that backup (see Updating Bifrost in the user guide). As
with 0.5.0, it won't start if that backup can't be written, so make room or set
`BIFROST_BACKUP_DIR` first. A newer version of a game a season already has can
now replace the season's copy before generation, once a season moderator
approves it, even on servers that approve uploads automatically. Those servers
also hold for a moderator any upload whose manifest names no game, and every
upload to a season one of whose custom worlds names no game.

### Added

- A **moderator log** for sysadmins, the console's *Moderator log* tab. It
  records what moderators and sysadmins do in season and instance
  administration (console commands included) and with a moderator's powers
  on players' pages: who, the season, the form's fields (secrets left out)
  and the outcome. Filterable by moderator, season and action, in numbered
  pages sized to fit the window; kept for a year.
- A **mailbox** in the header: an envelope that glows while you have unread
  notifications, opening a box of your recent ones. You're told when someone
  prioritizes a hint for an item in your world, when your signup needs
  attention (a failed check, or its world rejected), and when a moderator
  approves or rejects a world you uploaded.
  Moderators hear about their seasons' waiting uploads, signups that need
  attention, generation results and game servers that stop unexpectedly or
  won't start;
  sysadmins about catalog offers, new Bifrost releases, failed nightly
  backups and failed Archipelago installs.
- When the catalog has a newer version of a season's installed world, the
  season admin page offers **update to the catalog's** version until the
  season is generated. It replaces the world and checks every signup for the
  game again.
- A sysadmin approving an upload in a season can add it to the catalog in the
  same step (the approve button's box, ticked by default). A season
  moderator's approval offers the world to the sysadmins instead, on the
  catalog page, with who approved it.
- The signup wizard offers a **starting YAML** for the game you pick:
  Archipelago's own template, with every option, its default and what it
  does, from the season's Archipelago version. It's written on first use and
  kept, for core games and installed or vetted custom worlds (a custom
  world's is removed once it's in neither the catalog nor any season).

### Changed

- Prioritizing a hint is limited to 30 times in 10 minutes per player, and a
  hint the live board doesn't have can't be prioritized.
- The native bundle's Python is a newer build of Python 3.13.18
  (python-build-standalone 20261123, with libffi 3.8.1).
- The signup wizard's first step is one searchable list of every game the
  season can play, each with the version the server has: core games, the
  catalog's custom games and custom games already in the season, plus an
  entry to upload another custom world. It replaces choosing between core,
  catalog and upload first.
- A newer version of a custom game a season already has now replaces the
  season's copy, until the season is generated, instead of the season keeping
  its first version. If the upload is the catalog's own version, or the
  catalog has the game at least as new, the catalog's version is installed at
  once. Otherwise the upload waits for a moderator, even where uploads are
  auto-approved. An upload whose version can't be compared with the
  season's copy waits too, and a moderator can **replace anyway**. Every
  signup for that game is then checked against the new version, and those
  that fail are flagged as held signups are. If it's rejected, the signups
  held for it are checked against the season's copy instead.
- Where uploads are auto-approved, an upload that might be a game the season
  already has waits for a moderator too: one whose manifest names no game, or
  any upload while one of the season's worlds names no game. The uploader and
  the moderators are told why.

## [0.5.0] - 2026-11-23

**Upgrading:** the database gains a column (catalog entries' public notes). The first
start of 0.5.0 takes a pre-upgrade backup, then migrates; to go back to 0.4.x, restore
that backup (see Updating Bifrost in the user guide). Unlike earlier upgrades, 0.5.0
won't start if that backup can't be written, for example when the backup directory is
short of space, so make room or set `BIFROST_BACKUP_DIR` first. The catalog now holds
one version of each game: if yours has several versions of a game, remove the older
entries. The home page now shows the new **Site address** next to its title instead of
the connect host: set it under *Console → Settings*, or set `server.public_url`.

### Added

- Catalog entries have a **public note**, shown to players beside the game on
  the Games page, next to the admins' own note, which stays private. Both can
  be edited on the catalog page. A replacement for a game keeps its public
  note unless it brings its own.
- Sysadmins can replace a catalog game with a newer version a player
  uploaded: it's listed on the catalog page, with its file to download and
  vet, to replace the catalog's version or dismiss.
- A **Games** page (`/worlds`, linked from the home page and the signup
  wizard) lists what can be played on the server before you sign up: the core
  games of the Archipelago version new seasons use, with each world's own
  version where it declares one, the custom games the admins have vetted,
  with their versions, and the common games the server hosts. It's
  as public as the home page.
- A **Site address** setting: Bifrost's own address, shown next to the title
  on the home page.

### Changed

- Uploading a game the server already has uses the server's version: the
  season's own copy if it has one (a season keeps one world per game), else
  the catalog's, and a core game needs nothing installed (Archipelago ignored
  such an upload anyway). The catalog now holds one version of each game:
  adding a newer one replaces its entry, one that isn't newer is refused until
  the entry is removed, and seasons that installed the old one keep it.
- The home page no longer shows the connect host next to its title. That's
  the address for players' Archipelago clients, and season boards still show
  it. The home page shows the new site address there instead. Until one is
  set, it shows the address in `server.public_url` (`BIFROST_PUBLIC_URL`) if
  that's configured, or nothing.
- Plainer wording across pages, notices, error messages, logs and command
  output, without em-dashes.

### Fixed

- Pages fit a phone's screen. The header's sign-in line gets a row of its own
  instead of wrapping inside the bar. Tab bars become an even grid of buttons
  with every tab in sight. The console's tables become a card per row, each
  field labelled and its actions underneath, instead of breaking names and
  buttons mid-word, and the season list no longer widens the page. A standard
  minesweeper board fits the screen. Console buttons, settings labels, the
  board's join details and the log filter's dates no longer wrap or misalign.
  An archived season's minesweeper leaderboards are styled like the live ones,
  at any width.
- The page header no longer jumps when moving between pages. It's drawn from
  a copy saved by the previous page, and that copy now includes the home pill,
  so the pill no longer turns up a moment late and pushes the sign-in text and
  the admin pill aside. Signing in or out drops that copy, so the next page
  no longer shows the previous sign-in state, or a signed-out user's name,
  until it catches up.

## [0.4.3] - 2026-11-22

### Fixed

- On a season's signup page, the "Upload apworld", "Use this world" and
  "Check it" buttons sit on the same row as the file or world they act on, as
  tall as it, instead of alone on a line below it. On a phone they take a
  full-width line under it.

## [0.4.2] - 2026-11-21

### Added

- A **Home pill label** setting on the settings page, next to the home link:
  what the header's home pill says.

### Changed

- The header's home pill says HOME unless the label is set, with no arrow,
  instead of the name of the site it links to.

### Fixed

- The header's home pill no longer sits slightly lower than the rest of the
  header on pages other than the home page.

### Security

- The native bundle's Python is a newer build of Python 3.13.18
  (python-build-standalone 20261110). It brings zlib 1.4, which fixes several
  memory-safety bugs, including a buffer overflow in `gzwrite()`
  (CVE-2026-85091) and a write past the end of a buffer in
  `inflateGetDictionary()`. It also brings expat 2.10.0, which fixes four
  vulnerabilities in XML parsing (CVE-2026-113351, CVE-2026-115506,
  CVE-2026-117610, CVE-2026-118370). The container image uses Debian's Python
  and libraries instead: Debian trixie has fixed CVE-2026-85091, but not yet
  the expat ones.

## [0.4.1] - 2026-11-11

### Changed

- An archived season's page puts its final standings, its minesweeper leaderboards and
  its activity log on tabs, instead of one long page. The activity log's pages and
  filter stay on its tab.

### Fixed

- Switching tabs on a game's page or the season console changed the address to the
  season's own root, losing the page and its query, so reloading or sharing it led to
  the season's board instead. The address now keeps the page. On an archived season's
  page this also stopped the activity log fitting its rows to the window when its tab
  opened.

## [0.4.0] - 2026-11-11

### Added

- A demo build (`cargo build --features demo`) for a public demo: a bar switches between
  a player, a moderator and a sysadmin account without passwords, with a note pointing
  to it until a visitor picks one or closes it. Since every visitor can be sysadmin,
  it turns off what could run code, host files or change the instance, caps seasons,
  games and live seasons, keeps the seasons it starts with as they are, keeps game
  servers on localhost, and resets from a snapshot. Its seed fills it with seasons of
  real games, some finished and some part played, with hints (a couple starred as
  priorities), and minesweeper leaderboards.
  Release builds don't include it. Every push to `main` publishes it as the
  `ghcr.io/vidarrio/bifrost:demo` image. See `packaging/demo/README.md`.
- The home page says what Archipelago and Bifrost are under its title, with the four
  steps of a season: sign up, generate, play, follow along.
- Bifrost has a logo: six islands joined by bridges. It replaces the gold caret as the
  favicon, shows beside the name in the page header and on the home page. The home
  page's title and the live board's progress bars fade through its colour.

### Changed

- The home page lists the AP After Dark Discord, for unrated and 18+ games, next to the
  official Archipelago one, unless the instance has saved its own list in Settings. An
  instance that has keeps its list; add it there.
- The home page lists your seasons instead of showcasing one: every season you can see
  that isn't archived (drafts only for those who run them), public ones and private
  ones you're in, live ones first, each
  with its state, its games and players, and your own games in it. The "Join the
  current game" section and the "Live board" card are gone.
- How to join a season is a tab on its live board, next to the board itself: its
  address, its password for those who may see it, the Archipelago version its server
  runs, which can differ between seasons, and your games in it. The board's heading is
  the season's name.
- A game's figures, on its card on the live board and at the top of its page, give how
  many locations are left right after how many there are, before what was sent and
  received.

### Removed

- The standalone YAML checker (`/check`) and the home page's Tools section that linked
  it. Signing up checks a YAML against the season's own worlds before anything is saved,
  along with its slot name, so it covers what the checker did. The checker could also
  check against the wrong season, and anyone could run it without signing in.

### Fixed

- In the activity log's sender and receiver lists, the highlighted name scrolls into
  view. Before, the last names of a long list could be highlighted out of sight.
- The apworlds a season gets (custom worlds, the common games' and Universal Tracker's
  on every roll) and a season's `host.yaml` copied from the instance's own are put in
  place in one step, so a signup check or a generation reading one at that moment
  can't see part of it. Every file Bifrost replaces this way is now on disk before it
  takes the old one's place, so a crash or power cut leaves the old file or the new
  one, not an empty one.
- A co-op partner's name that matched a player's except for capital letters ("ann" and
  "Ann") passed the check at generation. The partner got no seat of their own and could
  take that player's game instead. Generation now refuses it, as it does an exact match.
- A re-roll or a generation that failed while it set up the season's games could leave
  the season with some games missing or their players unassigned. A re-roll also moved
  the season to the new seed's name and cleared its activity and starred hints while
  the old game kept running. The games are now set up all at once or not at all, and
  the new seed only becomes the season's once they are.
- If a season's game server passwords can't be read, its server doesn't start. Before,
  it started without a password.
- When a game's file couldn't be downloaded (not your game, or no such file), the
  notice's "back" link went to that game in the default season instead of the game's
  own season.
- File pickers look like the fields around them on every page. Before, the console's
  uploads, season import and replacing a signup's YAML showed the browser's plain
  "Choose File" button.
- Date pickers, dropdown lists and radio buttons follow the dark theme.
- The console's one-line forms line their buttons up with their fields, and the sign-in,
  setup, invite and reset forms' buttons match their fields' size.

## [0.3.0] - 2026-11-05

**Upgrading:** data from 0.2 or earlier can't be carried over, so start with an empty
data directory. From 0.3.0 on, upgrades keep working. Bifrost no longer runs as root
unless told to (see Security). If you use forward auth, your proxy needs new paths (see
Changed), and a `config.toml` with `[minecraft]` has to lose it (see Removed).

### Added

- Common games: co-op games a sysadmin hosts, such as a Minecraft, Factorio or Terraria
  server, that a season includes as one slot everyone plays. Sysadmins keep a catalog of
  setups under Console → Common games, or as folders under `common/` in the data
  directory: each one's YAML, any apworld, a join guide in Markdown and client files to
  download. A season can include several, two setups of one game among them, each with
  its own address. Players register the name they play under once per kind of game, and
  moderators download each common game's allowlist as text or JSON. `docs/examples/common`
  has a Minecraft setup to start from.
- A read-only export token for common games, made on the settings page, so a sysadmin's
  own script can fetch every live season's common games, allowlists and seed files from
  `/export/...` with `curl`. `docs/examples/common/sync` shows two ways
  to write one: copying the allowlist into place, and applying it over RCON.
- Private seasons. A new season's board, log and game pages are for its players,
  moderators and sysadmins, and a moderator can open them to anyone. Signups can also be
  made private, reachable only through a link. Non-members don't see a private season on
  the home page or in past seasons. Imported seasons stay open after
  generation, and before generation every season's pages need a sign-in.
- Signups, reworked:
  - The home page lists the seasons open for signup.
  - Each game has its own page, with notes for the other players, and its host can
    replace the YAML without losing the signup. Moderators can edit notes and, until
    generation, replace YAMLs.
  - A player can sign up with several games (*Add another game*), up to a limit
    moderators can set (0, the default, means no limit). Each YAML holds one player.
  - A YAML for a world awaiting approval is checked as far as it can be without running
    the world, and can be replaced while it waits. A signup that fails its check says
    so on the season's page and to moderators, who can replace its YAML or dismiss it.
    The approval screen lists who's waiting, and generation names the signups it's held
    up by.
- Co-op partners join through one-use invite links from a game's host, before or after
  generation, and pick their own seat name. They can edit the game's notes and, before
  generation, rename their seat or leave. Signing up takes you to your game's page.
- Moderators can delete a season before generation and cancel a live one back to
  signups, keeping its signups, notes and partners. Sysadmins can delete an archived
  season. Each asks you to type the season's name.
- PopTracker packs read their slot's hints and Archipelago's data storage from the
  season's save, as often as the server saves (every minute by default), so packs that
  track hints or a game's own progress show them, and hinted locations are outlined on
  the map. Watches on item codes run when those items change. Items draw with their
  image filters, `disabled_img` and overlays, and maps draw trapezoid, triangle and
  kite markers. Nothing a pack does can write to the server.
- The home page lists a signed-in player's games in live seasons, each linking to its
  page.
- The home page's minesweeper is for any account. A season's own is for its players and
  those running it.
- The README and user guide warn that Bifrost is in very early development: read the
  changelog before updating.

### Security

- Bifrost refuses to run as root, commands included, unless `BIFROST_ALLOW_ROOT=1` is set
  in the environment. If you run it as root, switch to its own user and `chown` its data
  directory first. Run as root, it won't start if another user could change the data
  directory, anything in it, a folder above it or a link on the way to it.
- Unpacking a bundle no longer writes through a link left at the paths it creates.
- PopTracker packs' Lua runs in a sandboxed process of its own, stopped after 30 seconds
  (`pack_timeout_secs`). Each slot computes its tracker once at a time, at most four run
  at once, and uploads are rate-limited. A pack that hits a limit waits a minute, then
  longer each time, or until it's replaced, and a slow pack's tracker updates less
  often.
- When `config.toml` doesn't parse, the error gives the line and column but no longer
  quotes the line, which could hold a secret.

### Fixed

- A game's status on the board follows its players, not Bifrost. Bifrost's own short
  connections no longer make a game flash "connected", and the game its tracker joins as
  shows a real player connecting and clears when they leave, instead of staying
  "playing". A server killed rather than stopped no longer keeps a status other than a
  goal after it restarts. This needs Archipelago 0.6.8-bifrost.4, now the bundled build;
  on older builds and external servers, statuses stay as they were.
- A game page keeps itself current while it's open, as the board does: its figures
  (locations, sent, received, left), its hints list, and its Universal Tracker tab with the
  "available" count at the top.
- Many more PopTracker packs load and track as they do in PopTracker:
  - Bifrost picks a pack's Archipelago variant, finds files in its folder, loads scripts
    with `require` (from the pack only), and supplies more of what packs call, such as
    overlays, `os.time`, frame handlers and change watches.
  - Packs get the slot's slot data, the seed name and the other players' names and
    games, as from Archipelago on connect, so packs that read the seed's options track
    them (Satisfactory's starting recipes, for one).
  - Access rules: `{}` inspect rules, `^$` levels, `[optional]` and
    `@location/section` references (undrawn locations too), section `ref`, locations of
    the same name, nested rules, `$name|arg` arguments, and how a location's rules join
    its parent's. Large logic packs that never finished computing now do.
  - Items: starting stages, `allow_disabled`, `inherit_codes`, `static` and
    `composite_toggle`, several items on one code, custom items in rules, sections that
    host an item, and counts and stages kept in their limits. Locations honour `parent`,
    and map markers their visibility rules.
  - An error in one of a pack's handlers no longer blanks its tracker (the tab says
    tracking may be incomplete), and packs that
    also read an emulator's memory, such as Super Metroid's, keep their received items.
- A game's PopTracker pack stays with it through a re-roll.
- A data directory moved or restored to another path keeps its signups: the database
  names their YAMLs from inside the data directory, not by absolute path.
- An unknown URL, "Moderators only" and a game page with no game show a page rather than
  the browser's own error. Long links and paths no longer widen the page, a failed
  generation shows as a failure, the apworld approval options are laid out properly, and
  a game page only links to minesweeper when the season has it on.
- A season's Archive tab only offers archiving once the season is live, as it couldn't be
  done before then.

### Changed

- If you use forward auth, add `/seat` and `/common` to your proxy's signed-in paths
  (`/common` replaces `/mc`). To reach private seasons' pages, also add `/board`, `/log`
  and `/season`, then set `season_pages = true`. Until you do, nobody can read a private
  season's pages or any season's before generation, sysadmins included. Don't set it
  before the paths are covered: those pages would believe identity headers sent by
  anyone. See the user guide.
- Connection details moved from the home page to each season's live board, with its own
  address and, for its players, moderators and sysadmins, its connect password. The
  instance's password note setting is gone.
- Players no longer claim or unclaim slots: a signup's host has their slot, partners
  join by invite, and moderators assign slots in seasons with no signups. A signup can't
  be removed once its season is generated, and nobody is removed from a seat after that.
- A signup's `name:` has to be one plain name of at most 16 characters, without
  placeholders, and slot names ignore case when checking they're free, as Archipelago
  does.
- A season's apworlds are locked once it's generated.
- An account that has played can't be deleted, so history stays complete.
- The data directory is kept to Bifrost's user. A service reading
  `connect_password.request` needs to run as Bifrost's user or root.
- The database runs in write-ahead mode, so `bifrost.db-wal` sits beside `bifrost.db`
  while it runs. Keep the data directory on a local filesystem, and copy the database
  through `bifrost backup` or with Bifrost stopped.
- `config.toml` is read as TOML 1.1, so its strings can also use `\e` and `\xHH`
  escapes.
- The header's link to the operator's site is named after that site (`example.org ↗`),
  not "home", which read as Bifrost's own home page.
- A season console's Overview shows only what applies to the season's stage: signups
  open and close before generation, and a live season offers only its re-roll.

### Removed

- Upgrading from 0.2 or earlier. Seasons, games, slots and seats now have generated ids,
  in URLs, the database and the data directory, so old URLs stop working and older data
  can't be carried over. This is a one-time break.
- The built-in Minecraft common game, its `[minecraft] address` setting,
  `BIFROST_MC_ADDR` and `mc-whitelist.json`. Set it up as a common game instead, from
  `docs/examples/common/minecraft-26.2`: its address goes in its `game.toml` or on the
  season console, and its allowlist comes from the season console or the export. A
  `config.toml` that still has `[minecraft]` is refused, saying so.
- The season buttons for clearing players, the save or the world, and the full reset,
  which could leave a running season broken. Cancelling generation and deleting replace
  them.

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

[Unreleased]: https://github.com/vidarrio/bifrost/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/vidarrio/bifrost/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/vidarrio/bifrost/compare/v0.4.3...v0.5.0
[0.4.3]: https://github.com/vidarrio/bifrost/compare/v0.4.2...v0.4.3
[0.4.2]: https://github.com/vidarrio/bifrost/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/vidarrio/bifrost/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/vidarrio/bifrost/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/vidarrio/bifrost/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/vidarrio/bifrost/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/vidarrio/bifrost/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/vidarrio/bifrost/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/vidarrio/bifrost/releases/tag/v0.1.0
