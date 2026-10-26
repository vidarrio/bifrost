# Bifrost → distributable appliance, multi-season, roles

Status: **draft / direction-setting.** Captures the agreed destination and the
architecture to get there. Not a committed implementation plan; sequencing in
§13 is a proposal.

## 1. Vision & scope

Bifrost becomes a **self-hosted appliance**: one download that a group runs for
their own Archipelago community, bundling and fully managing the AP server. One
instance == one community. **Explicitly not a multi-tenant SaaS** (that only
happens if someone pays for it, and it inverts most of the decisions below).

Three things gate "publishable", and one is the headline feature:

- **Decouple identity** — stop assuming the operator runs kanidm + oauth2-proxy.
- **Close the security flaws** — the threat model shifts from "trusted friends"
  toward "whatever an operator's users are", so untrusted-input paths must be
  contained (chiefly: apworld code execution).
- **Bundle + manage AP** — Bifrost owns the AP server lifecycle.
- **Headline:** run **multiple seasons at once**, managed by **season
  moderators**, from a cleaned-up console.

## 2. Goals / non-goals

**Goals**
- Single-community appliance, operator-friendly install.
- Native accounts as the default auth; existing SSO/reverse-proxy mode stays a
  first-class option.
- Multiple concurrent seasons, each independently managed.
- Three roles: **sysadmin**, **moderator** (season mod), **user**.
- Bifrost-managed season lifecycle incl. naming-at-creation and archiving.
- Contain untrusted generation (apworld) code regardless of host OS.

**Non-goals (for now)**
- Multi-tenant SaaS, billing, tenant isolation.
- Rewriting AP in Rust (it's a large Python ecosystem; we supervise it).
- Guaranteeing kernel-level isolation on every OS (we offer honest tiers).

## 3. Deployment & packaging

Offer **two forms**, recommend the first:

1. **Container image (recommended).** `docker run …` (or compose) bundling a
   pinned AP + vendored Python + Bifrost. Gives strong generation isolation for
   free (see §4) and trivially solves the Python-bundling problem. This is the
   "one managed download" story for most operators.
2. **Native self-extracting single file (convenience).** A Bifrost binary with
   an appended payload (vendored Python + AP) that unpacks to a data dir on
   first run. Satisfies "single download" for people who won't run Docker, at
   the cost of per-OS build/maintenance of the embedded runtime and weaker
   isolation off-Linux.

**Container image — as built (`Dockerfile`).** Three stages: Bifrost
(`rust:1.98-trixie`, release build), Archipelago (a pinned build of Bifrost's fork via
`AP_VERSION`, default 0.6.8-bifrost.2; venv with core + every bundled world's
requirements and `setuptools<81`; `_speedups` prebuilt with Cython), and a
`debian:trixie-slim` runtime with python3, bubblewrap, tini, and gcc + python3-dev
(so releases installed from the settings page build their speedups), running as an
unprivileged `bifrost` user. Paths match Bifrost's defaults (`/opt/archipelago`,
`/opt/bifrost/{scripts,vendor}`, data volume `/var/lib/bifrost`); exposes 8080
and the AP port range. Run with `--security-opt unmask=/proc/*` (podman) /
`systempaths=unconfined` (docker) so bubblewrap can mount its own `/proc` —
without it Bifrost drops to the limits tier and says which flag to add — and
`--stop-timeout 30` (live seasons' servers are stopped in parallel, each with a
15 s save grace). Verified rootless under podman 5.4 in an unprivileged LXC:
strong tier active, the managed-server and multi-season end-to-end suites pass
against the container, and stop/start keeps every season's port and save.

**Native bundle — as built (`packaging/bundle/`, `src/bundle.rs`).**
`packaging/bundle/build.sh` produces `dist/bifrost-<version>-x86_64-linux`
(~80 MB): a static musl Bifrost with a zip payload appended, then a trailer
(payload length + magic). The payload holds a relocatable CPython 3.13 from
python-build-standalone (pinned release + SHA-256) with Archipelago's
requirements installed straight into it (no venv, so it stays relocatable;
Kivy/KivyMD left out; `_speedups` prebuilt), the pinned Archipelago source
(tag + tarball SHA-256), Bifrost's `scripts/` + `vendor/`, and a `BUNDLE_ID`
(version + AP version + content hash). On start, a binary with a payload
unpacks it once into `$BIFROST_DATA/runtime/<id>/` (atomic rename; symlinks
must stay inside) and supplies the defaults for `BIFROST_AP_DIR`,
`BIFROST_AP_PYTHON`, `BIFROST_RUNTIME_DIR` and `BIFROST_PYTHON` (explicit
settings still win); without `BIFROST_DATA` it uses `~/.local/share/bifrost`
(`/var/lib/bifrost` as root). A newer bundle unpacks beside the old runtime,
which is removed unless an installed release's venv was built from it.
Isolation is the host's: bubblewrap if installed (strong tier), otherwise
limits only. Verified: first run unpacks, restart doesn't; checks, generation
and a live server run on the bundled Python under bwrap; a release installs
from the settings page with it; and it runs on a bare `debian:bookworm-slim`
with no Python at all. Linux x86_64 only for now.

**AP coupling → one adapter.** Today we touch AP internals directly (pickled
`current.apsave` via `restricted_loads`, `AutoWorldRegister`, helper scripts
pinned to `/srv/archipelago`). Before bundling, isolate *all* AP-internals
access behind a single `ap_adapter` module (savefile decode, world registry,
generate/check invocation, multidata read). AP version bumps then touch one
place; the bundle pins a known-good AP version and Bifrost owns upgrades.

