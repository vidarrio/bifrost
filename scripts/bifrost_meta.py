#!/usr/bin/env python3
"""Bifrost season-metadata helper. Decodes the seed (`output/current.zip`) with
AP's own loader and emits, as JSON, the static facts the live board needs but
the network protocol doesn't carry:

  - per AP slot: world_size (items placed in their world), items_for (items
    destined for them anywhere), items_for_elsewhere (those outside their world);
  - item/location id -> name maps for each game in play (for the feed + hints);
  - per AP slot: its slot data, as AP sends it to the slot's clients on connect
    (for the slot's PopTracker pack).

Run under the AP venv; the season's custom worlds resolve via bifrost_ap.

Usage: bifrost_meta.py <seed.zip>"""
import sys
import json
import zlib
import zipfile

try:
    import bifrost_ap  # noqa: F401  (pins AP paths; must precede AP imports)
    from Utils import restricted_loads
    from NetUtils import encode
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
    precollected = decoded.get("precollected_items", {})  # {slot: [item_id, ...]} starting inventory
    slot_data = decoded.get("slot_data", {})        # {slot: {...}} sent to the slot's clients on connect

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

    # Per finder slot, the classification flags of the item at each of its
    # locations, keyed by LOCATION NAME: {slot_name: {location_name: flags}}.
    # Flags bits: 0b001 progression, 0b010 useful, 0b100 trap (0 = filler). The
    # finder is the slot whose world the location is in — i.e. the sender of a
    # "sent" log line — so the activity ingester can classify a sent item from
    # (sender name, location name) with no id juggling.
    loc_flags = {}
    for finder, locs in locations.items():
        si = slot_info.get(finder)
        id2name = location_names.get(si.game) if si else None
        if not id2name:
            continue
        m = {}
        for loc_id, tup in locs.items():
            name = id2name.get(str(loc_id))
            if name is not None:
                m[name] = tup[2] if len(tup) > 2 else 0
        if m:
            loc_flags[si.name] = m

    out = {
        "seed": decoded.get("seed_name"),
        "slots": {},
        "item_names": item_names,
        "location_names": location_names,
        "loc_flags": loc_flags,
    }
    def slot_json(s):
        # One slot's slot data as AP encodes it for the network. Data AP can't
        # send (or JSON can't hold, like NaN) costs only that slot: its pack
        # gets {}, as if the world sent none.
        try:
            return json.loads(json.dumps(json.loads(encode(slot_data.get(s, {}))), allow_nan=False))
        except Exception as e:
            print("slot %s: slot data not kept: %s" % (s, e), file=sys.stderr)
            return {}

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
            # Every location id this slot actually has (the seed's full set for
            # the slot). Lets the PopTracker engine apply `visibility_rules` to
            # hide checks the slot's options never generated.
            "all_locations": sorted(int(loc_id) for loc_id in locations.get(s, {})),
            # The slot's starting inventory (precollected items). The network
            # protocol delivers these on connect so a live tracker autotracks
            # them, but they're absent from the savefile's received_items — feed
            # them to the engine too, or season gates (e.g. "summer") read false.
            "precollected": [int(i) for i in precollected.get(s, [])],
            # The slot's slot data, encoded as AP encodes it for the network, so
            # the slot's PopTracker pack gets on connect what it would from AP.
            "slot_data": slot_json(s),
        }
    print(json.dumps(out))


if __name__ == "__main__":
    main()
