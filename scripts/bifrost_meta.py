#!/usr/bin/env python3
"""Bifrost season-metadata helper. Decodes the seed (`output/current.zip`) with
AP's own loader and emits, as JSON, the static facts the live board needs but
the network protocol doesn't carry:

  - per AP slot: world_size (items placed in their world), items_for (items
    destined for them anywhere), items_for_elsewhere (those outside their world);
  - item/location id -> name maps for each game in play (for the feed + hints).

Run under the AP venv; pins local/user path so custom worlds resolve (same
reason as bifrost_check.py)."""
import sys
import os
import json
import zlib
import zipfile

AP_DIR = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, AP_DIR)

try:
    import Utils
    Utils.local_path.cached_path = AP_DIR
    Utils.user_path.cached_path = AP_DIR
    from Utils import restricted_loads
    from worlds import AutoWorldRegister
except Exception as e:  # pragma: no cover
    print(json.dumps({"error": "meta init failed: %s" % e}))
    sys.exit(0)


def main() -> None:
    path = sys.argv[1]
    with zipfile.ZipFile(path) as zf:
        member = next(f for f in zf.namelist() if f.endswith(".archipelago"))
        data = zf.read(member)
    decoded = restricted_loads(zlib.decompress(data[1:]))

    slot_info = decoded["slot_info"]                # {slot: NetworkSlot}
    locations = decoded["locations"]                # {slot: {loc_id: (item, recv, flags)}}

    world_size = {s: len(locs) for s, locs in locations.items()}
    items_for = {}
    items_for_own = {}
    for finder, locs in locations.items():
        for _loc_id, tup in locs.items():
            recv = tup[1]
            items_for[recv] = items_for.get(recv, 0) + 1
            if recv == finder:
                items_for_own[recv] = items_for_own.get(recv, 0) + 1

    games = sorted({si.game for si in slot_info.values()})
    item_names = {}
    location_names = {}
    for game in games:
        world = AutoWorldRegister.world_types.get(game)
        if not world:
            continue
        item_names[game] = {str(v): k for k, v in world.item_name_to_id.items()}
        location_names[game] = {str(v): k for k, v in world.location_name_to_id.items()}

    out = {
        "seed": decoded.get("seed_name"),
        "slots": {},
        "item_names": item_names,
        "location_names": location_names,
    }
    for s, si in slot_info.items():
        out["slots"][str(s)] = {
            "name": si.name,
            "game": si.game,
            "world_size": world_size.get(s, 0),
            "items_for": items_for.get(s, 0),
            "items_for_elsewhere": items_for.get(s, 0) - items_for_own.get(s, 0),
        }
    print(json.dumps(out))


if __name__ == "__main__":
    main()
