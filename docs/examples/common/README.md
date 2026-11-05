# Common-game setups

Examples of setups for the catalog of common games, one folder each. Copy a
folder into the data directory's `common/`, rename it to the key you want, and
change it to match your server. See "Common games" under Sysadmins in the
[user guide](../../USER_GUIDE.md) for what each file does.

- `minecraft-26.2/`: Minecraft Java Edition with NeoForge and the Archipelago
  Randomizer mod. It doesn't include the apworld or the client jars: add
  `minecraft.apworld` and the files under `client/` that `game.toml` lists.
- `sync/`: keeping a server in step with Bifrost through its export, over
  Minecraft's RCON or by copying files into place, with systemd units.
