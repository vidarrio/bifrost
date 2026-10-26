# Bifrost

A self-hosted season manager for [Archipelago](https://archipelago.gg)
multiworlds, written in Rust (axum). One download runs the whole thing:
signups with live YAML checks, seed generation, the AP servers themselves (one
per live season), a live board with hints, and the archive of past seasons.

It's built for a friend group or community running recurring seasons, and
ships as a container image or a single native file. Everything mutable lives
in one data directory.

## Run it

**Container (recommended)** — the image carries Bifrost, a pinned Archipelago
and bubblewrap:

```sh
podman build -t bifrost .            # or docker build
podman run -d --name bifrost --stop-timeout 30 --security-opt unmask=/proc/* \
    -p 8080:8080 -p 38281-38300:38281-38300 -v bifrost-data:/var/lib/bifrost bifrost
podman logs bifrost                  # the first-run /setup link
```

`--security-opt unmask=/proc/*` (docker: `--security-opt systempaths=unconfined`)
lets the sandbox mount its own `/proc`; without it Bifrost falls back to a
weaker sandbox and refuses custom apworlds (it says so in the log).
`--stop-timeout 30` gives live AP servers time to save.

**Native single file (Linux x86_64)** — a static binary with its own Python
and Archipelago appended; it unpacks them into the data dir on first run:

```sh
packaging/bundle/build.sh            # → dist/bifrost-<version>-x86_64-linux
./dist/bifrost-<version>-x86_64-linux
```

Without `BIFROST_DATA` it keeps its data in `~/.local/share/bifrost` (or
`/var/lib/bifrost` as root). Install `bubblewrap` on the host for the strong
sandbox, which custom apworlds need.

**First run:** open the `/setup?token=…` link from the log to create the first
sysadmin. Then set the connect host and Discord links under *Console →
Settings*, create a season, and open its signups.

## What it does

- **Seasons.** Draft → signups → live → archived, several live at once, each
  with its own AP server on a stable port from `BIFROST_AP_PORTS` (default
  38281–38300; the range size caps how many seasons can be live). Moderators run
  the seasons they create; sysadmins run the instance.
- **Signups.** A stepped wizard: pick a core game, a world from the curated
  catalog, or upload a custom `.apworld`; the YAML is checked against the
  season's worlds with Archipelago's own code before it's accepted.
- **Apworld catalog and approval.** Sysadmins keep a catalog of vetted worlds.
  Other uploads install right away or wait for a moderator's approval,
  depending on a sysadmin toggle; signups that need a pending world are held
  and checked once it's approved.
- **Generation.** Rolls the season from its signups, maps slots and claims, and
  starts (or restarts) the season's AP server. Re-rolls keep claims.
- **Live board and game pages.** Each slot's status, goal and item flow;
  hints with fuzzy item search and prioritization; Universal Tracker's
  "in logic" list; PopTracker packs.
- **Archipelago updates.** Check for releases, install one (tested with a
  sandboxed generation before it's offered), and switch new seasons to it. Live
  seasons keep the version that generated them until they're re-rolled or
  archived.
- **Backups.** Nightly and on demand, with retention (the last 7 days, 4 weeks
  and 3 months, capped at 5 GiB by default), downloadable from the settings
  page.
- **Minesweeper.** Clear a no-guess board to earn a bonus hint.

## Accounts

Usernames and passwords by default, with single-use invites, community invite
links and sysadmin-minted reset links. Alternatively an identity provider via
forward-auth (`bifrost auth mode forward-auth`), exclusive per instance. The
CLI is the break-glass:

```
bifrost auth status                       show the auth mode and accounts
bifrost auth mode native|forward-auth     switch auth mode (restart to apply)
bifrost admin reset-link <username>       mint a 24 h password-reset link
bifrost admin create-sysadmin <username>  create a sysadmin + reset link
bifrost admin set-role <username> <role>  user|moderator|sysadmin
bifrost backup now | list | restore <zip> backups (restore with Bifrost stopped)
```

## Configuration

Settings come from the environment or `$BIFROST_DATA/config.toml` (the
environment wins). `bifrost config example` prints a commented template of
every setting. Page-level settings (connect host, home page, Discords, apworld
approval, Archipelago versions) live in the console instead.

## Isolation

Anything that runs apworld code — YAML checks, generation, seed decoding, the
AP servers — runs sandboxed: under bubblewrap with no network (except the AP
servers and the in-logic tracker), a read-only system, and only the season's
own output writable. Without bubblewrap Bifrost uses resource limits only and
refuses custom apworlds unless the operator explicitly allows them.

## Layout

```
src/
  main.rs         routes, handlers, startup
  config.rs       settings + the data-dir layout
  db.rs, db/      SQLite: seasons, accounts, slots, claims, submissions, catalog, settings
  auth.rs         sessions, roles, forward-auth, CSRF; account.rs: login/invites/users pages
  runtime.rs      per-season boards and workers; apserver.rs: AP server supervisor
  ap_adapter.rs   the one door to Archipelago internals (sandboxed helper scripts)
  apinstall.rs    which AP install each season runs; apupdate.rs: installing releases
  worlds.rs       apworld catalog, uploads and approval
  generate.rs     generate / re-roll orchestration
  sandbox.rs      bubblewrap / rlimit isolation
  backup.rs       backups and retention; bundle.rs: the single-file runtime
  render.rs       all HTML
scripts/          Python helpers run under Archipelago's interpreter
vendor/           bundled apworlds (Universal Tracker)
packaging/bundle/ the native bundle build
docs/design/      design notes
```

## Development

```sh
cargo test                           # unit tests
cargo build && BIFROST_DATA=./data BIFROST_AP_DIR=/path/to/Archipelago \
    BIFROST_RUNTIME_DIR=. BIFROST_INSECURE_COOKIES=1 target/debug/bifrost
```

`BIFROST_AP_DIR` should hold an Archipelago checkout with a `venv/`
(`BIFROST_AP_PYTHON` overrides the interpreter).
