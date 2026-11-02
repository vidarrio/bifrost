"""PopTracker packs run in a worker of their own: a good pack uploads and
tracks, and one that never finishes or prints without end costs a time limit,
not the server."""
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import zipfile

import requests

from lib import AP_DIR as AP, BIN, DEV, REPO, Server, check, descendants, first_game, get, new_season, post, summary, wait_for

YAML = b"name: Ann\ngame: ChecksFinder\nChecksFinder: {}\n"
ITEMS = """[{"name":"Key","type":"toggle","img":"x.png","codes":"key"},
{"name":"Own hints","type":"toggle","img":"x.png","codes":"own_hints"},
{"name":"Leak","type":"toggle","img":"x.png","codes":"leak"}]"""
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
  Archipelago:Get({"_read_hints_0_1", "_read_hints_0_2", "_read_race_mode"})
end)
-- Its own hints come back as a list, however empty; the server's other keys as nothing.
Archipelago:AddRetrievedHandler("hints", function(key, value)
  if key == "_read_hints_0_1" and type(value) == "table" then
    Tracker:FindObjectForCode("own_hints").Active = true
  elseif key ~= "_read_hints_0_1" and value ~= nil then
    Tracker:FindObjectForCode("leak").Active = true
  end
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
    global sid, gid
    root = srv.setup_sysadmin()
    sid = new_season(root, "Packs")
    post(root, f"/s/{sid}/admin/signups", data=dict(open="1"))
    post(root, f"/s/{sid}/signup", files={"yaml": ("y.yaml", YAML)})
    post(root, f"/s/{sid}/admin/generate")
    wait_for(lambda: "Generated seed" in get(root, f"/s/{sid}/admin").text, 300, 3)
    wait_for(lambda: "Ann" in get(root, f"/s/{sid}/board").text, 60, 2)
    gid = wait_for(lambda: first_game(root, sid), 60, 2)
    return root


def upload(root, init):
    return post(root, f"/s/{sid}/game/{gid}/poptracker", files={"pack": ("p.zip", pack(init))})


def state(root, **kw):
    return get(root, f"/s/{sid}/game/{gid}/poptracker/state", **kw).json()


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


def helper_output(save):
    """Run the save-reading helper on a save made here, under the bundled AP."""
    d = tempfile.mkdtemp(dir=DEV)
    script = (
        "import pickle, sys, zlib; sys.path.insert(0, sys.argv[2]); "
        "from NetUtils import Hint, HintStatus; "
        "open(sys.argv[1], 'wb').write(zlib.compress(pickle.dumps(eval(sys.argv[3]))))"
    )
    path = os.path.join(d, "save")
    py = os.path.join(AP, "venv/bin/python")
    subprocess.run([py, "-c", script, path, AP, save], check=True, cwd=AP, capture_output=True)
    env = dict(os.environ, BIFROST_AP_DIR=AP, BIFROST_SEASON_DIR=d, PYTHONPATH=os.path.join(REPO, "scripts"))
    r = subprocess.run([py, "-P", os.path.join(REPO, "scripts/bifrost_counts.py"), path], capture_output=True, text=True, env=env, cwd=d, check=True)
    shutil.rmtree(d, ignore_errors=True)
    return r.stdout.strip().splitlines()[-1]


hints = "{(0, 1): {Hint(1, 2, l, 6, False, str(l), 1, HintStatus.HINT_PRIORITY) for l in range(40)}}"
save = "{'location_checks': {(0, 1): {5}}, 'received_items': {}, 'hints': %s, 'stored_data': {'ok': {'a': 1}, '_read_x': 1, 'nan': float('nan'), 'big': 'x' * 300000, 'surrogate': '\\ud800', '\\ud800': 1, 'huge': 10 ** 400}}" % hints
line = helper_output(save)
out = json.loads(line)
check("the helper gives a slot's hints, in the server's form", out["slots"]["1"]["hints"][0]["class"] == "Hint" and out["slots"]["1"]["hints"][0]["status"] == 30, line[:300])
check("…in a stable order", [h["location"] for h in out["slots"]["1"]["hints"]] == sorted(h["location"] for h in out["slots"]["1"]["hints"]))
check("stored values are text of their own, without the server's keys or oversized ones", json.loads(out["stored"]["ok"]) == {"a": 1} and "_read_x" not in out["stored"] and "big" not in out["stored"] and "nan" not in out["stored"], sorted(out["stored"]))
check("a key that can't be encoded is dropped", "\ud800" not in out["stored"])
line.encode("utf-8")

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
    held = {c for i in (d or {}).get("state", {}).get("items", []) if i["active"] for c in i["codes"]}
    check("it reads its own hints, and no one else's", "own_hints" in held and "leak" not in held, held)
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

    # A manifest that overflows the parser's stack is the worker's problem alone.
    for depth in (5000, 20000):
        buf = io.BytesIO()
        with zipfile.ZipFile(buf, "w") as z:
            z.writestr("scripts/init.lua", GOOD)
            z.writestr("items.json", ITEMS)
            z.writestr("locations.json", LOCATIONS)
            z.writestr("x.png", "image")
            z.writestr("manifest.json", '{"variants":' + "[" * depth + "]" * depth + "}")
        post(root, f"/s/{sid}/game/{gid}/poptracker", files={"pack": ("p.zip", buf.getvalue())})
        r = get(root, f"/s/{sid}/game/{gid}/poptracker/asset", params={"file": "x.png"})
        check(f"a manifest {depth} levels deep can't bring the server down", srv.proc.poll() is None and r.status_code in (200, 404), r.status_code)
    upload(root, GOOD)

    # Going away mid-upload costs nothing lasting.
    try:
        root.post(root.base + f"/s/{sid}/game/{gid}/poptracker", files={"pack": ("p.zip", pack(LOAD + SPIN))}, timeout=1)
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
    check("the server answers while one runs", get(root, f"/s/{sid}/board").status_code == 200 and time.time() - t < 3)
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