**De-hardcode first.** Every `/srv/archipelago`, venv path, and script location
becomes config-driven, resolved relative to the data dir in §3a. This is the
prerequisite for both portability and bundling; low-risk, do it first.

## 3a. Data layout — one data dir; runtime vs. state

Split **immutable runtime** (shipped in the bundle/image, read-only, replaced on
upgrade) from **mutable state** (everything else, under a single configurable
`$BIFROST_DATA` root — the one thing an operator backs up or mounts as a volume):

- **Runtime (bundle/image):** the Bifrost binary, the pinned AP
  (`BIFROST_AP_DIR`) and its Python (`BIFROST_AP_PYTHON`), and Bifrost's shipped
  files (`BIFROST_RUNTIME_DIR`: the repo's `scripts/` helper scripts and
  `vendor/` bundled apworlds). Not under the data dir.
- **State (`$BIFROST_DATA`):**

```
$BIFROST_DATA/
  config.toml                 # §10 (secrets move here); step 1 stays env-driven
  bifrost.db                  # embedded SQLite — no system dep, no server
  packs/<season>/<ap_slot>/   # poptracker packs
  uploads/                    # submitted player YAMLs
  common/                     # operator-supplied shared-game assets (MC apworld/YAML/client jars)
  archives/<seed>.tar.gz      # raw season bundles
  mc-whitelist.json, connect_password.request
  seasons/<id>/               # an Archipelago *user dir* per season:
    Players/                  # staged per-player YAMLs
    worlds/                   # custom .apworld (per season → isolated)
    output/                   # AP_<seed>.zip, current.zip, current.apsave
    logs/
    host.yaml                 # per-season AP settings (server_options are baked into the seed)
```

The season dir deliberately mirrors AP's own user-dir layout: the helper
scripts pin AP's `user_path` to it (and `local_path` to the install), so AP and
Universal Tracker find the season's YAMLs, custom worlds (`worlds/` — AP scans
`user_path("worlds")` whenever user dir ≠ install dir), output, logs and
`host.yaml` with their own defaults. Implemented in `src/config.rs`
(`Config`, `SeasonDirs`) and `scripts/bifrost_ap.py`. Until seasons are
first-class (§7) the runtime uses a single `seasons/current/`.

Why: one dir to back up / mount; per-season `worlds/`+`output/` give the
data isolation §7/§10 want for free; and the generation sandbox (§4) binds just
one season dir read-write.

**DB choice — keep SQLite.** It's embedded via `sqlx` (bundled libsqlite3; no
system package, no separate DB server), which is what keeps "one download, just
run it" true. A server DB (Postgres…) would only matter for the ruled-out SaaS
path. (Aside: the box lacks the `sqlite3` *CLI*, a manual-inspection convenience
only — the engine is compiled into the binary; inspect via `python3`'s `sqlite3`
module.)

**No live-box compatibility.** The current deployment is **not** migrated in
place: once the running season ends it is retired and Bifrost is redeployed
fresh on the appliance layout (§11). So the single-root layout above is *the*
layout — no legacy profile, no defaults that mimic today's
`/var/lib/bifrost` + `/srv/archipelago/{Players,custom_worlds,output,logs}`
split, no compatibility shims. Today's flat AP-dir state becomes the per-season
`seasons/<id>/…` form. Whatever starts the AP server must point it at the
season's `output/current.zip` (until the §7.2 supervisor owns that, it's the
redeploy's service wiring).

## 4. Generation isolation (`sandbox::spawn`)

Generation, YAML checking, meta decode, and in-logic all **import worlds**, i.e.
run arbitrary apworld Python. All already run as subprocesses, so we insert a
**sandbox launcher** in front of the spawn — no re-architecture. It detects the
strongest available tier at startup, applies it, and **reports the active tier**
to the sysadmin.

Tiers, strongest → weakest:

1. **In-container** (appliance is containerized): subprocess already boxed; add
   a separate uid + seccomp + no-net on top. Strongest, free.
2. **Linux + bubblewrap**: unprivileged userns, read-only binds of only what's
   needed, tmpfs scratch, `--unshare-net`, dropped caps.
3. **Linux, no bwrap**: helper sets `NO_NEW_PRIVS` + seccomp-bpf + `setrlimit` +
   `unshare` net/mount where permitted, then execs Python.
4. **macOS / Windows**: `sandbox-exec` profile / AppContainer + Job Objects.
   Weaker; the thin tier.
5. **Universal floor**: separate process + `setrlimit` (CPU/mem/nproc/fsize) +
   wall-clock timeout + kill + ephemeral workdir. Stops the vast majority of
   real badness even without namespaces.

Design rules:

- **No-network is default-on** for generation at every tier that can enforce it.
  It's the cheapest, most portable, highest-value control (most apworld attacks
  need the network to exfiltrate or pull a second stage).
- **Trusted vs. untrusted split.** Core/official AP worlds run at a light tier;
  user-supplied custom `.apworld`s require a strong tier or an explicit operator
  opt-in. Keeps the weak-tier fallback *useful*: core games always work; custom
  worlds are gated by the available isolation.
- **Honest posture, not a hard promise.** Sysadmin console shows e.g.
  "Isolation: strong (bwrap, no network)" vs "minimal (limits only) — install
  bubblewrap or use the container image to allow custom worlds."

**Implementation notes (step 3a, on `appliance`).** `src/sandbox.rs` is the
launcher; `ap_adapter` runs every helper through it with a per-helper policy.

