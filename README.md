# Bifrost

A self-hosted season manager for [Archipelago](https://archipelago.gg)
multiworlds. It's built for a friend group or community running recurring
seasons. One program runs the whole thing: signups with live YAML checks, seed
generation, the Archipelago servers, a live board with hints and trackers, and
an archive of past seasons.

- **[User guide](docs/USER_GUIDE.md)**: how to play, run seasons and operate an
  instance.
- **[AI disclaimer](AI_DISCLAIMER.md)**: Bifrost is largely AI-written. Read
  this before you deploy it.

## Features

- **Seasons.** Each season goes draft → signups → live → archived, and several
  can be live at once. Every live season gets its own Archipelago server on a
  stable port.
- **Signups.** A three-step wizard: pick a game (core, a vetted catalog world,
  or a custom upload), then upload a YAML. The YAML must pass Archipelago's own
  checks before the signup is accepted.
- **Custom worlds, safely.** Sysadmins keep a catalog of vetted apworlds. Other
  uploads can wait for a moderator's approval. Every world is hash-pinned, and
  all apworld code runs in a bubblewrap sandbox.
- **Generation.** One click rolls the seed, maps slots and claims, and starts
  the server. Re-rolls keep claims.
- **Live board and game pages.** Slot status, goals and item flow, hints with
  item search and prioritization, Universal Tracker's in-logic list,
  PopTracker maps in the browser, and minesweeper for bonus hints.
- **Roles.** Players, moderators who run their own seasons, and sysadmins who
  run the instance. Accounts are invite-only with built-in passwords, or come
  from your identity provider through forward-auth.
- **Archipelago updates.** Install new releases from the settings page; each is
  tested before it's offered. A live season keeps the version it was generated
  with. Releases come from Bifrost's [Archipelago fork](packaging/archipelago/),
  which carries small compatibility patches.
- **Operations.** Nightly backups with retention, live load monitoring, and
  season bundles you can download and import on another instance.

## Quick start

```sh
podman build -t bifrost .            # or docker build
podman run -d --name bifrost --stop-timeout 30 --security-opt unmask=/proc/* \
    -p 8080:8080 -p 38281-38300:38281-38300 -v bifrost-data:/var/lib/bifrost bifrost
podman logs bifrost                  # prints the first-run /setup link
```

Open the `/setup?token=…` link to create the first sysadmin. Then set the
connect host under *Console → Settings* and create a season.

You can also build Bifrost as a single native Linux executable
(`packaging/bundle/build.sh`). See the
[user guide](docs/USER_GUIDE.md#installing) for both options, what the
container flags do, and running behind a reverse proxy.

## Development

Bifrost is written in Rust (axum, SQLite via sqlx). Its Python helpers run
under Archipelago's own interpreter.

```sh
cargo test --workspace               # unit tests
cargo build && BIFROST_DATA=./data BIFROST_AP_DIR=/path/to/Archipelago \
    BIFROST_RUNTIME_DIR=. BIFROST_INSECURE_COOKIES=1 target/debug/bifrost
```

`BIFROST_AP_DIR` must point at an Archipelago checkout with a `venv/`
(`BIFROST_AP_PYTHON` overrides the interpreter). `bifrost config example`
prints every setting.

```
src/
  main.rs           routes, handlers, startup
  config.rs         settings and the data-dir layout
  db.rs, db/        SQLite: seasons, accounts, slots, claims, signups, catalog, settings
  auth.rs           sessions, roles, forward-auth, CSRF (account.rs: sign-in, invites, users)
  scope.rs          season-scoped URLs (/s/<id>/…)
  runtime.rs        per-season boards and workers (apserver.rs: the AP server supervisor)
  ap.rs             Archipelago client and live board state
  ap_adapter.rs     the one door to Archipelago internals (sandboxed helper scripts)
  sandbox.rs        bubblewrap / resource-limit isolation
  apinstall.rs      which AP install each season runs (apupdate.rs: installing releases)
  worlds.rs         apworld catalog, uploads and approval
  checker.rs        YAML checks
  generate.rs       generation and re-rolls
  inlogic.rs        Universal Tracker's in-logic list
  ptrack.rs         PopTracker packs (engine in crates/poptracker)
  mine.rs           no-guess minesweeper
  season.rs         archived-season summaries (archive.rs: season bundles and import)
  backup.rs         backups and retention (bundle.rs: the single-file runtime)
  load.rs           live load sampling and warnings
  render.rs         all HTML
crates/poptracker/  PopTracker pack engine
scripts/            Python helpers run under Archipelago's interpreter
static/             the home page template
vendor/             bundled apworlds (Universal Tracker)
packaging/          the native bundle build and the Archipelago fork's patches
docs/               the user guide
```

## License

MIT; see [LICENSE](LICENSE).
