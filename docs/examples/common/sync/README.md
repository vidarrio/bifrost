# Syncing a common game's server

Bifrost never reaches a common game's server. A small job on the server's host
keeps it in step instead, through the read-only export: make a token under
**Settings → Common-game export** in Bifrost's console.

`bifrost-common-sync` is that job, with two ways to apply what it reads. It
needs only Python 3.

- **`rcon`** talks to a running Minecraft server over RCON. It adds the
  season's allowlist to the server's whitelist and removes names that aren't
  on it anymore, except those in `KEEP`. Nothing restarts. Turn RCON on in the
  server's `server.properties` (`enable-rcon=true`, `rcon.password=…`), and
  keep its port off the network.
- **`copy`** writes files: the allowlist as text, JSON, or Minecraft's
  `whitelist.json` (it looks up the UUIDs from Mojang), and then runs `AFTER`
  if anything changed, for example a restart. It suits any game whose server
  reads its allowlist from a file.

Either way, with `SEED_DIR` set it also fetches the slot's seed files when the
season is generated or re-rolled, such as the `.apmc` the Minecraft randomizer
loads. A new seed needs a fresh world on the server; that part is yours.

## Setting it up with systemd

Install the script and the units:

```sh
install -m 755 bifrost-common-sync /usr/local/bin/
install -m 644 bifrost-common-sync@.service bifrost-common-sync@.timer \
  bifrost-common-copy@.service bifrost-common-copy@.timer /etc/systemd/system/
install -d -m 700 /etc/bifrost-common-sync
install -m 600 bifrost-common-sync.env /etc/bifrost-common-sync/vanilla.env
```

Fill in `/etc/bifrost-common-sync/vanilla.env`: Bifrost's address, the token,
the setup's key and the RCON password, or the copy settings. Then try it once
and start the timer:

```sh
systemctl start bifrost-common-sync@vanilla.service
journalctl -u bifrost-common-sync@vanilla
systemctl enable --now bifrost-common-sync@vanilla.timer
```

One instance per server: a second Minecraft server, say `atm10`, gets its own
`atm10.env` and `bifrost-common-sync@atm10.timer`.

For copy mode, use `bifrost-common-copy@vanilla.service` and
`bifrost-common-copy@vanilla.timer` the same way. The rcon service runs as a throwaway user that can only
reach the network and its own state. The copy service runs as root, so it can
write the server's files and run `AFTER`. Narrow it with `User=` and
`ReadWritePaths=` to what your server needs.

## Without systemd

The script reads only its environment, so cron works too:

```sh
* * * * * root sh -c 'set -a; . /etc/bifrost-common-sync/vanilla.env; exec bifrost-common-sync rcon'
```
