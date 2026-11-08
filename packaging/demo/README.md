# A public demo

A demo build lets anyone try Bifrost without an account. A bar on every page
switches between three accounts, `player`, `moderator` and `sysadmin`, with no
password; until a visitor picks one, a note above the bar points to it. It is
built for a demo that is open to the internet, so it assumes
every visitor can be sysadmin:

- **Turned off**, whoever asks:
  - the instance settings, including the links shown on every page, Archipelago
    installs, backups and the export token;
  - apworld uploads, common-game edits and PopTracker pack uploads;
  - season imports;
  - role changes, disabling accounts, and invite and reset links.

  The pages still show these, with their buttons greyed out.
- **Capped:** at most 12 seasons, 12 games in a season and 4 seasons live at
  once (each runs a game server of a couple of hundred MB). Generation runs one
  at a time, as always.
- **The demo's own seasons stay as they are.** When it starts from a snapshot,
  the seasons in it can be looked at, administration included, but not
  changed: no deleting, regenerating, archiving or closing them. Everything
  works on a season a visitor makes.
- **Game servers listen on localhost only**, whatever `BIFROST_AP_BIND` says.
  The boards and trackers work; visitors can't connect a game client.
- **Native sign-in and managed game servers only.** A demo build refuses to
  start in forward-auth mode or with `BIFROST_AP_MODE=external`.
- **Rate limits are per address**, since visitors share three accounts. Behind
  a reverse proxy, that only works if Bifrost trusts the proxy to say who's
  asking, so a demo build won't start without `BIFROST_TRUSTED_PROXIES`. Set
  `BIFROST_DEMO_DIRECT=1` instead only when visitors reach it with no proxy in
  between, as the seed below does on localhost.
- **Reset from a snapshot.** With `BIFROST_DEMO_SNAPSHOT`, every start replaces
  the data dir with a copy of the snapshot. With `BIFROST_DEMO_RESET_MINUTES` (above 0, at
  most a week, or it won't start), Bifrost stops after that long, and the service manager starts it again. The
  bar counts down to the reset.

Release builds never include any of this. CI checks that the image and the
bundle don't answer on `/demo`.

What visitors can still do is shared with every other visitor until the next
reset: season names, signups, notes, hints. Keep the reset interval short.

## Build it

```sh
cargo build --release --features demo          # target/release/bifrost
# or the image:
docker build --build-arg FEATURES=demo -t bifrost:demo .
```

Every push to `main` also publishes the image as `ghcr.io/vidarrio/bifrost:demo`
(and `:demo-<short commit>`), built by `.github/workflows/demo.yml`. It's a
demo build: never run it as a real instance. Release tags (`latest`, `X.Y.Z`,
`X.Y`) are never demo builds.

## Make the snapshot

Start the demo build on an empty data dir, seed it, stop it and keep a copy.
Use the build you'll run: for the image, seed a container of it the same way.
The seeding instance needs:

- `BIFROST_DEMO_DIRECT=1`, since nothing proxies it;
- `BIFROST_INSECURE_COOKIES=1`, since the seed talks to it over plain HTTP and
  a `Secure` session cookie isn't sent back there;
- for a container, a data dir owned by the image's user (`podman run --rm
  --entrypoint id <image>` shows it), and `BIFROST_BIND=127.0.0.1` if it uses
  the host's network, so a passwordless instance is never reachable from
  outside;
- when it shares a host or network namespace with a running Bifrost (such as
  the demo you serve), its own `BIFROST_PORT` and `BIFROST_AP_PORTS`. The seed
  waits for the archived season's game server, which can't start on a port
  the other instance holds. Point the seed at that `BIFROST_PORT` too.

Only the seeding instance gets these. The demo you serve keeps its
`Secure` cookie behind HTTPS.

```sh
export BIFROST_DATA=/srv/bifrost-demo/seed BIFROST_DEMO_DIRECT=1 BIFROST_INSECURE_COOKIES=1
target/release/bifrost &                        # wait for "listening"
packaging/demo/seed.py http://127.0.0.1:8080 "$BIFROST_DATA"
kill %1; wait
cp -a /srv/bifrost-demo/seed /srv/bifrost-demo/snapshot
```

The seed makes three seasons, with real games that need no ROM:

- an archived one, where three of the four games finished;
- a live one with a common game: one game finished, the others part played,
  with a few hints out;
- one taking signups, run by the moderator.

It plays the first two through their season console, which the demo allows
only on seasons it didn't start with, and it reads which locations to check
from the spoiler in the data dir. It also writes a few weeks of minesweeper
results, general and for both seasons, straight into the database, since a
result's time is how long the board took to play. All this takes about eight
minutes.

Since it reads the data dir and writes to the database, run the seed as a user
who can: the instance's own user, or root. Bifrost keeps the data dir to
itself. For a container, run the seed as root on the host.

## Run it

`BIFROST_DATA` and `BIFROST_DEMO_SNAPSHOT` must be separate directories. The
data dir is wiped on every start.

With systemd:

```ini
[Service]
Environment=BIFROST_DATA=/srv/bifrost-demo/data
Environment=BIFROST_DEMO_SNAPSHOT=/srv/bifrost-demo/snapshot
Environment=BIFROST_DEMO_RESET_MINUTES=60
Environment=BIFROST_UPDATE_CHECK=off
Environment=BIFROST_TRUSTED_PROXIES=<your proxy's address>
ExecStart=/usr/local/bin/bifrost
Restart=always
```

With the image, mount the snapshot read-only and let the container restart:

```sh
docker run -d --restart always -p 8080:8080 \
  -v /srv/bifrost-demo/snapshot:/snapshot:ro \
  -e BIFROST_DEMO_SNAPSHOT=/snapshot -e BIFROST_DEMO_RESET_MINUTES=60 \
  -e BIFROST_UPDATE_CHECK=off -e BIFROST_TRUSTED_PROXIES=<your proxy's address> \
  bifrost:demo
```

Give the demo a machine, VM or container of its own: no other services, no
real Bifrost data, no keys, and CPU, memory and disk limits of its own. With
`BIFROST_UPDATE_CHECK=off` it fetches nothing, so its outbound network can stay
closed too.
Visitors see what the console shows any sysadmin: the data and backup paths,
the host's load, and the Archipelago versions installed.

Put it behind your usual HTTPS reverse proxy. `BIFROST_TRUSTED_PROXIES` is the
proxy's address as Bifrost sees it. In a container published with `-p`, that's
often the bridge's gateway (such as `172.17.0.1`) rather than the proxy host's
own address. A wrong value still starts, but every visitor then shares one rate
limit again: the sign that it's wrong is that "Too many requests" reaches
everyone at once. Only Bifrost's web port needs to
be reachable. Leave the game port range closed.
