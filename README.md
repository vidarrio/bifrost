# Bifrost

The hub for all things Archipelago — the bridge between the worlds. A
self-hosted, kanidm-authed season manager for a friend-group
[Archipelago](https://archipelago.gg) multiworld, written in Rust (axum).

Bifrost runs **alongside** the Archipelago MultiServer on **the Bifrost host**, fronted by
Traefik at **bifrost.example.org**. It is a *client* of the AP server (over its
WebSocket) and an orchestrator of generation on disk — it never replaces the
MultiServer, which keeps running standalone at `ap.example.org:38281`.

Built for our stack (see the sibling services: kanidm and a Discord bot), but
the design is general.

## What it does

- **Hub / landing** — how to connect, who's playing what, the relevant Discords,
  and tracker links (Universal Tracker, PopTracker).
- **YAML checker** (`/check`, public) — validates an options file against the
  worlds installed on the server by shelling out to the AP venv's own
  `roll_settings`, so it catches version drift and bad option values for real.
- **Season signup** (`/signup`, gated) — a stepped wizard: pick core vs. custom;
  for custom, upload the `.apworld` (installed into `custom_worlds/` immediately
  so the checker can validate against it); upload and live-check the YAML; add
  co-op names; submit.
- **Claiming** — one slot per player per season. A slot is claimed by an SSO
  identity; co-op games have several Bifrost seats mapped to one AP slot.
- **Live board** (`/board`, public) — a persistent WebSocket (`Tracker` client)
  to the MultiServer showing each slot's status, goal, item flow, prioritized
  hints, and a rolling event feed (item sends, goals, DeathLinks). Item and
  location names are resolved from the seed's data package.
- **Per-game pages** (`/game/{ap_slot}`, gated) — claim/unclaim, a hint box with
  live fuzzy item search that spends the slot's points, and every hint in that
  world with a "prioritize" toggle (prioritized ones surface on the board).
  Hinting is claimant-only.
- **Admin panel** (`/admin`, admin-gated) —
  - **Generate** a season from the signup queue (stages YAMLs → rolls a seed →
    maps slots + auto-claims → deploys → closes signups).
  - **Regenerate** the current lineup (reuse staged YAMLs + apworlds, new world +
    fresh save, applying the current `host.yaml`, carrying claims over by name).
  - Open/close **signups**; enable/disable the **minesweeper** minigame.
  - Manage the deployed **players** (signups) and installed **apworlds**, with
    per-item delete + clear-all; **clear save**, **clear world**, and a
    **full reset**.
  - Set the **connect password** (live via `/option`, persisted to `host.yaml`).
  - A **server console** that runs commands as admin (`/send`, `/hint`,
    `/players`, …) over a short-lived `!admin` connection, with command / player
    / item autocomplete.
- **Minesweeper minigame** (`/minesweeper`, gated) — clear a board to earn a
  bonus hint. Boards are **guaranteed solvable without guessing** (a logic solver
  vets every board before it's served) and the game is server-authoritative. A
  bonus hint is redeemed automatically when you hint without enough AP points
  (Bifrost places it as admin, which bypasses the point cost).

## Auth

kanidm SSO via oauth2-proxy forward-auth on Traefik. The board and checker are
public; signup / claim / hint / game / minesweeper require the **`archipelago_users`**
group; `/admin/*` requires **`archipelago_admins`**. Identity arrives as
`X-Auth-Request-Preferred-Username` / `X-Auth-Request-Groups` headers, which only
Traefik can set. The header shows who you're signed in as and (for admins) an
`admin` pill.

## How it talks to Archipelago

- A single persistent `Tracker`-tagged WebSocket for the live board: reads
  `Connected` for the roster and subscribes to each slot's
  `_read_client_status_*` / `_read_hints_*`, folding `PrintJSON` + `Bounced`
  events into the feed.
- Hints and point lookups open a short-lived connection **as the target slot**
  (so a hint spends that slot's points).
- Admin commands open a short-lived connection and use `!admin login <server_password>`
  then `!admin <command>`; output returns as `AdminCommandResult`.
- Season metadata (per-slot totals, item/location name maps) is decoded from the
  seed via a Python helper and cached.

Data model: an **AP slot** is one YAML/game/connection; a **Bifrost slot** is one
person (claimable, tied to an SSO) mapping to an AP slot. A **season** is keyed by
the AP `seed_name`, so a new seed gets a fresh set of slots.

## Reuse, don't rebuild

- **Logic reachability** → [Universal Tracker](https://github.com/FarisTheAncient/Archipelago).
- **Per-game maps** → [PopTracker](https://github.com/black-sliver/PopTracker).
- **YAML validation / seed decoding / generation** → the AP install's own code in
  the venv on the Bifrost host (via the `scripts/` helpers).

## Layout

```
src/
  main.rs      routes, handlers, AppState
  ap.rs        tracker client + live board state + hint/points/admin connections
  meta.rs      season metadata (decoded seed: totals + name maps)
  checker.rs   YAML checker (spawns the AP venv)
  generate.rs  generate / regenerate orchestration + deploy + clears
  db.rs        SQLite: slots, claims, submissions, settings, hint credits, priorities
  mine.rs      minesweeper engine (no-guess board generator + solver)
  render.rs    all HTML
static/index.html   landing page (include_str!)
scripts/       Python helpers run under the AP venv:
  bifrost_check.py     validate a YAML
  bifrost_meta.py      decode a seed's metadata
  bifrost_generate.py  run Generate.py (pins local/user paths for custom_worlds)
```

## Config (environment)

| var | purpose |
| --- | --- |
| `BIFROST_PORT` | listen port (default 8080) |
| `BIFROST_DB` | SQLite path (default `/var/lib/bifrost/bifrost.db`) |
| `BIFROST_SLOT` | slot name the tracker connects as (enables the live board) |
| `BIFROST_AP_URL` | MultiServer WebSocket (default `ws://127.0.0.1:38281`) |
| `BIFROST_AP_PASSWORD` | server **connect** password |
| `BIFROST_AP_ADMIN_PASSWORD` | server **admin** password (`server_password` in host.yaml) |
| `BIFROST_COOP` | `Name:CoopSeat,…` — extra co-op seats to mint when seeding |

## Sandbox & privileges

`bifrost.service` runs as an unprivileged `bifrost` user with `ProtectSystem=strict`;
it has `ReadWritePaths` for only `Players/`, `custom_worlds/`, `output/`, `logs/`
(and `/var/lib/bifrost`). It never needs root:

- Deploying `output/current.zip` fires a root `archipelago-reload.path` unit that
  restarts the game.
- Changing the connect password writes a request file that a root
  `bifrost-connpw.path` unit applies to `host.yaml` + the service env.

> Note: `host.yaml` `server_options` (passwords, `hint_cost`, `item_cheat`, …) are
> baked into each seed's multidata at generation and restored from the savefile,
> so editing `host.yaml` only affects the **next** seed — change the running
> season with `/option` (the admin console and the connect-password setter do this
> for you).

## Build & deploy

Rust/axum (edition 2024). Built on the work box and shipped as a
stripped release binary to the Bifrost host — the binary is the whole deploy; the `scripts/`
helpers ship alongside it.

```
# edit → commit → push to GitHub main → deploy
the deploy script --yes
```

the deploy script builds `origin/main` from GitHub in a deploy checkout on the work box,
copies the binary to the Bifrost host, ships the helper scripts, restarts the service,
and health-checks it. `cargo test mine` verifies the no-guess board generator.
