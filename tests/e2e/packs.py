"""PopTracker packs run in a worker of their own: a good pack uploads and
tracks, and one that never finishes or prints without end costs a time limit,
not the server."""
import io
import os
import shutil
import sys
import tempfile
import threading
import time
import zipfile

import requests

from lib import BIN, Server, check, descendants, get, post, summary, wait_for

YAML = b"name: Ann\ngame: ChecksFinder\nChecksFinder: {}\n"
ITEMS = '[{"name":"Key","type":"toggle","img":"x.png","codes":"key"}]'
LOCATIONS = '[{"name":"Door","access_rules":["key"],"sections":[{"name":"S","item_count":1}]}]'
LOAD = """
Tracker:AddItems("items.json")
Tracker:AddLocations("locations.json")
"""
GOOD = LOAD + """
print("this goes nowhere")
Archipelago:AddClearHandler("clear", function(data)
  print(string.rep("noise", 1000))
  Tracker:FindObjectForCode("key").Active = data.player_name == "Ann"
    and Archipelago:GetPlayerGame(1) == "ChecksFinder"
end)
"""
SPIN = "while true do pcall(function() while true do end end) end"
BACKTRACK = 'string.find(string.rep("a", 5000), string.rep("a*", 20) .. "b")'
NOISE = 'while true do print(string.rep("noise", 100000)) end'
HANDLER = LOAD + 'Archipelago:AddClearHandler("clear", function() ' + SPIN + " end)\n"


def pack(init):
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        z.writestr("scripts/init.lua", init)
        z.writestr("items.json", ITEMS)
        z.writestr("locations.json", LOCATIONS)
    return buf.getvalue()


def live_season(srv):
    root = srv.setup_sysadmin()
    post(root, "/admin/seasons", data=dict(name="Packs"))
    post(root, "/s/1/admin/signups", data=dict(open="1"))
    post(root, "/s/1/signup", files={"yaml": ("y.yaml", YAML)})
    post(root, "/s/1/admin/generate")
    wait_for(lambda: "Generated seed" in get(root, "/s/1/admin").text, 300, 3)
    wait_for(lambda: "Ann" in get(root, "/s/1/board").text, 60, 2)
    wait_for(lambda: get(root, "/s/1/game/1").status_code == 200, 60, 2)
    return root


def upload(root, init):
    return post(root, "/s/1/game/1/poptracker", files={"pack": ("p.zip", pack(init))})


def state(root, **kw):
    return get(root, "/s/1/game/1/poptracker/state", **kw).json()


def tracked(root):
    d = state(root)
    return d if "state" in d and d["state"]["items"][0]["active"] else None


def workers(srv):
    out = []
    for p in descendants(srv.proc.pid):
        try:
            if b"pack-worker" in open(f"/proc/{p}/cmdline", "rb").read():
                out.append(p)
        except OSError:
            pass
    return out


def scratch(srv):
    try:
        return os.listdir(os.path.join(srv.data, "tmp/packs"))
    except FileNotFoundError:
        return []


