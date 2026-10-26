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
  scratch dir; PNG normalization → the pack dir. **No network**, except
  **in-logic**, which must reach the AP server (`--share-net`): world code
  there runs sandboxed but networked — a known gap until the AP server is
  reachable over a socket bound into the sandbox.
- A season `host.yaml` is created on demand by a step that imports only AP's
  `settings` (no worlds), so world-importing helpers can keep the season
  read-only.
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
  the proxy adds) and/or `TRUSTED_PROXIES` (IPs/CIDRs) — every configured
  check must pass, and with neither configured nobody is trusted;
  `SUBJECT_HEADER` / `USERNAME_HEADER` / `GROUPS_HEADER` (oauth2-proxy
  defaults); `ROLE_GROUPS` (`sysadmin=<group>,user=<group>`; when set, the IdP
  is authoritative for roles and users in none of the groups are refused);
  `SIGN_IN_URL` / `SIGN_OUT_URL` for the header bar. Identity headers are read
  **only on signed-in tiers**: on public routes the proxy doesn't overwrite
  them, so a client could supply its own.
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
"which season" selector, so seasons must be separated *before* the AP protocol:

- **Option A — port per season (recommended first):** allocate from a configured
  range; Bifrost shows each player their season's connect address. Simple,
  robust; operator forwards a port range.
- **Option B — SNI/subdomain or reverse-proxy routing:** `season-x.host` →
  Bifrost ws-proxies to the right instance, one public port. Nicer UX, more
  moving parts, and depends on the operator's TLS/DNS. Offer as an enhancement.

Decision deferred; start with A, design the connect-address as an abstraction so
B can slot in.

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

## 9. Console redesign (tabbed)

Split today's flat admin page into a **tabbed console**, scoped by role:

- **Sysadmin console (instance):** Users & roles · Auth providers · Isolation &
  AP version/upgrade · Backups · Danger zone.
- **Season console (per season, for its mod/sysadmin):** Overview/lifecycle ·
  apworlds · YAMLs · Players & slots · Connection info · Archive.

Season creation form takes the **name up front** (and signup window). Archiving
is a button that drives the managed transition in §7.1 — no manual copying.

## 10. Security hardening checklist

Beyond §4 isolation:

- **Header-spoof fix** (trust-header provider bound to a trusted proxy) — do now.
- **Rate limiting** on auth + expensive endpoints (generation, checks).
- **Secrets**: move off host env to a config file / first-run setup; no secrets
  in world-visible state.
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
- **Connections** — port-per-season vs. proxy routing (§7.3): decide before the
  fleet lands.
- **Single-binary AP bundling cost** — accepted as a convenience tier; container
  is primary.
- **Resource ceilings** for concurrent live seasons — need an operator-visible
  limit/warning.

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
| **Admin** | `/admin` | admin role only |
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
