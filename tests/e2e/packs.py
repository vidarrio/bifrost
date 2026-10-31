"""PopTracker packs run in a worker of their own: a good pack uploads and
tracks, and one that never finishes or prints without end costs a time limit,
not the server."""
import io
import sys
import threading
import time
import zipfile

from lib import Server, check, descendants, get, post, summary, wait_for

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


srv = Server("packs", port=18196, ap_ports="39181-39183", extra_env={"BIFROST_PACK_TIMEOUT_SECS": "5"})
srv.start()
try:
    root = srv.setup_sysadmin()
    post(root, "/admin/seasons", data=dict(name="Packs"))
    post(root, "/s/1/admin/signups", data=dict(open="1"))
    post(root, "/s/1/signup", files={"yaml": ("y.yaml", YAML)})
    post(root, "/s/1/admin/generate")
    wait_for(lambda: "Generated seed" in get(root, "/s/1/admin").text, 300, 3)
    wait_for(lambda: "Ann" in get(root, "/s/1/board").text, 60, 2)
    wait_for(lambda: get(root, "/s/1/game/1").status_code == 200, 60, 2)

    def upload(init):
        return post(root, "/s/1/game/1/poptracker", files={"pack": ("p.zip", pack(init))})

    def state():
        return get(root, "/s/1/game/1/poptracker/state").json()

    def workers():
        out = []
        for p in descendants(srv.proc.pid):
            try:
                if b"pack-worker" in open(f"/proc/{p}/cmdline", "rb").read():
                    out.append(p)
            except OSError:
                pass
        return out

    def tracked():
        d = state()
        return d if "state" in d and d["state"]["items"][0]["active"] else None

    r = upload(GOOD)
    check("a good pack uploads", "Pack rejected" not in r.text, r.text[:300])
    d = wait_for(tracked, 60, 2)
    check("it tracks, with the slot data and players, whatever it prints", d, state())

    for name, code in (("loops past the instruction limit", SPIN), ("hangs in a pattern", BACKTRACK), ("prints without end", NOISE)):
        started = time.time()
        r = upload(LOAD + code + "\n" + 'Archipelago:AddClearHandler("c", function() end)')
        took = time.time() - started
        check(f"a pack that {name} is refused", "Pack rejected" in r.text and "didn't finish" in r.text, r.text[:300])
        check("…at the time limit", took < 20, took)
    check("a refused pack leaves the good one installed", wait_for(tracked, 30, 2), state())

    # One that only misbehaves once it's fed passes the upload's check.
    r = upload(HANDLER)
    check("a pack that hangs only when connected uploads", "Pack rejected" not in r.text, r.text[:300])
    d = wait_for(lambda: (s := state()) and "error" in s and s, 30, 1)
    check("its tracker reports the time limit", d and "didn't finish" in d["error"], d)
    busy = threading.Thread(target=state)
    busy.start()
    time.sleep(1)
    t = time.time()
    check("the server answers while one runs", get(root, "/s/1/board").status_code == 200 and time.time() - t < 3)
    check("…with a worker running", workers())
    busy.join()
    check("no worker is left running", wait_for(lambda: not workers(), 20, 1))
finally:
    srv.stop()

sys.exit(summary())
