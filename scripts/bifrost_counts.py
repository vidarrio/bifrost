#!/usr/bin/env python3
"""Authoritative per-slot sent/received state, decoded from the running
server's save (`output/current.apsave`).

An external Archipelago tracker only sees item events that happen while it is
connected, so a long-lived board started mid-season (or restarted) massively
undercounts. The server itself keeps the true tallies in the savefile:

  * sent  = locations the slot has checked  -> len(location_checks[(0, slot)])
  * items = items the slot has been granted -> received_items[(0, slot, *)], each
            a NetworkItem(item, location, player=finder, flags); we emit
            [item, location, finder] so the board can show every found item with
            its source location, same format as hints. received count = len(items).

Output: one JSON line, {"<slot>": {"sent": n, "items": [[item, loc, finder], ...]}, ...}.
"""
import sys
import json
import zlib

sys.path.insert(0, "/srv/archipelago")
from Utils import restricted_loads  # noqa: E402
import NetUtils  # noqa: E402,F401  (registers Hint/NetworkItem for unpickling)

save_path = sys.argv[1] if len(sys.argv) > 1 else "/srv/archipelago/output/current.apsave"

try:
    with open(save_path, "rb") as f:
        save = restricted_loads(zlib.decompress(f.read()))
except FileNotFoundError:
    print("{}")
    sys.exit(0)

location_checks = save.get("location_checks", {})  # {(team, slot): set(loc)}
received_items = save.get("received_items", {})    # {(team, slot, remote_bool): [NetworkItem, ...]}

sent = {}
for key, locs in location_checks.items():
    if isinstance(key, tuple) and len(key) >= 2 and key[0] == 0:
        sent[key[1]] = len(locs)

# received_items is keyed (team, slot, remote). send_items_to() appends EVERY
# received item to the (slot, True) bucket but only remote-origin items to
# (slot, False) — so (slot, False) is a subset and summing both double-counts.
# Use the True bucket (the full list); fall back to False if a slot somehow
# lacks it. Each NetworkItem is (item, location, player=finder, flags).
true_buckets = {}
false_buckets = {}
for key, items in received_items.items():
    if isinstance(key, tuple) and len(key) >= 2 and key[0] == 0:
        slot = key[1]
        remote = key[2] if len(key) >= 3 else True
        (true_buckets if remote else false_buckets)[slot] = items

items_by_slot = {}
for slot in set(true_buckets) | set(false_buckets):
    bucket = true_buckets.get(slot, false_buckets.get(slot, []))
    items_by_slot[slot] = [[it.item, it.location, it.player] for it in bucket]

out = {
    str(slot): {"sent": sent.get(slot, 0), "items": items_by_slot.get(slot, [])}
    for slot in set(sent) | set(items_by_slot)
}
print(json.dumps(out))
