#!/usr/bin/env python3
"""Bifrost games helper. Lists the games an Archipelago install provides, as
JSON: {"games": [{"game": name, "version": "x.y.z" or null}, ...]}.

Run under the install's venv with BIFROST_SEASON_DIR set to an empty scratch
dir, so only the install's own worlds load (no uploaded world). A world's
version is the one its manifest declares, when it declares one.

Usage: bifrost_games.py"""
import json
import sys

try:
    import bifrost_ap  # noqa: F401  (pins AP paths; must precede AP imports)
    from worlds import AutoWorldRegister
except Exception as e:  # pragma: no cover
    print(json.dumps({"error": "games init failed: %s" % e}))
    sys.exit(0)


def version_of(world):
    v = getattr(world, "world_version", None)
    if v is None:
        return None
    parts = tuple(v) if isinstance(v, tuple) else None
    if parts is not None and not any(parts):
        return None  # 0.0.0: the default, nothing declared
    return ".".join(str(p) for p in parts) if parts is not None else str(v)


def main() -> None:
    games = []
    for name, world in AutoWorldRegister.world_types.items():
        if getattr(world, "hidden", False):
            continue
        games.append({"game": name, "version": version_of(world)})
    games.sort(key=lambda g: g["game"].casefold())
    print(json.dumps({"games": games}))


if __name__ == "__main__":
    main()
