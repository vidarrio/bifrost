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

If your game needs a patch or output file (a ROM patch, a mod file, a save to
load), it's usually waiting on your slot's game page: see
[Your game files](#your-game-files) below. Most games connect directly and
need no file.

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

#### Your game files

When a season is generated, Archipelago produces a patch or output file for
each game that needs one. Bifrost picks out each slot's files from the seed
(by the slot number in Archipelago's usual file names) and lists them on that
slot's game page under **Your game files**, with a download link, the file
type and its size. Once you've claimed your slot, download them there and use
them with your game's client or patcher.

- Only players who've claimed a seat on the slot (every seat, for a co-op
  slot) and the season's admins can see and download them.
- After a re-roll, the page offers the new seed's files; the old ones no
  longer fit the new game.
- The spoiler log and the server's own file are never offered to players.
- The common game's page shows its own setup steps (client and mod
  downloads) instead.
- If your game needs a file and none is listed, ask the season's moderators.
  A world that names its files unusually can't be matched to a slot; the
  moderators see such files in the season console's *Player files* tab and
  can hand yours over.

### Bonus hints: minesweeper

If the season's moderators turned it on, **minesweeper** on the season board
earns bonus hints. Each board you clear without hitting a mine earns one
bonus hint, up to five a season in any 24 hours. Every board can be solved
without guessing. Bonus hints are spent automatically when you're out of hint
points.

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
- **Player files.** The per-player output files in the current seed, for
  download, plus the seed's spoiler log (for admins only). Players get their
  own files on their game page, so there's usually no need to send them out by
  hand. A file whose name doesn't say which slot it's for is listed here too,
  marked as not matched to a slot: hand that one to the right player
  yourself. Bifrost also logs such files after each generation.
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

**Container (recommended).** The image, published on `ghcr.io` with every
release (`latest`, or a version like `0.1.0`), contains Bifrost, a pinned
Archipelago build and bubblewrap:

```sh
podman run -d --name bifrost --stop-timeout 30 --security-opt unmask=/proc/* \
    -p 8080:8080 -p 38281-38300:38281-38300 -v bifrost-data:/var/lib/bifrost \
    ghcr.io/vidarrio/bifrost:latest    # or docker run
podman logs bifrost                  # the first-run /setup link
```