- Tiers built so far: **bubblewrap** (strong) and **limits** (the floor).
  Detected at startup (`BIFROST_SANDBOX=auto|bwrap|none`; `bwrap` fails startup
  if unusable) and shown in the log and the sysadmin console. The container
  tier (1), the no-bwrap seccomp tier (3) and macOS/Windows (4) are not built —
  the container image (§3, step 5) should be made bwrap-capable instead.
- Both tiers: empty environment plus only what the helper needs (Bifrost's AP
  passwords / proxy secret never reach world code — they did before), own
  session, `RLIMIT_AS/CPU/FSIZE` (`BIFROST_SANDBOX_MEM_MB` 4096,
  `_CPU_SECS` 1800, `_FILE_MB` 1024), per-helper wall-clock timeout that kills
  the whole process group.
- bubblewrap policy: all namespaces unshared, caps dropped, `/usr` + a short
  `/etc` allowlist read-only (never all of `/etc`: service drop-ins hold
  secrets), private `/tmp`, AP install + scripts + season dir read-only.
  Writable: generation → season `output/` + `logs/`; check/in-logic → a private
  scratch dir; PNG normalization → the pack dir. **No network** for any
  helper. In-logic, which must reach its season's AP server, gets a Unix
  socket in its scratch dir that Bifrost relays to that server alone
  (`ap_adapter::relay_to`; the helper points AP's websocket connect at it).
  Only the AP servers themselves have the network.
- A season `host.yaml` is created on demand: a copy of the operator's
  `$BIFROST_DATA/host.yaml` if there is one, else by a step that imports only
  AP's `settings` (no worlds), so world-importing helpers can keep the season
  read-only. Managed servers get the season's passwords on their command line.
- Trusted vs custom worlds: worlds Bifrost ships (vendored Universal Tracker,
  the operator's common-game apworld) are recognised **by SHA-256**; anything
  else in `worlds/` is custom. On the limits tier, custom worlds are refused
  (upload and every world-importing helper) unless
  `BIFROST_ALLOW_UNSANDBOXED_CUSTOM_WORLDS=1`. Uploads can no longer use the
  shipped worlds' file names (previously an upload named `tracker.apworld`
  replaced the trusted copy).
- Verified: unit tests attack the bubblewrap jail from inside (network, unbound
  files, read-only writes, `/etc`, `/home`, env secrets — all blocked) and the
  floor (env scrub, group kill on timeout, file-size cap); end to end, the
  binary ran check, generate, meta, counts and a networked in-logic against a
  real MultiServer, all inside bubblewrap.

## 5. Identity & authentication

Introduce an **auth abstraction**: one internal `Principal` + session the app
derives from a pluggable provider. Today `identity(&headers)` trusts upstream
proxy headers with no in-app auth; that must become one provider among several.

**Decided (2026-10-26):** native auth is **username + password only** — no
passkeys/WebAuthn, no 2FA. Switching an instance to **IdP-managed auth is a
first-class path**, not an afterthought. **Password resets are reset links
minted by the sysadmin** (no email/SMTP, no self-serve recovery).

Providers:

- **Native accounts (default).** Username + password; argon2id hashing; signed,
  HttpOnly/Secure/SameSite session cookies; CSRF tokens on every state-changing
  POST; login throttling; first-run creates the sysadmin. Recovery: the
  sysadmin mints a single-use, expiring reset link from the console and hands
  it to the user out of band.
  **Account creation is invite-only** (no open registration), via two link
  kinds the sysadmin mints:
  - **single-use invite** — one account, then dead (also expires);
  - **community invite** — multi-use with a timed expiry, for posting in e.g.
    a Discord server; revocable early, shows how many accounts it created.
- **IdP-managed (first-class alternative).** The instance delegates identity to
  an external IdP; Bifrost keeps only the in-app user record (for roles,
  claims, ownership) keyed by the IdP's stable subject. **Auth mode is
  exclusive per instance**: in IdP mode there are no native logins at all.
  Break-glass is a **local CLI** on the host (mint a reset link, switch the
  mode back) — never a web backdoor. **Roles**: config may map IdP groups →
  roles (like today's `archipelago_admins`); where a mapping is set the IdP is
  authoritative for that role, otherwise roles are assigned in Bifrost's
  console. Two transports (**forward-auth is built first**, OIDC follows behind
  the same provider interface):
  - **Trust-header / forward-auth** — the current kanidm/oauth2-proxy mode, but
    **explicitly opt-in and bound to a configured trusted proxy** (shared
    secret or source allowlist). Trusting identity headers unconditionally is a
    spoofing flaw the moment the app is reachable without the proxy.
  - **OIDC** — direct OIDC client for operators who want SSO without a
    forward-auth proxy.

Authorization becomes an **in-app role model** (next section), independent of
kanidm groups.

**Implementation notes (step 2, on `appliance`).**

- `src/auth.rs` — one middleware per request: resolves the `Principal` for the
  instance's mode, refuses cross-site state-changing requests, and enforces the
  §14 route tiers before any handler runs; handlers read the `Me` extractor
  and ask `authorize(principal, Action)`. `src/account.rs` — sign-in/out,
  first-run setup, invite/reset redemption, the `/admin/users` console.
  `src/db/auth.rs` — `account`, `session`, `auth_link` tables (tokens stored
  only as SHA-256). `src/cli.rs` — the break-glass CLI.
- **CSRF: Fetch-Metadata / Origin verification instead of per-form tokens.**
  State-changing requests are refused unless `Sec-Fetch-Site` is
  `same-origin`/`none`, or (older browsers) `Origin` matches `Host`; requests
  with neither header aren't from a browser and can't be CSRF. Together with
  `SameSite=Lax` session cookies this covers every POST, including multipart
  uploads and `fetch` calls, without threading a token through ~30 forms
  (OWASP lists both as valid CSRF defenses).
- Auth mode lives in the DB (`setting.auth_mode`), switched only by
  `bifrost auth mode native|forward-auth` (restart to apply) — the same CLI is
  the break-glass: `bifrost admin reset-link <user>`, `bifrost admin
  create-sysadmin <user>`, `bifrost admin set-role <user> <role>`,
  `bifrost auth status`.
- First run (native, no accounts): Bifrost logs a one-time
  `/setup?token=…` link; whoever holds it creates the sysadmin. No
  first-come-first-served setup page on a public instance.
- Sessions: random 256-bit cookie (`HttpOnly; SameSite=Lax; Secure` — set
  `BIFROST_INSECURE_COOKIES=1` only for plain-HTTP dev), 30-day expiry; a
  password reset or disabling an account ends its sessions. Login backoff is
  per username (5 free failures, then 30 s doubling to 15 min), and unknown
  usernames cost the same argon2 work as wrong passwords.
- Forward-auth config (`BIFROST_FWD_*`): `SECRET_HEADER` + `SECRET` (a header
  the proxy adds) and/or `BIFROST_TRUSTED_PROXIES` (IPs/CIDRs) — every configured
  check must pass, and with neither configured nobody is trusted;
  `SUBJECT_HEADER` / `USERNAME_HEADER` / `GROUPS_HEADER` (oauth2-proxy
  defaults); `ROLE_GROUPS` (`sysadmin=<group>,user=<group>`; when set, the IdP
  is authoritative for roles and users in none of the groups are refused);
  `SIGN_IN_URL` / `SIGN_OUT_URL` for the header bar. Identity headers are read
  **only on signed-in tiers**: on public routes the proxy doesn't overwrite
  them, so a client could supply its own.
- **Season URLs.** Season pages live under `/s/{id}/…` (`/s/3/admin`,
  `/s/3/claim`, …), and Bifrost tiers them by the part after the id. The
  proxy must therefore run forward-auth on those prefixes too, e.g. Traefik
  `PathRegexp(`^/s/[0-9]+/(admin|claim|hint|signup|game|prioritize|minesweeper|api|mc)`)`
  next to the plain prefixes — otherwise it passes a client's own identity
  headers through and Bifrost believes them. A proxy set up for the old
  single-season paths is **not** enough. Instance pages (`/admin/users`,
  `/admin/settings`, `/admin/catalog`, `/api`, …) have one URL only.
- Until seasons have owners (§7) `ManageSeason` is sysadmin-only, matching
  §14's sysadmin-only `/admin`; moderators gain powers with §7.

## 6. Roles & authorization

Three roles, least-privilege:

| Capability | User | Moderator | Sysadmin |
|---|---|---|---|
| Sign up / claim / hint / track in a season | ✔ | ✔ | ✔ |
| Create a season, set its name & window | | ✔ | ✔ |
| Manage a season they own (apworlds, YAMLs, generate, start/stop, archive) | | ✔ (own) | ✔ (any) |
| Manage *any* season | | | ✔ |
| Manage users & role assignment | | | ✔ |
| Instance config: auth providers, isolation tier, AP version/upgrade, backups | | | ✔ |

- **User** — player.
- **Moderator (season mod)** — creates and runs seasons for players; full
  control over seasons they own, no instance-level power.
- **Sysadmin** — infrastructure: users/roles, auth, isolation, AP lifecycle,
  backups, and override on any season.

Seasons have an **owning moderator** (creator) and optional co-mods. Sysadmin is
implicitly authorized on everything. Authorization checks move to a central
`authorize(principal, action, resource)` helper rather than ad-hoc
`is_admin`/`i_claim` scattered through handlers.

*Implemented:* `auth::authorize` with `CreateSeason` (moderator+),
`ManageSeason { mods }` (sysadmin, or a moderator who created or co-mods the
season — `season.created_by` + `season_mod`) and `ManageInstance` (sysadmin).
Season handlers branch on `SeasonCtx::ident` ("may run *this* season");
co-mods are managed on the season's admin page (moderator accounts only).
Moderators reach administration through `/admin/seasons`, which lists every
season with admin links only for the ones they run.

## 7. Multi-season architecture

This is the deepest change. Today the system assumes a single "current" season:
`board.seed`, `output/current.zip`, one AP process on a fixed port, one journal,
single-season background loops. The schema already keys most tables by `season`
— good — but the *runtime* is singular.

### 7.1 Season entity & lifecycle

A first-class `Season` record:

```
id, name (set at creation), created_by (moderator),
state ∈ { draft, signup, generating, live, archived },
seed/seed_name, ap_port (or route key), data_dir, created_at, archived_at
```

Lifecycle is a **Bifrost-managed state machine**, no manual file steps:

```
draft → signup → generating → live → archived
```

- **draft/signup** — mod configures worlds/YAMLs; players sign up.
- **generating** — Bifrost runs sandboxed generation (§4); on success →
- **live** — Bifrost starts an AP server instance for this season; board/game/
  log/hints/tracking all operate on it.
- **archived** — Bifrost stops the instance, frees its port, snapshots the
  season (YAMLs, apworlds, multidata, journal, final standings) into managed
  storage, and keeps it browsable read-only. (Replaces the manual archive step.)

### 7.2 AP instance supervisor (fleet)

Bifrost supervises **N AP server processes**, one per live season:
install/pin → generate → start/stop/restart → health → log capture, per season.
Each instance has its own output dir, savefile, multidata, journal, and port.
The current single-process + systemd-restart coupling becomes a Bifrost-owned
supervisor over a process fleet. Resource note: each live season costs an AP
process (+ generation spikes); the console should show count/limits.

### 7.3 Connections / ports

One AP server == one multiworld, and the AP Connect handshake carries no
"which season" selector, so seasons must be separated *before* the AP protocol.
The AP MultiServer is a raw game socket, not an HTTP app — it does no vhost
routing itself — and native desktop clients connect to a literal `host:port` the
player bookmarks. So a port per live season is the substrate no matter what
fronts it; the only real choice is what, if anything, sits in front.

**Decided: port-per-season is the baseline.** The sysadmin configures a port
range (default e.g. `38281–38300`); the supervisor (§7.2) allocates one port per
*live* season from it, and Bifrost shows each player their season's connect
address on the board. **Concurrency cap = range size** — an intentional,
operator-visible limit (see the resource-ceilings open question). The web UI
stays on Bifrost's single port/host and routes seasons internally (§7.5); only
the AP game sockets consume the range.

Allocation is **stable, not ephemeral**: a port is pinned at go-live and held
until archive — never re-picked on restart — so a crash-and-restart returns on
the same port and bookmarked connect strings keep working. Lifecycle:
assign-at-go-live → hold → release-on-archive, with a boot-time reconciliation
pass (reclaim ports whose season is gone; detect a port still held by a zombie
instance before reassigning).

**Optional overlay (not an alternative) — subdomain / `:443` for browser
clients.** A deployment that already runs a reverse proxy with wildcard TLS can
additionally expose each season's wss at `season-x.host:443` (the Candy Box
`:443` trick), giving browser clients nice names on one public port. This layers
*on top* of the per-season port — native clients still use the real port — so it
depends on the operator's TLS/DNS and is an enhancement, not a replacement.
Design the connect-address as an abstraction so B slots in without touching
handlers.

### 7.4 Background workers per season

The journal ingester, counts refresher, and in-logic recomputer become
**per-season** (a supervised task set per live season, torn down on archive)
rather than one global loop. Keyed by the season's instance + data dir.

### 7.5 Routing & "current season"

Drop the implicit current-season assumption. URLs gain a season scope, e.g.
`/s/{season_id}/board`, `/s/{season_id}/game/{slot}`, `/s/{season_id}/log`, with
a top-level season switcher and a sensible default (most recent live). The
landing page lists seasons by state. This touches most handlers; do it behind a
`SeasonCtx` extractor so handlers resolve a season once.

### 7.6 Data model deltas

- New `season` table (above); `created_by`, `name`, `state`, `ap_port`/route.
- `account` (with `role`) + `session` + `auth_link` tables (native auth) —
  done in step 2; every per-user table keys on `account_id`, never a name.
- New `season_mod` (season ↔ moderator) for co-mods.
- Per-season apworld pinning (§8) table.
- Existing season-keyed tables: confirm every query is season-scoped (mostly
  already true); migrate `seed`-keyed assumptions to `season_id`.
- On-disk: per-season state moves under `$BIFROST_DATA/seasons/<id>/…` (§3a),
  replacing today's single flat `Players/custom_worlds/output/logs`.

### 7.7 Implementation notes (step 4, on `appliance`)

Built (the AP fleet included; the tabbed console is still to come):

- **Season entity** (`src/db/seasons.rs`): `season` table — name at creation,
  creator, lifecycle state, seed, live/archived times, per-season toggles
  (minesweeper, common game), journal cursor, frozen summary. Lifecycle rules
  are a pure, tested `State::can_go`; `Db::transition` enforces them plus
  **room to go live**: a free AP port in managed mode (§7.3), or no other
  live season with an external server. A crash mid-generation can't strand a
  season in `generating`.
- **Re-keyed data:** every season-scoped table (`bifrost_slot`, `submission`,
  `hint_credit`, `hint_priority`, `activity`, `inlogic`, `mine_result`) is keyed
  `season INTEGER REFERENCES season(id) ON DELETE CASCADE` (freeplay
  minesweeper = NULL); `season_archive` and the global settings rows are gone.
  A re-roll stays in its season: `clear_seed_data` drops what the old seed
  produced (activity, cursor, priorities, in-logic) and `remap_season` carries
  slots + claims onto the new AP slot numbers; minesweeper results and hint
  credits now survive a re-roll.
- **Per-season runtime** (`src/runtime.rs`): board + generation state per
  season; each live season's AP server and workers (tracker, counts, activity, in-logic) start
  on going live and stop on archive. The tracker refuses an AP server running
  a different seed than the season deployed.
- **Per-season dirs:** `seasons/<id>/{Players,worlds,output,logs,submissions}`
  (+ `host.yaml`, `worlds.lock.json`); `seasons/_core/` for YAML checks before
  any season exists.
- **Season-scoped URLs** (`src/scope.rs`): `/s/{id}/…` is rewritten before
  routing (auth tiers and rate limits keep matching plain paths); handlers take
  a `SeasonCtx`; unscoped URLs resolve to the default season (live → generating
  → signup → draft → latest archived). Every page carries `<base href="/s/{id}/">`
  for the season it was rendered for, and season links are base-relative, so a
  page never drifts to another season mid-flow; server redirects and form
  `back` targets go through `SeasonCtx::url` / `back`; post-login redirects keep
  the scoped URL.
- **Lifecycle UI:** `/admin/seasons` lists seasons and creates them (named up
  front); a season's admin page drives draft⇄signup, generate (→ live),
  re-roll, and **archive** (live → archived: frozen summary + raw bundle +
  workers stopped; replaces the manual archive). Signups and apworld uploads
  are accepted only in `signup`. Archived seasons' boards show their summary
  at `/s/{id}/season`; the landing page lists them.

**AP server fleet (managed mode, the default).** `src/apserver.rs` runs one
MultiServer per live season — sandboxed with network (it imports worlds), on
`BIFROST_AP_BIND` and the season's own port, `scripts/bifrost_server.py`
pinning AP to the season dir. It restarts the server with backoff if it dies
(and retries a start that failed, e.g. because a leftover process still holds
the port — checked before launching, and logged plainly); **redeploys stop it (it saves) before wiping
the save and swapping the seed in**, so the old seed's state can't leak into
the new world (the race the old reload unit worked around); archive and
Bifrost shutdown (SIGTERM / Ctrl-C) stop it cleanly with `/exit`. Its output
goes to `seasons/<id>/logs/server.log` and straight into the activity feed
(no journald). Per-season passwords live on the season row: a connect
password the season's mods set (applied live via the server console) and a
generated admin password only Bifrost uses; Bifrost's own connections join
as `BIFROST_SLOT` or the season's first slot. `BIFROST_AP_MODE=external`
keeps the old arrangement (env-configured endpoint + passwords, journal
ingestion, host-side restarts) and with it one live season at a time.

**Ports (§7.3, as built).** `BIFROST_AP_PORTS` (default `38281-38300`) is the
range; `season.ap_port` (UNIQUE) is assigned inside the go-live transition
(`BEGIN IMMEDIATE`, lowest free port, or the one the season already holds if
it's still in range — so re-rolls and restarts keep it) and cleared on
archive. Generation checks for room *before* rolling, so a full range refuses
up front with "every AP port … is held by a live season". Boot runs
`reconcile_ap_ports`: non-live seasons drop their port, live ones keep theirs
if still in range or get re-pinned; any that can't be placed are logged and
left without a server. `Runtimes::connect_address` is the one place the
player-facing address is built (instance connect host + season port) — the
`:443`/subdomain overlay slots in there.

**Instance settings.** A sysadmin-only `/admin/settings` page (stored in the
`setting` table, not `config.toml`) holds what's operator-specific on the
public pages: the connect host players' clients use, the password note, an
optional header "home" link, and the Discord servers list. The home page
renders entirely from it — one connect address per live season — and shows
the Archipelago version read from the managed install's `Utils.py`
(`ap_adapter::ap_version`), as does the settings page alongside the port
range. The board shows its season's address.

The tabbed consoles (§9) and co-moderators/moderator ownership (§6) are in.

## 8. Season apworld & YAML management

Per-season, owned by the moderator, surfaced in the season console:

- **apworlds:** move from "upload any `.apworld`" to a **per-season allowlist**
  of `{game → source + pinned version + sha256}`. Sources: the bundled core set,
  or an **admin-curated catalog** (optionally seeded from the
  `silasary/apworlds` index as a *discovery* source — never an automatic trust
  root), or an explicit upload that a sysadmin/mod accepts. Downloads verify the
  pinned hash → tamper-evident, and custom worlds are gated by the isolation
  tier (§4). This kills the "insider uploads a bespoke RCE payload" vector and
  makes upstream swaps detectable.
- **YAMLs:** per-season list with live validation (sandboxed check), per-player
  ownership, replace/remove, and clear errors. Validation runs in the same
  sandbox as generation.

**Implementation notes (step 3b, partial).** Hash pinning is in: every upload
goes through `ap_adapter::install_world`, which records its SHA-256, uploader
and time in `seasons/<id>/worlds.lock.json` (outside `worlds/`, read-only to
helpers, included in the season bundle). Before any world-importing helper
runs, every custom world must match its pin — an unpinned file (dropped in
behind Bifrost's back) or one changed since upload is refused on every tier,
and the admin's apworld list shows each world's provenance (shipped / pinned
hash + uploader / ⚠). Worlds Bifrost ships are trusted by hash, not pinned.
**Catalog + approval — as built (`src/worlds.rs`, `db/worlds.rs`).** A
sysadmin-curated **catalog** (instance console → Apworlds) holds vetted worlds
as `catalog/<sha256>.apworld`, with game/version read from the world's
`archipelago.json` (overridable) and a note. Players can pick a catalog world
in signup step 1, and an upload whose bytes match a catalog entry is accepted
as that entry. Everything else follows the sysadmin's **approval toggle**:
*automatic* installs uploads straight away (pinned as before); *manual* (the
default) parks them in `seasons/<id>/pending/` — outside `worlds/`, so AP
never loads them — as a `world_request`. A signup whose YAML needs a pending
world is *held* (`submission.held_for`): its slot name is read from the YAML's
top level without AP, and it's checked for real on approval. The season's
moderators or a sysadmin approve or reject from the season console; approval
installs and pins the world, then re-checks the held signups in the
background (pass → released with the name/game the check read; fail → the
reason shown on the roster). Rejection leaves held signups flagged until
removed. Generation refuses while any signup is held. A different file under
a name that's installed or pending is refused (no swapping a world out from
under others' YAMLs). Sysadmins can also promote an installed upload into the
catalog from the season console. Removing a catalog entry leaves seasons'
installed copies alone. Not built: seeding discovery from an external index
(`silasary/apworlds`) — the catalog is filled by hand.

## 9. Console redesign (tabbed)

Split today's flat admin page into a **tabbed console**, scoped by role:

- **Sysadmin console (instance):** Users & roles · Auth providers · Isolation &
  AP version/upgrade · Backups · Danger zone.
- **Season console (per season, for its mod/sysadmin):** Overview/lifecycle ·
  apworlds · YAMLs · Players & slots · Connection info · Archive.

Season creation form takes the **name up front** (and signup window). Archiving
is a button that drives the managed transition in §7.1 — no manual copying.

**As built:** the season console is one page with client-side tabs —
Overview (signups window, generate/re-roll, moderators) · Signups & apworlds ·
Player files · Connection (address, connect password, server console) ·
Options (minesweeper, common game) · Archive & reset. The open tab lives in the
URL hash and is stamped onto every form action, so post→redirect lands on the
same tab. The instance console is three linked pages under one tab bar —
Seasons · Users & links · Settings (connect host, home-page content, plus
read-only AP version, port range, auth mode, isolation); moderators see only
Seasons. Not yet: auth-provider settings in the UI (the CLI owns auth mode).

**Archipelago upgrades — as built (`src/apinstall.rs`, `src/apupdate.rs`,
`scripts/bifrost_ap_install.py`).** Buttons on the settings page, no console.
*Check for releases* asks GitHub (`BIFROST_AP_REPO`) for recent releases;
*install* runs the installer under `BIFROST_PYTHON` (outside the sandbox: it
needs the network) into `$BIFROST_DATA/ap/.partial-<tag>/` — source tarball,
its own venv, core + per-world requirements (GUI-only Kivy/KivyMD skipped; a
world whose extras fail is reported, not fatal; Cython speedups when a
compiler + headers exist, else AP's pure-Python fallback) — then a sandboxed
test check + one-player generation with it; only then is it moved to
`ap/<version>/` and marked ready. The page shows progress and refreshes until
done. *Use for new seasons* makes an install **active** (signup checks and new
generations use it). Each generation **pins** its season to the install it ran
with (`season.ap_version`), so a live season's server and helpers keep that
version through restarts until it's re-rolled or archived — switching never
changes a live game. *Remove* is refused for the bundled install, the active
one, and any a non-archived season is pinned to. AP's own pip-on-startup is
off (`SKIP_REQUIREMENTS_UPDATE`): Bifrost owns each install's dependencies.

**Archipelago fork — as built (`packaging/archipelago/`).** Releases come
from Bifrost's fork, `vidarrio/Archipelago` (the `BIFROST_AP_REPO` default),
so Bifrost can carry small compatibility patches: each build is an upstream
tag plus `packaging/archipelago/patches/`, released as
`<tag>-bifrost.<build>` by `release.sh`, with a `BIFROST_VERSION` file that
names the install (players are shown the upstream part). The first patch
silences join/leave/tag-change/`!admin` announcements for clients that
present the server's `BIFROST_QUIET_KEY` (random per Bifrost process, given
only to the AP servers it runs; `ap::quiet_key`, also passed to the in-logic
helper). A client-chosen tag would let players connect unannounced. Upstream
servers still work; they just announce those clients.

**Backups — as built (`src/backup.rs`).** Nightly at `[backup] hour` (UTC,
default 04:00; `off` disables), plus *Back up now* on the settings page and
`bifrost backup now`. One owner-only zip per backup in `[backup] dir`
(default `$BIFROST_DATA/backups`; another disk is better): a `VACUUM INTO`
snapshot of the database plus every file under the data dir except backups,
`ap/` (reinstallable) and `tmp/`. Retention keeps the newest backup of each of
the last 7 days, 4 weeks and 3 months that have one, then drops the oldest
while the total exceeds `max_mb` (default 5 GiB) — never the newest; a backup
is skipped when the disk couldn't fit another one plus 512 MiB headroom. The
page lists backups with download/delete. Restore is offline:
`bifrost backup restore <zip>` refuses while something listens on Bifrost's
port, and moves the current contents to `.pre-restore-<time>/` instead of
deleting them.

## 10. Security hardening checklist

Beyond §4 isolation:

- **Header-spoof fix** (trust-header provider bound to a trusted proxy) — do now.
- **Rate limiting** on auth + expensive endpoints (generation, checks).
  *Done (3c):* `src/ratelimit.rs` token buckets, keyed by account (signed in)
  or client IP; login/setup/invite/reset always by IP. YAML checks 10/10 min,
  signups 10/10 min, apworld uploads 10/h, login 20/10 min, link redemption
  10/10 min, hints 30/10 min; 429 + `Retry-After`. The checker queue is capped
  (8) so a burst can't build an unbounded backlog behind its one-at-a-time
  gate. Client IP = TCP peer, or the right-most untrusted `X-Forwarded-For`
  hop when the peer is in `BIFROST_TRUSTED_PROXIES` — the same list
  forward-auth trusts (replaces `BIFROST_FWD_TRUSTED_PROXIES`). Generation is
  sysadmin-only and already single-flight.
- **Secrets**: move off host env to a config file / first-run setup; no secrets
  in world-visible state.
  *Done (3d):* every `BIFROST_*` setting can live in `$BIFROST_DATA/config.toml`
  (`[server]`, `[archipelago]`, `[runtime]`, `[sandbox]`, `[forward_auth]`,
  `[minecraft]`, `[season]`; template: `bifrost config example`), read once
  via `config::var` — environment overrides the file, so containers can still
  inject. Startup warns if the file is group/other-readable; unknown keys fail
  startup (a typo'd secret must not silently fall back). Sandboxed helpers get
  an empty environment (3a), so neither source reaches world code. The
  homelab-specific `BIFROST_MC_ADDR` default is gone (unset = not shown).
  Still open: AP server passwords get generated by Bifrost once it owns the AP
  process (§7.2); until then they're operator-set.
- **CSRF** on all state-changing POSTs (comes with native sessions).
- **PopTracker pack sandbox**: already hardened (stdlib-stripped Lua, mem +
  instruction caps, `pack_join` traversal guard, zip-extraction byte cap).
  Re-audit once users are less trusted.
- **Input validation** pass over newly-public endpoints.
- **Per-season isolation of data dirs** so one season can't read another's files.

## 11. Transition from the current deployment

**Clean break, no in-place migration.** The current system (single season on
the Bifrost host, co-located with AP) keeps running untouched from `main` until its
season ends. After that it is retired and the appliance is deployed fresh —
new data dir, new config, new service wiring. Consequences:

- No backwards compatibility is built: config names, env vars, on-disk layout
  and DB schema may change freely on the `appliance` branch.
- No step needs to be "shippable onto the live box"; each step only needs to
  build, pass tests, and leave the branch working.
- Anything worth keeping from the old instance (e.g. past-season archive
  bundles / summaries) is carried over as a one-off import at redeploy time,
  decided then — not via compatibility code.

## 12. Risks & open decisions

- **AP-internals treadmill** — pinning + the adapter contain it, but upgrades are
  ongoing work.
- **Connections** — decided (§7.3): stable port-per-season from a configured
  range (cap = range size); subdomain/`:443` is an optional overlay for browser
  clients, not an alternative.
- **Single-binary AP bundling cost** — accepted as a convenience tier; container
  is primary.
- **Resource ceilings** for concurrent live seasons — addressed by live load
  tracking (`src/load.rs`): sampled every 10 s from `/proc` (a container's
  cgroup v2 memory limit wins over the host's), an hour kept; the settings page
  charts memory and CPU, lists each live season's AP server (its whole process
  tree) and the helpers, and warns on tight memory, a busy CPU or a filling
  disk; a season's admin page warns before going live when the largest
  running server wouldn't fit in what's free. Warnings, not a hard cap.

## 13. Proposed sequencing

1. **De-hardcode paths + `ap_adapter`** (prerequisite; no feature change) —
   move to the `$BIFROST_DATA` layout (§3a) directly; no compatibility with the
   current deployment's paths (§11).
2. **Auth abstraction + native accounts** (default) and the trust-header
   spoof fix; in-app roles (sysadmin/mod/user) + `authorize()`.
3. **Security hardening**: `sandbox::spawn` tiers + no-net + trusted/untrusted
   split; apworld allowlist + hash pinning; rate limiting; secrets/first-run.
4. **Multi-season core**: `Season` entity + lifecycle, fleet supervisor,
   per-season workers, season-scoped routing, console tabs, managed archiving.
5. **Packaging**: container image (primary) + native self-extracting (optional);
   AP bundled and lifecycle-owned.

1 de-risks everything; 2–3 are the "ready to publish" gates; 4 is the headline;
5 is the distributable bundle. Multi-season (4) can precede packaging (5) and
land on the current LXC host first.

## 14. Auth route split — what native auth must reproduce

*Implemented by `auth::tier` + `auth::middleware` (step 2).*

Today the public / authenticated / admin split is enforced at the reverse proxy
(forward-auth). When Bifrost owns auth it must reproduce the **same split
in-app**, with the same default-public behaviour. The split is by URL path
prefix, matched most-specific-first, with an implicit public catch-all.

| Tier | Route prefixes | Who |
|---|---|---|
| **Sysadmin** | `/admin/users` `/admin/links` | sysadmin only |
| **Admin** | `/admin` (season admin, `/admin/seasons`) | moderators and up — the handler then requires that they run *this* season |
| **Authenticated** | `/claim` `/hint` `/signup` `/game` `/prioritize` `/minesweeper` `/api` `/mc` | any logged-in user (user or admin) |
| **Public** | everything else — `/`, `/board`, `/check`, `/log`, static assets | anyone, no login |

Rules to preserve exactly:
- **Default is public.** Gating is an explicit allow-list of prefixes; anything
  not listed is public (fail-open to public, never the reverse).
- `/game` is authenticated, so the PopTracker `…/poptracker/{state,asset}` and
  the per-game endpoints under it require a login — keep that.
- Most-specific prefix wins (`/admin` is admin even though `/` is public).

Transport/identity contract (forward-auth mode stays byte-compatible with the
existing proxy; native mode yields the same `Principal`): on allow, the upstream
sets request headers the app trusts — `X-Auth-Request-User`, `-Email`,
`-Groups`, `-Preferred-Username` — and group membership picks the tier; on deny
the proxy 302s to sign-in. The trust-header provider MUST be bound to the trusted
proxy (the spoof fix in §5); those headers are otherwise forgeable.

Role mapping: the live instance has two groups today (a *users* group → **user**
tier, an *admins* group → **admin** tier); the design's third role
(**moderator**) layers on later (§6). Exact group names / domains / proxy
addresses are per-instance config, not part of this doc — kept in the
(gitignored) operational handoff.
