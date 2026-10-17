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
    # Loading the world registry can register setting keys AP would then autosave
    # into host.yaml at exit — a write the read-only sandbox refuses. Disable it.
    import settings
    settings.skip_autosave = True
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
    # Per receiving slot, where each of its items sits: {slot: {item_id: [[finder, loc_id], ...]}}.
    # Lets the hint box count copies of same-named items and drop fully-hinted ones.
    placements = {}
    for finder, locs in locations.items():
        for loc_id, tup in locs.items():
            item_id, recv = tup[0], tup[1]
            items_for[recv] = items_for.get(recv, 0) + 1
            if recv == finder:
                items_for_own[recv] = items_for_own.get(recv, 0) + 1
            placements.setdefault(recv, {}).setdefault(item_id, []).append([finder, loc_id])

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
            "placements": {
                str(item_id): locs for item_id, locs in placements.get(s, {}).items()
            },
        }
    print(json.dumps(out))


if __name__ == "__main__":
    main()