- `--security-opt unmask=/proc/*` lets the sandbox mount its own `/proc`.
  Without it, Bifrost falls back to a weaker sandbox and refuses custom
  apworlds. Docker needs `--security-opt systempaths=unconfined` for the same,
  plus `--security-opt seccomp=unconfined`, because Docker's default seccomp
  filter blocks the namespace calls bubblewrap makes (Podman's allows them),
  and `--security-opt apparmor=unconfined` on hosts that use AppArmor. Those
  turn Docker's own filters off for the whole container, Bifrost included,
  leaving bubblewrap and Bifrost's own syscall filter to isolate the apworld
  code; Podman needs neither. The console's settings page shows which sandbox
  tier is active.
- `--stop-timeout 30` gives live servers time to save on shutdown.
- Publish the whole Archipelago port range. Its size caps how many seasons can
  be live at once.
- To build the image yourself instead: `podman build -t bifrost .` in a
  checkout, then run `bifrost` in place of the published image.

**Native single file (Linux x86_64).** A static binary with its own Python and
Archipelago appended. It unpacks them into the data directory on first run.
Download `bifrost-<version>-x86_64-linux` and `SHA256SUMS` from a
[release](https://github.com/vidarrio/bifrost/releases), then:

```sh
sha256sum -c SHA256SUMS
chmod +x bifrost-<version>-x86_64-linux
./bifrost-<version>-x86_64-linux
```

To keep it, install it under a plain name, such as `/opt/bifrost/bifrost`, and
run it from a service (there's a systemd unit under
[Running on Windows](#running-on-windows) that works on any Linux). For it to
update itself, the user Bifrost runs as must own the file and be able to write
to its folder (`bifrost update` writes the new file next to it), so a
root-owned folder like `/usr/local/bin` won't do.
`bifrost update` keeps whatever name the file has, so a name with a version in
it would go stale.

To build it yourself instead: `packaging/bundle/build.sh` in a checkout puts it
in `dist/`.

Both the image and the file carry build provenance from the release workflow;
`gh attestation verify <file> -R vidarrio/bifrost` (or
`gh attestation verify oci://ghcr.io/vidarrio/bifrost:<version> -R vidarrio/bifrost`)
checks that they were built by this repository from the release's tag.

Without `BIFROST_DATA`, it keeps its data in `~/.local/share/bifrost`, or in
`/var/lib/bifrost` when run as root. Install `bubblewrap` on the host for the
strong sandbox, which custom apworlds need.

Put Bifrost behind a reverse proxy that terminates HTTPS. Session cookies are
marked `Secure`. Add the proxy's address to `trusted_proxies` so per-client
rate limiting sees real client addresses. Bifrost listens on every interface
by default; with the proxy on the same host, set `bind = "127.0.0.1"`
(`BIFROST_BIND`) so nothing reaches Bifrost around the proxy. In the
container, leave `bind` alone and publish the web port on the host's loopback
instead (`-p 127.0.0.1:8080:8080`). The Archipelago port range still has to be
reachable by players.

### Running on Windows

Bifrost needs Linux, but a Windows PC can host it through
[WSL2](https://learn.microsoft.com/windows/wsl/), which runs a real Linux
kernel. These steps haven't been tested on Windows by the project yet; please
report what does or doesn't work in an issue. Whichever route you take, check
afterwards that **Console → Settings** shows the bubblewrap sandbox tier,
which custom apworlds need.

Install WSL2 from an administrator PowerShell, then restart:

```powershell
wsl --install            # Ubuntu by default
```

**Container, through Docker Desktop.** Install
[Docker Desktop](https://docs.docker.com/desktop/setup/install/windows-install/)
with its WSL2 backend, turn on *Start Docker Desktop when you sign in*, and
run in PowerShell:

```powershell
docker run -d --name bifrost --restart unless-stopped --stop-timeout 30 `
    --security-opt seccomp=unconfined --security-opt systempaths=unconfined `
    -p 8080:8080 -p 38281-38300:38281-38300 -v bifrost-data:/var/lib/bifrost `
    ghcr.io/vidarrio/bifrost:latest
docker logs bifrost      # the first-run /setup link
```

The two `--security-opt` flags let bubblewrap run inside the container (see
[Installing](#installing)). The data stays in the named volume, inside Docker
Desktop's Linux VM. The container only runs while someone is signed in to
Windows, since Docker Desktop is a desktop app. Podman Desktop also runs the
image (with the Podman flags from [Installing](#installing)), but on Windows
it publishes ports on `127.0.0.1` only, so other machines can't reach it.

**Native single file, inside the WSL2 distro.** This route runs without anyone
signed in and needs Windows 11 22H2 or later to be reachable from other
machines (see below). In the Ubuntu shell, systemd must be on: it is in a fresh
Ubuntu from `wsl --install`; otherwise put `[boot]` / `systemd=true` in
`/etc/wsl.conf` and run `wsl --shutdown` from PowerShell. Then download the
release straight into Ubuntu and install it as a service:

```sh
v=0.1.0    # the release you want
curl -LO https://github.com/vidarrio/bifrost/releases/download/v$v/bifrost-$v-x86_64-linux
curl -LO https://github.com/vidarrio/bifrost/releases/download/v$v/SHA256SUMS
sha256sum -c SHA256SUMS
sudo apt update && sudo apt install bubblewrap
sudo useradd --system --home-dir /var/lib/bifrost --shell /usr/sbin/nologin bifrost
sudo install -d -o bifrost -m 700 /var/lib/bifrost
sudo install -d -o bifrost /opt/bifrost
sudo install -o bifrost -m 755 bifrost-$v-x86_64-linux /opt/bifrost/bifrost
sudo tee /etc/systemd/system/bifrost.service >/dev/null <<'UNIT'
[Unit]
Description=Bifrost
Wants=network-online.target
After=network-online.target

[Service]
User=bifrost
Environment=BIFROST_DATA=/var/lib/bifrost
ExecStart=/opt/bifrost/bifrost
Restart=on-failure
# Stop Bifrost first and let it shut the Archipelago servers down (they save);
# systemd's default would stop them all at once.
KillMode=mixed
TimeoutStopSec=30

[Install]
WantedBy=multi-user.target
UNIT
sudo systemctl enable --now bifrost
journalctl -u bifrost     # the first-run /setup link
```

The same unit works on any Linux with systemd. Keep the data directory on the
Linux side (as above), not under `/mnt/c`: files on the Windows drive are much
slower from WSL, and Linux permissions don't hold there.

**Reaching it from other machines.** Docker Desktop publishes the ports on
Windows itself. WSL2's own network sits behind NAT by default, so for the
native file turn on mirrored networking in `%UserProfile%\.wslconfig`, then
run `wsl --shutdown`:

```ini
[wsl2]
networkingMode=mirrored
```

Mirrored networking needs Windows 11 22H2 or later. On older Windows, use the
Docker Desktop route instead. Either way, allow the web port and the whole
Archipelago port range through the Windows firewall, from an administrator
PowerShell:

```powershell
New-NetFirewallRule -DisplayName Bifrost -Direction Inbound -Protocol TCP `
    -LocalPort 8080,38281-38300 -Action Allow
# Native file with mirrored networking: WSL's own firewall too
New-NetFirewallHyperVRule -Name Bifrost -DisplayName Bifrost -Direction Inbound `
    -VMCreatorId '{40E0AC32-46A5-438A-A0B2-2B479E8F2E90}' -Protocol TCP `
    -LocalPorts 8080,38281-38300 -Action Allow
```

Players outside your network also need those ports forwarded on your router.
Set the **connect host** under *Console → Settings* to the address they reach,
and put Bifrost behind a reverse proxy for HTTPS as on Linux.

**Keeping it running.** WSL2 starts with the first `wsl` command, not with
Windows, and stops a little while after its last one exits. To start the
native service with Windows and keep it up, add a Task Scheduler task that
runs at startup, with *Run whether user is logged on or not*:

```text
wsl.exe -d Ubuntu --exec sleep infinity
```

Turn off sleep in Windows' power settings too: a sleeping PC disconnects every
player and pauses the seasons.

### First run

The log prints a one-time `/setup?token=…` link. Open it to create the first
sysadmin. Then:

1. **Console → Settings**: set the **connect host** players use. Optionally
   set a password note, a home link for the header pill, and Discord servers
   for the home page.
2. **Console → Users & links**: mint invites.
3. **Console → Seasons**: create a season and open its signups. Or hand that
   job to a moderator.

### Updating Bifrost

Bifrost's version numbers follow [semantic versioning](https://semver.org/).
Before 1.0, a new minor version (`0.2.0`) can bring new features or breaking
changes, and a patch version (`0.2.1`) brings fixes, and sometimes new
documentation, but no new features or behaviour changes. Each release's notes
are on the [releases page](https://github.com/vidarrio/bifrost/releases) and
in the [changelog](../CHANGELOG.md). Sysadmins see a banner in the console
when a newer release is out (see [Configuration](#configuration)).

**What an update does.** Updating means stopping Bifrost, swapping the image
or file, and starting it again. Everything else carries over:

- **The data directory and database.** On its first start the new version
  takes a pre-upgrade backup and then upgrades the database (see
  [Backups](#backups)). It logs the exact command to go back.
- **Live seasons.** Their Archipelago servers save and stop with Bifrost, and
  start again with it. Players' clients reconnect on their own after a few
  seconds, and the web pages work again once Bifrost is back. Give the container
  a stop timeout (`--stop-timeout 30`) or the service manager a stop delay, so
  the servers have time to save.
- **Archipelago versions.** Each season stays on the Archipelago version it
  was generated with, even when a new Bifrost bundles a newer one. If that
  version's install is gone (say, a new image no longer bundles it), Bifrost
  reinstalls it on start; only if that fails does the season start on the
  active version instead, with a warning in the log. New seasons use whichever
  version is active on the settings page.

Pick a quiet moment, such as between seasons or when few players are
connected, but there's no need to wait for a season to end.

**Container image.** Each release is tagged `X.Y.Z`, `X.Y` and `latest`
(a pre-release gets only its `X.Y.Z-…` tag).
Pinning `X.Y` (say `ghcr.io/vidarrio/bifrost:0.2`) gets fixes but no
surprise minor versions. To update, pull the image and recreate the container
with the same volume and flags:

```sh
podman pull ghcr.io/vidarrio/bifrost:0.2
podman stop bifrost && podman rm bifrost
podman run -d --name bifrost … ghcr.io/vidarrio/bifrost:0.2   # as before
```

To update automatically, run the container under systemd (a Quadlet
`.container` file) with the label `io.containers.autoupdate=registry`, and
enable `podman-auto-update.timer`. Podman then pulls the tag daily and
restarts the container when it moved. With Docker, Watchtower does the same.

**Native single file.** Update it in place:

```sh
sudo -u bifrost env BIFROST_DATA=/var/lib/bifrost /opt/bifrost/bifrost update        # the newest release
sudo -u bifrost env BIFROST_DATA=/var/lib/bifrost /opt/bifrost/bifrost update 0.2.1  # or a given version
sudo systemctl restart bifrost
```

`bifrost update` downloads the release's file and checks it against the
release's `SHA256SUMS`, whose signature must verify against the release key
built into Bifrost. It then puts the file in place of the running one and
keeps the old one next to it as `<name>.previous`. Run it as the user that
owns the file, with the service's data directory (`BIFROST_DATA`): it uses
the Python unpacked there, and with another data directory it would unpack a
fresh copy first. Bifrost keeps running the old version until you restart it.
The signature is made by the release workflow with a key only the project
holds, so a file swapped into a release, or a release published without that
key, is refused. Releases before signing started (0.1.0 to 0.1.2) can't be
installed this way; download those by hand. Those versions' own
`bifrost update` doesn't check signatures yet, so the first signed release is
best installed by hand too: download the file and `SHA256SUMS`, check its
provenance as described under [Installing](#installing), and put it in place
of the old one. From then on `bifrost update` checks every release. To check the file's build
provenance as well, download it yourself and verify it as described under
[Installing](#installing).

**Going back.** If a new version misbehaves, stop it, restore the
pre-upgrade backup it took, and start the old version. The new version's log
gives the restore command with the backup's full path; `bifrost backup list`
lists the backups by name (find `…-pre-upgrade-from-<old version>.zip`), and
they live in the backup directory (`$BIFROST_DATA/backups` unless moved).
Pass `restore` the full path:

```sh
bifrost backup restore /var/lib/bifrost/backups/bifrost-…-pre-upgrade-from-0.1.0.zip
```

With the container stopped, run the restore in a one-off container on the
same volume, then start the old image tag:

```sh
podman run --rm -v bifrost-data:/var/lib/bifrost ghcr.io/vidarrio/bifrost:0.1 \
    backup restore /var/lib/bifrost/backups/bifrost-…-pre-upgrade-from-0.1.0.zip
```

For the single file, move `<name>.previous` back into place. An older Bifrost refuses a data directory that a newer one has
already upgraded, so restore the backup first.

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
  GitHub limits anonymous lookups to 60 an hour per address; set
  `GITHUB_TOKEN` in Bifrost's environment to lift that.
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

Bifrost also backs up on the first start after an upgrade (or a downgrade),
before the new version touches the database, and logs the command to go back.
These are named after the version they go back to
(`…-pre-upgrade-from-0.1.0.zip`), and the newest three are kept regardless of
the retention settings. If that backup fails, Bifrost doesn't start when
the upgrade would change the database; free up space and start it again. An
older Bifrost refuses a data directory that a newer one has already upgraded:
run the newer version, or restore the pre-upgrade backup.

Only starting the server upgrades a data directory. Commands such as
`bifrost auth status` leave it as it is, and refuse to run on one whose
database belongs to another version; `bifrost backup list` and
`bifrost backup restore` always work.

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

Switching ends every session, so everyone signs in again afterwards.

Configure it under `[forward_auth]`:

- Bifrost trusts identity headers only from a configured proxy: a shared
  secret header the proxy adds (`secret_header` + `secret`), and/or the proxy's
  address in `trusted_proxies`. Every configured check must pass. With neither
  configured, nobody is trusted. Prefer the secret header: in some container
  and NAT setups every client appears to come from the same gateway address,
  and if that address is in `trusted_proxies`, a client that reaches Bifrost
  directly is trusted too. Binding Bifrost to `127.0.0.1` (see
  [Installing](#installing)) closes that path when the proxy runs on the same
  host.
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
PathRegexp(`^/s/[^/]*/+(admin|claim|hint|signup|game|prioritize|minesweeper|api|mc)`)
```

Match any season id there, not just digits, so that no spelling of a season
URL slips past the rule. Bifrost itself only accepts plain digits.

### Isolation

Anything that runs apworld code runs sandboxed. That covers YAML checks,
generation, seed decoding and the Archipelago servers. With bubblewrap
available, the sandbox gives each helper:

- no network (the servers excepted; the in-logic tracker reaches its season's
  server through a relayed socket);
- a read-only system;
- write access only to the season's own output;
- a syscall filter that refuses what Python has no use for and what widens the
  kernel's attack surface (new namespaces, mounts, kernel keyrings, BPF,
  `ptrace`, io_uring and the like, and sockets other than Unix, IP and netlink
  ones).

Without bubblewrap, Bifrost applies resource limits and the syscall filter
only, and refuses custom apworlds unless you explicitly set
`allow_unsandboxed_custom_worlds`. The settings page shows which tier is
active. Resource limits for checks and generation are under `[sandbox]`.

Every sandboxed job is also capped in processes and open files. The process
cap counts all of Bifrost's user's processes, so run Bifrost as a dedicated,
non-root user (the image and the systemd unit above do). For a firmer bound,
give Bifrost a cgroup process limit as well: `TasksMax=` in its systemd unit,
or `--pids-limit` on the container.

### Configuration

Settings come from the environment or from `$BIFROST_DATA/config.toml`. The
environment wins. Print a commented template of every setting with:

```sh
bifrost config example
```

Keep `config.toml` at `chmod 600`, since it can hold secrets. Page-level
settings, such as the connect host, home page, Discords, apworld approval and
Archipelago versions, are set in the console instead.

Once a day Bifrost asks GitHub whether a newer Bifrost has been released, and
sysadmins see a banner in the console when one has (**Settings** shows the
running version and what the last check found). It downloads nothing. Turn it
off with `[updates] check = false` (`BIFROST_UPDATE_CHECK=off`). Set
`GITHUB_TOKEN` to avoid GitHub's anonymous rate limit.

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
bifrost update [<version>]                replace the single-file build with the
                                          newest release (or <version>)
```

### Data directory

Everything mutable lives in one data directory: the database, seasons,
catalog, Archipelago installs, archives and backups (unless moved). Back it
up, or let the nightly backups do it, and keep it on persistent storage.
