# Syncing a common game's server

Bifrost never reaches a common game's server. Something on the server's host
keeps it in step instead, through Bifrost's read-only export: make a token
under **Settings → Common-game export**. What that something is depends on the
game and how you run it, so it's yours to write. Here are two ways to go
about it, as starting points.

The export, all of it sent with `Authorization: Bearer <token>`:

- `GET /export/commons` lists the live seasons' common games as JSON: each
  season's id and seed name, and each common game's key, address, allowlist
  URL and seed files' URLs.
- `GET /export/<season>/<key>/allowlist` gives one name per line (or JSON,
  with `?format=json`).
- `GET /export/<season>/<key>/file?name=…` gives one of the slot's seed files,
  by the name the index lists. A season's seed name changes when it's
  re-rolled: that's when the server needs its new file.

Keep the token in a file only root can read, use `https://`, and don't follow
redirects with it (plain `curl` doesn't).

## Copying a file into place, on a timer

For a server that reads its allowlist from a file, fetch it and swap it in.
Some games need a format of their own (Minecraft's `whitelist.json` wants
UUIDs), so convert as you go.

```sh
#!/bin/sh
# /usr/local/bin/fetch-allowlist: BIFROST_URL, BIFROST_EXPORT_TOKEN, SEASON,
# COMMON_KEY and OUT come from the environment.
set -eu
curl -fsS -H "Authorization: Bearer $BIFROST_EXPORT_TOKEN" \
  "$BIFROST_URL/export/$SEASON/$COMMON_KEY/allowlist" -o "$OUT.new"
mv "$OUT.new" "$OUT"
```

Run it every few minutes with a systemd timer, as the server's own user:

```ini
# /etc/systemd/system/fetch-allowlist.service
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
such as [mcrcon](https://github.com/Tiiffi/mcrcon). Only send names the game
takes: a setup's own name rule may be looser, and anything else in a command
is a command.

```sh
curl -fsS -H "Authorization: Bearer $BIFROST_EXPORT_TOKEN" \
  "$BIFROST_URL/export/$SEASON/$COMMON_KEY/allowlist" |
while read -r name; do
  case $name in *[!A-Za-z0-9_]*|"") continue ;; esac
  mcrcon -H 127.0.0.1 -p "$RCON_PASSWORD" "whitelist add $name"
done
```

Removing names that have left the list, picking up a re-rolled seed, or
restarting the server after a change are for your script to add as your
server needs them.
