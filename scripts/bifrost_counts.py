#!/usr/bin/env python3
"""Authoritative per-slot sent/received totals, decoded from the running
server's save (`output/current.apsave`).

An external Archipelago tracker only sees item events that happen while it is
connected, so a long-lived board started mid-season (or restarted) massively
undercounts. The server itself keeps the true tallies in the savefile:

  * sent     = locations the slot has checked   -> len(location_checks[(0, slot)])
  * received = items the slot has been granted  -> sum(len) over received_items[(0, slot, *)]

Output: one JSON line, {"<slot>": {"sent": n, "received": n}, ...}.
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

received = {}
for key, items in received_items.items():
    if isinstance(key, tuple) and len(key) >= 2 and key[0] == 0:
        received[key[1]] = received.get(key[1], 0) + len(items)

out = {
    str(slot): {"sent": sent.get(slot, 0), "received": received.get(slot, 0)}
    for slot in set(sent) | set(received)
}
print(json.dumps(out))
