# Bifrost

The hub for all things Archipelago — the bridge between the worlds.

Runs alongside the Archipelago MultiServer on **the Bifrost host**, fronted by Traefik at
**bifrost.example.org**. It is a *client* of the AP server (over its WebSocket), never a
host: the game keeps running on the standalone MultiServer (`ap.example.org:38281`).

## What it does (phased)

- **Phase 0 — shell** *(current)*: landing page + `/health`.
- **Phase 1 — hub**: welcome page with links (official games, unofficial apworlds
  sheet, the relevant Discords), current-season roster + connect info, per-player
  tracker links.
- **Phase 2 — YAML checker**: validate an options file against the installed
  worlds by shelling out to the local AP venv (reusing AP's own checker).
- **Phase 3 — live board**: a WebSocket client to the MultiServer showing
  per-slot checks / items / hints / goals / DeathLink, live.
- **Phase 4 — identity + the Discord bot**: kanidm SSO, Discord identity via the Discord bot's
  `/link`, and AP events relayed to Discord through the Discord bot.

## Reuse, don't rebuild

- **Logic reachability** → [Universal Tracker](https://github.com/FarisTheAncient/Archipelago).
- **Per-game maps** → [PopTracker](https://github.com/black-sliver/PopTracker).
- **YAML validation** → the AP install's own checker in the venv on the Bifrost host.

## Build & deploy

Rust/axum, built on the work box and shipped as a release binary to
the Bifrost host, like its sibling services. Deploy with the deploy script.