# A copy of the binary, to replace while it runs, as `bifrost update` does.
work = tempfile.mkdtemp(prefix="bifrost-e2e-packs-")
exe = os.path.join(work, "bifrost")
shutil.copy2(BIN, exe)
srv = Server("packs", port=18196, ap_ports="39181-39183", extra_env={"BIFROST_PACK_TIMEOUT_SECS": "5"}, bin=exe)
srv.start()
try:
    root = live_season(srv)
    r = upload(root, GOOD)
    check("a good pack uploads", "Pack rejected" not in r.text, r.text[:300])
    d = wait_for(lambda: tracked(root), 60, 2)
    check("it tracks, with the slot data and players, whatever it prints", d, state(root))
    t = time.time()
    polls = [state(root) for _ in range(30)]
    check("polls of an unchanged slot share one result", all("state" in p for p in polls) and time.time() - t < 3, time.time() - t)

    for name, code in (("loops past the instruction limit", SPIN), ("hangs in a pattern", BACKTRACK), ("prints without end", NOISE)):
        started = time.time()
        r = upload(root, LOAD + code + "\n" + 'Archipelago:AddClearHandler("c", function() end)')
        took = time.time() - started
        check(f"a pack that {name} is refused", "Pack rejected" in r.text and "didn't finish" in r.text, r.text[:300])
        check("…at the time limit", took < 20, took)
    check("a refused pack leaves the good one installed", wait_for(lambda: tracked(root), 30, 2), state(root))
    check("uploads leave no scratch behind", wait_for(lambda: not scratch(srv), 10, 1), scratch(srv))

    r = upload(root, 'error(string.rep("\\1", 3000000), 0)')
    check("a long error is cut short", "Pack rejected" in r.text and len(r.text) < 100000, len(r.text))

    # Going away mid-upload costs nothing lasting.
    try:
        root.post(root.base + "/s/1/game/1/poptracker", files={"pack": ("p.zip", pack(LOAD + SPIN))}, timeout=1)
    except requests.RequestException:
        pass
    check("an abandoned upload's worker stops", wait_for(lambda: not workers(srv), 15, 1))
    check("…and its scratch goes", wait_for(lambda: not scratch(srv), 15, 1), scratch(srv))

    # One that only misbehaves once it's fed passes the upload's check.
    r = upload(root, HANDLER)
    check("a pack that hangs only when connected uploads", "Pack rejected" not in r.text, r.text[:300])
    results = []
    first = threading.Thread(target=lambda: results.append(state(root)))
    first.start()
    wait_for(lambda: workers(srv), 5, 0.1)
    t = time.time()
    check("the server answers while one runs", get(root, "/s/1/board").status_code == 200 and time.time() - t < 3)
    check("…with a worker running", workers(srv))
    check("a second viewer gets busy, not a second worker", state(root).get("busy"))
    first.join()
    check("its tracker reports the time limit", results and "didn't finish" in results[0].get("error", ""), results)
    check("no worker is left running", wait_for(lambda: not workers(srv), 10, 1))
    t = time.time()
    d = state(root)
    check("it isn't run again on the next poll", "didn't finish" in d.get("error", "") and time.time() - t < 2 and not workers(srv), d)

    # A run belongs to the slot: a viewer going away neither stops nor restarts it.
    upload(root, HANDLER)
    try:
        state(root, timeout=1)
    except requests.RequestException:
        pass
    check("a viewer leaving doesn't stop the run", workers(srv))
    check("…and others share it", state(root).get("busy"))
    d = wait_for(lambda: (s := state(root)) and "error" in s and s, 15, 1)
    check("…and see how it ended", d and "didn't finish" in d["error"], d)

    # After an update replaced the binary, until the restart.
    os.remove(exe)
    shutil.copy2(BIN, exe)
    r = upload(root, GOOD)
    check("a replaced binary still runs packs, and a new upload clears the hold", "Pack rejected" not in r.text, r.text[:300])
    check("…and tracks", wait_for(lambda: tracked(root), 30, 2), state(root))
finally:
    srv.stop()
    shutil.rmtree(work, ignore_errors=True)

# Without bubblewrap, the worker has limits and the time limit all the same.
srv = Server("packs-limits", port=18197, ap_ports="39184-39186", extra_env={"BIFROST_PACK_TIMEOUT_SECS": "5", "BIFROST_SANDBOX": "none"})
srv.start()
try:
    root = live_season(srv)
    r = upload(root, GOOD)
    check("without bubblewrap, a good pack uploads", "Pack rejected" not in r.text, r.text[:300])
    check("…and tracks", wait_for(lambda: tracked(root), 60, 2), state(root))
    r = upload(root, LOAD + SPIN)
    check("…and a pack that doesn't finish is refused", "didn't finish" in r.text, r.text[:300])
finally:
    srv.stop()

sys.exit(summary())
