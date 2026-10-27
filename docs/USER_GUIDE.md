# Bifrost user guide

Bifrost runs recurring [Archipelago](https://archipelago.gg) multiworld seasons
for a group: signups, seed generation, the Archipelago servers, a live board
with hints, and an archive of past seasons, all from one web app.

This guide has a part for each role:

- [Players](#players): signing up, claiming a slot, connecting, hints and tracking.
- [Moderators](#moderators): creating and running seasons.
- [Sysadmins](#sysadmins): installing and operating the instance.

## Roles

| Role | Can |
|---|---|
| **User** (player) | Sign up for seasons, claim slots, hint, track, play minesweeper. |
| **Moderator** | Everything a user can, plus create seasons and run the ones they created or co-moderate. |
| **Sysadmin** | Everything, on every season, plus users, invites, the apworld catalog, Archipelago versions, backups and instance settings. |

Some pages are public: the home page, the live board, the activity log, the
YAML checker and past-season summaries. Everything else needs you to be
signed in.

### Seasons

A season moves through four states:

1. **Draft**: created, signups closed.
2. **Signups**: players submit their games.
3. **Live**: generated; its Archipelago server is running.
4. **Archived**: finished. Its summary stays on the home page under *Past seasons*.

Several seasons can be live at once. Each live season has its own server on
its own port. When a page isn't tied to a particular season (the home page,
`/board`, `/signup`), it means the live season. If none is live, it means the
newest season in progress, and failing that the most recently archived one.
Every season also has its own addresses under `/s/<id>/…`, such as
`/s/3/board`.

---

## Players

### Getting an account

Bifrost is invite-only. A sysadmin gives you an invite link, either a personal
one or a community link posted in your group's chat. Open it and pick a
username and password. If you forget your password, ask a sysadmin for a
reset link; there's no email recovery.

On instances that sign in through an identity provider (an external login
service such as kanidm or Authentik), there is no Bifrost password. Use the
*sign in* link in the header.

### Signing up for a season

Open **Sign up for a season** from the home page while a season's signups are
open. The wizard has three steps:

1. **Pick the game.** Choose one of:
   - a core (officially supported) game;
   - a world from the server's catalog (custom worlds the admins have vetted);
   - an upload of another custom world's `.apworld` file, if the server allows custom worlds.
2. **Upload your options YAML.** It is checked against the season's installed
   worlds with Archipelago's own code, and has to pass before you can
   continue. Errors are shown as Archipelago reports them.
3. **Confirm.** If several people share the game (co-op), list the others,
   comma-separated. They claim their own seat once the season is generated.

You can replace or delete your signup until the season is generated.

On some servers, uploading a world that isn't in the catalog makes it wait for
a moderator's approval. Your signup then shows as *held*. It is checked
automatically once the world is approved. If the world is rejected, the signup
stays flagged and you'll need to pick something else.

The **YAML checker** (`/check`) runs the same check without signing up. It's
useful when you're tuning options.

### After generation: claiming your slot

When the season is generated, open the **live board** and **claim** your slot.
Only the claimant can hint on a slot. If you grab the wrong one, unclaim it.
Usually your own signups are claimed for you at generation; co-op partners
claim their own seats.

### Connecting

The home page lists each live season's address (`host:port`) and, if one is
set, the connect password. In your game's Archipelago client, connect to that
address using your slot name.

If your game needs a patch or output file, download it from your slot's
**game page** (live board → your slot). Most games connect directly and need
no file.

### Your game page

Click your slot on the live board to open its game page. It has four tabs:

- **Hints.** Your hint points and what a hint costs in this world. Search your
  item pool and hint an item. Hints already found for your items are listed;
  mark the ones you care about as **prioritized** so other players see them
  first. Hint points come from Archipelago: you earn them by checking
  locations.
- **Universal Tracker.** Which of your locations are in logic right now,
  recomputed a few seconds after your items or checks change.
- **PopTracker.** Upload a PopTracker pack (`.zip`) for your game to get its
  map and auto-tracking in the browser.
- **Log.** This game's activity: items sent and received, hints, goal.

### Bonus hints: minesweeper

If the season's moderators turned it on, **minesweeper** on the season board
earns bonus hints. Each board you clear without hitting a mine earns one
bonus hint. Every board can be solved without guessing. Bonus hints are spent
automatically when you're out of hint points.

The minesweeper linked from the home page is free play: it counts toward the
all-time leaderboard but earns no hints.

### The common game

A season can include a shared **common game**: a Minecraft world anyone with
a claimed game this season can join. It has its own hint points, earned as the
group checks its locations, which any player with a slot can spend. The common
game's page explains the exact client setup it needs. The server is
whitelisted, so register your Minecraft username there before joining.

### Past seasons

Archived seasons stay on the home page with their final standings, who played
what, and minesweeper results.

---

## Moderators

Moderators reach administration through the **admin** link in the header,
which opens the **Seasons** console. It lists every season. You get admin
links for the seasons you run, plus a form to create a new one.

### Creating a season

Give it a name. It starts in **Draft**. Open its admin page (the season
console) to set it up.

### The season console

The season console has six tabs:

- **Overview.** The season's state and lifecycle actions:
  - **Open / close signups.**
  - **Generate.** Stages the queued signups, rolls a seed, maps slots and
    auto-claims, starts the season's Archipelago server, and closes signups.
  - **Re-roll.** Regenerates the current lineup with a fresh seed and save.
    Claims are kept; the game restarts.
  - **Moderators.** Add co-moderators (moderator accounts only).

  Generation refuses while any signup is held for an unapproved world. A
  warning shows when the server is short on memory for another live season.
- **Signups & apworlds.** The roster with each signup's check result, the
  season's installed custom worlds and where each came from, and **apworlds
  waiting for approval**. Approving installs the world and re-checks the
  signups held for it. Rejecting leaves them flagged. Sysadmins can also
  promote an installed world into the catalog from here.
- **Player files.** Each player's YAML and output files, for download.
- **Connection.** The season's address, its **connect password** (changing it
  restarts the server), and a **server console** for Archipelago server
  commands, such as `/send`, with auto-completion.
- **Options.** Turn the season's minesweeper and the common game on or off.
- **Archive & reset.**
  - **Archive this season.** Finishes it: the server is stopped, a summary is
    written for *Past seasons*, and the season's files are packed into a
    bundle. The game must be connected (its board loaded) to archive.
  - **Delete the season's players.** Removes all slots, claims and staged YAMLs
    and keeps the signups, so you can start the round over.

On a game page, the season's admins also see **Mark finished**. It registers
the player's goal (announcing it) and collects or releases their items
according to the server settings. It can't be undone from Bifrost.

### Season bundles

An archived season's admins can download its **bundle** from the Seasons
console. The bundle contains the summary, seed, save, YAMLs and worlds.
Another Bifrost instance can import it (see
[Importing past seasons](#importing-past-seasons)).

### Archipelago settings for a season

Hint cost, release and collect modes, and the other Archipelago generation
settings come from the instance's `host.yaml` (see
[Configuration](#configuration)). Each new season starts from a copy of it.

---

## Sysadmins

### Installing

**Container (recommended).** The image contains Bifrost, a pinned Archipelago
build and bubblewrap:

```sh
podman build -t bifrost .            # or docker build
podman run -d --name bifrost --stop-timeout 30 --security-opt unmask=/proc/* \
    -p 8080:8080 -p 38281-38300:38281-38300 -v bifrost-data:/var/lib/bifrost bifrost
podman logs bifrost                  # the first-run /setup link
```

- `--security-opt unmask=/proc/*` lets the sandbox mount its own `/proc`.
  Docker's equivalent is `--security-opt systempaths=unconfined`. Without it,
  Bifrost falls back to a weaker sandbox and refuses custom apworlds.
- `--stop-timeout 30` gives live servers time to save on shutdown.
- Publish the whole Archipelago port range. Its size caps how many seasons can
  be live at once.

**Native single file (Linux x86_64).** A static binary with its own Python and
Archipelago appended. It unpacks them into the data directory on first run:

```sh
packaging/bundle/build.sh            # → dist/bifrost-<version>-x86_64-linux
./dist/bifrost-<version>-x86_64-linux
```

Without `BIFROST_DATA`, it keeps its data in `~/.local/share/bifrost`, or in
`/var/lib/bifrost` when run as root. Install `bubblewrap` on the host for the
strong sandbox, which custom apworlds need.

Put Bifrost behind a reverse proxy that terminates HTTPS. Session cookies are
marked `Secure`. Add the proxy's address to `trusted_proxies` so per-client
rate limiting sees real client addresses.

### First run

The log prints a one-time `/setup?token=…` link. Open it to create the first
sysadmin. Then:

1. **Console → Settings**: set the **connect host** players use. Optionally
   set a password note, a home link for the header pill, and Discord servers
   for the home page.
2. **Console → Users & links**: mint invites.
3. **Console → Seasons**: create a season and open its signups. Or hand that
   job to a moderator.

### Users and invites

**Console → Users & links**:

- **Single-use invites**: one account, then the link is dead. Invites expire.
- **Community invites**: multi-use links with an expiry, for posting in a
  group chat. Revocable early; each shows how many accounts it created.
- **Reset links**: give one to a user who forgot their password. Any reset
  ends their existing sessions.
- **Roles**: user, moderator, sysadmin. You can also disable accounts.

### The apworld catalog and approval

**Console → Apworlds** holds the **catalog**: custom worlds you have vetted.
Players can pick them at signup, and an upload whose bytes match a catalog
entry is treated as that entry. The game and version are read from the
world's `archipelago.json`, and you can override them.

The **approval toggle** decides what happens to other uploads:

- **Manual** (default): uploads wait for a season moderator's approval. Until
  then, Archipelago never loads them.
- **Automatic**: uploads install straight away.

Every installed world is pinned by its SHA-256. A world file that changes
after upload, or appears without going through Bifrost, is refused.

### Archipelago versions

The image and bundle ship a pinned Archipelago. **Settings → Archipelago**
manages further versions:

- **Check for releases.** Lists recent releases of Bifrost's Archipelago fork.
  The fork is upstream Archipelago plus small compatibility patches, such as
  silencing join and leave announcements for Bifrost's own connections.
- **Install.** Builds the release with its own Python environment. A release
  is only offered once a sandboxed test generation passes with it.
- **Use for new seasons.** Signup checks and new generations use this version.
  A live season keeps the version it was generated with until it's re-rolled
  or archived, so switching never changes a running game.
- **Remove.** Refused for the bundled version, the active one, and any version
  a season still uses.

### Backups

Backups run nightly (04:00 UTC by default), and on demand with **Settings →
Back up now**. Retention keeps the newest backup of each of the last 7 days,
4 weeks and 3 months, within 5 GiB by default. The settings page lists backups
for download or deletion. Each backup is a zip of the database plus the data
directory. The Archipelago installs aren't included because they can be
reinstalled.

To restore, stop Bifrost first, then run:

```sh
bifrost backup restore <backup.zip>
```

The current contents are moved aside to `.pre-restore-<time>/`, not deleted.
Prefer a backup directory on another disk (`[backup] dir`).

### Live load

**Settings → Load** shows host memory, CPU and disk over the last hour, and
what each live season's server and the sandboxed helpers use. It warns when
memory, CPU or disk run short. A season's console also warns before go-live
if another server probably wouldn't fit.

### Importing past seasons

Upload a season bundle (`.tar.gz`) under **Console → Seasons → Import a
finished season**, or on the host run:

```sh
bifrost season import <bundle.tar.gz>
```

The season appears under *Past seasons*. A season whose seed is already
present is refused.

### Sign-in modes

**Native** (default): usernames and passwords managed by Bifrost.

**Forward-auth**: an identity provider authenticates users through your
reverse proxy (for example oauth2-proxy with Traefik), and Bifrost reads the
identity headers the proxy sets. The mode is exclusive: in forward-auth mode
there are no Bifrost passwords. Switch with:

```sh
bifrost auth mode forward-auth       # or: native — restart Bifrost to apply
```

Configure it under `[forward_auth]`:

- Bifrost trusts identity headers only from a configured proxy: a shared
  secret header the proxy adds (`secret_header` + `secret`), and/or the proxy's
  address in `trusted_proxies`. Every configured check must pass. With neither
  configured, nobody is trusted.
- `role_groups` maps groups to roles, for example
  `sysadmin=archipelago_admins,moderator=archipelago_moderators,user=archipelago_users`.
  When it's set, the identity provider decides roles, and users in none of the
  groups are refused. Without it, roles are assigned in the console.
- `sign_in_url` / `sign_out_url` are used for the header's links.

The proxy must run forward-auth on every signed-in path. That includes the
season-scoped forms under `/s/<id>/`, not only the plain prefixes. Otherwise a
client can send its own identity headers through the proxy. The signed-in
prefixes are `/admin`, `/claim`, `/hint`, `/signup`, `/game`, `/prioritize`,
`/minesweeper`, `/api` and `/mc`. Everything else is public. A Traefik rule
for the season forms looks like:

```
PathRegexp(`^/s/[0-9]+/(admin|claim|hint|signup|game|prioritize|minesweeper|api|mc)`)
```

### Isolation

Anything that runs apworld code runs sandboxed. That covers YAML checks,
generation, seed decoding and the Archipelago servers. With bubblewrap
available, the sandbox gives each helper:

- no network (the servers excepted; the in-logic tracker reaches its season's
  server through a relayed socket);
- a read-only system;
- write access only to the season's own output.

Without bubblewrap, Bifrost applies resource limits only, and refuses custom
apworlds unless you explicitly set `allow_unsandboxed_custom_worlds`. The
settings page shows which tier is active. Resource limits for checks and
generation are under `[sandbox]`.

### Configuration

Settings come from the environment or from `$BIFROST_DATA/config.toml`. The
environment wins. Print a commented template of every setting with:

```sh
bifrost config example
```

Keep `config.toml` at `chmod 600`, since it can hold secrets. Page-level
settings, such as the connect host, home page, Discords, apworld approval and
Archipelago versions, are set in the console instead.

Archipelago's own generation settings (hint cost, release and collect modes,
and so on) come from `$BIFROST_DATA/host.yaml` if you put one there. Each new
season starts from a copy of it. Archipelago's defaults apply otherwise.

### Command line

The CLI works on the data directory directly. It is also the break-glass path
when nobody can sign in.

```
bifrost auth status                       show the sign-in mode and accounts
bifrost auth mode native|forward-auth     switch sign-in mode (restart to apply)
bifrost admin reset-link <username>       mint a 24 h password-reset link
bifrost admin create-sysadmin <username>  create a sysadmin + reset link
bifrost admin set-role <username> <role>  user|moderator|sysadmin
bifrost backup now | list | restore <zip> backups (restore with Bifrost stopped)
bifrost season import <bundle.tar.gz>     add a finished season from a bundle
bifrost config example                    print the configuration template
```

### Data directory

Everything mutable lives in one data directory: the database, seasons,
catalog, Archipelago installs, archives and backups (unless moved). Back it
up, or let the nightly backups do it, and keep it on persistent storage.
