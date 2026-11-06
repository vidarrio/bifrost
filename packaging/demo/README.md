# A public demo

A demo build lets anyone try Bifrost without an account. A bar on every page
switches between three accounts, `player`, `moderator` and `sysadmin`, with no
password. It is built for a demo that is open to the internet, so it assumes
every visitor can be sysadmin:

- **Turned off**, whoever asks:
  - the instance settings, including the links shown on every page, Archipelago
    installs, backups and the export token;
  - apworld uploads, common-game edits and PopTracker pack uploads;
  - season imports;
  - role changes, disabling accounts, and invite and reset links.

  The pages still show these, with their buttons greyed out.
- **Capped:** at most 12 seasons, and 12 games in a season. Generation runs one
  at a time, as always.
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

```sh
export BIFROST_DATA=/srv/bifrost-demo/seed BIFROST_DEMO_DIRECT=1
target/release/bifrost &                        # wait for "listening"
packaging/demo/seed.py http://127.0.0.1:8080 "$BIFROST_DATA"
kill %1; wait
cp -a /srv/bifrost-demo/seed /srv/bifrost-demo/snapshot
```

The seed makes three seasons:

- an archived one;
- a live one with a common game and minesweeper;
- one taking signups, run by the moderator.

It generates the first two, which takes a minute or two.

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
