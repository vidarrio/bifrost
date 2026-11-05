# Syncing a common game's server

Bifrost never reaches a common game's server. Something on the server's host
keeps it in step instead, through Bifrost's read-only export: make a token
under **Settings → Common-game export**. What that something is depends on the
game and how you run it, so it's yours to write. Here are two ways to go
about it, as starting points.

The export, all of it sent with `Authorization: Bearer <token>`:

- `GET /export/commons` lists the live seasons' common games as JSON: each
  season's id and seed name, and each common game's key, address, allowlist
  (`null` for a setup that takes no names) and seed files. Its `url` fields
  are paths, to put after Bifrost's address.
- `GET /export/<season>/<key>/allowlist` gives one name per line (or JSON,
  with `?format=json`).
- `GET /export/<season>/<key>/file?name=…` gives one of the slot's seed files;
  each file in the index has its `url` ready. A season's seed name changes when
  it's re-rolled: that's when the server needs its new file.

Keep the token out of command lines, where other users can read it with `ps`:
put the whole header in a file only the job can read, and hand curl the file.

```sh
printf 'Authorization: Bearer %s\n' "$TOKEN" > /etc/bifrost-export.hdr
chmod 600 /etc/bifrost-export.hdr    # then chown it to the job's user
```

Use `https://`, and don't follow redirects with the token (plain `curl`
doesn't). A season's id changes every season, so look it up by the setup's key
rather than writing it down; this takes the first live season that has it:

```sh
curl -fsS -H @/etc/bifrost-export.hdr "$BIFROST_URL/export/commons" |
  jq -r --arg key "$COMMON_KEY" \
    'first(.seasons[] | .commons[] | select(.key == $key) | .allowlist.url) // empty'
```

## Copying a file into place, on a timer

For a server that reads its allowlist from a file, fetch it and swap it in.
Some games need a format of their own (Minecraft's `whitelist.json` wants
UUIDs), so convert as you go.

```sh
#!/bin/sh
# /usr/local/bin/fetch-allowlist: BIFROST_URL, COMMON_KEY and OUT come from
# the environment; the token's header from /etc/bifrost-export.hdr.
set -eu
hdr=/etc/bifrost-export.hdr
url=$(curl -fsS -H @$hdr "$BIFROST_URL/export/commons" |
  jq -r --arg key "$COMMON_KEY" \
    'first(.seasons[] | .commons[] | select(.key == $key) | .allowlist.url) // empty')
[ -n "$url" ] || exit 0    # no live season has it
curl -fsS -H @$hdr "$BIFROST_URL$url" -o "$OUT.new"
mv "$OUT.new" "$OUT"
```

Run it every few minutes with a systemd timer, as the server's own user, and
start it with `systemctl enable --now fetch-allowlist.timer`:

```ini
# /etc/systemd/system/fetch-allowlist.service
[Unit]
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
User=gameserver
EnvironmentFile=/etc/fetch-allowlist.env
ExecStart=/usr/local/bin/fetch-allowlist

# /etc/systemd/system/fetch-allowlist.timer
[Timer]
OnBootSec=1min
OnUnitActiveSec=5min

[Install]
WantedBy=timers.target
```

## Applying it to a running server, over RCON

For a Minecraft server with RCON on (`enable-rcon=true` and an
`rcon.password` in `server.properties`), add each name with an RCON client
such as [mcrcon](https://github.com/Tiiffi/mcrcon), which takes the password
from `MCRCON_PASS` rather than the command line. Only send names the game
takes: a setup's own name rule may be looser, and anything else could change
what the command does.

```sh
# $url as looked up above; MCRCON_PASS set in the environment.
curl -fsS -H @/etc/bifrost-export.hdr "$BIFROST_URL$url" |
while read -r name; do
  case $name in *[!A-Za-z0-9_]*|"") continue ;; esac
  mcrcon -H 127.0.0.1 "whitelist add $name"
done
```

Removing names that have left the list, picking up a re-rolled seed, or
restarting the server after a change are for your script to add as your
server needs them.
