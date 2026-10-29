"""Bifrost's AP fork: Bifrost's own connections (board tracker, points, hint,
admin relay, in-logic) don't announce joins/leaves/!admin; a real player
client still does. Runs on the e2e Archipelago install (a fork build), or QUIET_AP=<dir>;
`quiet.py upstream` runs the control on a plain upstream install."""
import asyncio
import json
import os
import re
import subprocess
import sys
import time

from lib import AP_DIR, DEV, Server, check, get, post, summary, wait_for

UPSTREAM = sys.argv[1:] == ["upstream"]  # control run: the plain release announces everything
AP = os.environ.get("QUIET_AP") or (os.path.join(DEV, "archipelago") if UPSTREAM else AP_DIR)
YAMLS = [b"name: Ann\ngame: ChecksFinder\nChecksFinder: {}\n", b"name: Bob\ngame: ChecksFinder\nChecksFinder: {}\n"]

_vf = os.path.join(AP, "BIFROST_VERSION")
VERSION = open(_vf).read().strip() if os.path.exists(_vf) else "0.6.8"
BASE = VERSION.split("-bifrost.")[0]

srv = Server("quiet", port=18186, ap_ports="39140-39142", extra_env={"BIFROST_AP_DIR": AP})
srv.start()
try:
    check("runs the expected build", f"Archipelago {VERSION} bundled" in srv.logtext())
    root = srv.setup_sysadmin()
    check("landing shows the upstream version", f"Archipelago {BASE}<" in get(root, "/").text or f"Archipelago {BASE} " in get(root, "/").text)
    post(root, "/admin/seasons", data=dict(name="Quiet"))
    sid = int(re.search(r"/s/(\d+)/admin", get(root, "/admin/seasons").text).group(1))
    post(root, f"/s/{sid}/admin/signups", data=dict(open="1"))
    for y in YAMLS:
        post(root, f"/s/{sid}/signup", files={"yaml": ("y.yaml", y)})
    post(root, f"/s/{sid}/admin/generate")
    t = wait_for(lambda: (x := get(root, f"/s/{sid}/admin").text) and ("Generated seed" in x or "Failed" in x) and x, 300, 3)
    check("generation", t and "Generated seed" in t)
    check("tracker connects", wait_for(lambda: "Ann" in get(root, f"/s/{sid}/board").text, 60))

    claim = get(root, f"/s/{sid}/claim").text
    ids = re.findall(r'name="slot_id" value="(\d+)"', claim)
    r = post(root, f"/s/{sid}/claim", data=dict(slot_id=ids[0], op="claim"))
    get(root, f"/s/{sid}/game/1")                       # points relay
    hint = lambda: (j := post(root, f"/s/{sid}/hint", data=dict(slot_id=ids[0], item="Map Width")).json()) \
        and "isn't live" not in j.get("message", "") and j                             # hint relay, once the tracker's in
    check("hint relay answers", wait_for(hint, 60, 2))

    # Bonus hints: out of AP points, a credit places exactly one hint; a name
    # that isn't one of this world's items is refused and the credit kept.
    import sqlite3
    def credits():
        with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as c:
            row = c.execute("SELECT credits FROM hint_credit WHERE season = ?", (sid,)).fetchone()
            return row[0] if row else 0
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as c:
        acct = c.execute("SELECT id FROM account WHERE username = 'root'").fetchone()[0]
        c.execute("INSERT INTO hint_credit (season, account_id, credits) VALUES (?, ?, 1)", (sid, acct))
    j = post(root, f"/s/{sid}/hint", data=dict(slot_id=ids[0], item="Everything")).json()
    check("a bonus hint for a non-item is refused", not j.get("ok") and "exact item name" in j.get("message", ""), j)
    check("…and keeps the credit", credits() == 1)
    # (Retried while the seed's metadata is still loading: until then nothing
    # can be pinned to a location, and the credit is kept.)
    bonus = lambda: (j := post(root, f"/s/{sid}/hint", data=dict(slot_id=ids[0], item="Map Width")).json()) \
        and "exact item name" not in j.get("message", "") and j
    j = wait_for(bonus, 90, 3) or {}
    check("a bonus hint is placed for one credit", j.get("ok") and "spent a bonus hint" in j.get("message", "") and credits() == 0, j)
    r = post(root, f"/s/{sid}/admin/console", data=dict(cmd="/players")).json()  # admin relay
    check("admin relay works", r.get("ok"), r)

    def inlogic_rows():
        import sqlite3
        with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as c:
            return c.execute("SELECT count(*) FROM inlogic WHERE season = ?", (sid,)).fetchone()[0]
    check("in-logic tracker ran", wait_for(lambda: inlogic_rows() >= 2, 120, 3))

    # A real player: AP's own client library, no Bifrost tag.
    port = re.search(r"127\.0\.0\.1:(391\d\d)", get(root, f"/s/{sid}/admin").text + srv.logtext()).group(1)
    def connect_as(name, extra):
        player = f'''
import asyncio, json, websockets
async def main():
    async with websockets.connect("ws://127.0.0.1:{port}") as ws:
        await ws.recv()
        await ws.send(json.dumps([dict({{"cmd": "Connect", "game": "", "name": "{name}", "uuid": "p",
            "version": {{"class": "Version", "major": 0, "minor": 6, "build": 8}}, "items_handling": 0,
            "tags": ["TextOnly"], "slot_data": False, "password": ""}}, **{extra!r})]))
        for _ in range(5):
            if "Connected" in await ws.recv(): break
        await ws.send(json.dumps([{{"cmd": "Say", "text": "!admin login guess"}}]))
        await asyncio.sleep(1)
asyncio.run(main())
'''
        p = subprocess.run([os.path.join(AP, "venv/bin/python"), "-c", player], capture_output=True, text=True, timeout=60)
        check(f"client {name} {extra} connects", p.returncode == 0, p.stderr[-500:])
    connect_as("Bob", {})                                        # a real player
    connect_as("Bob", {"tags": ["TextOnly", "Bifrost"]})         # spoofing the old tag
    connect_as("Bob", {"bifrost_key": "guess"})                  # guessing the key
    time.sleep(3)

    log = open(os.path.join(srv.data, "seasons", str(sid), "logs", "server.log")).read()
    joins = re.findall(r".*has (?:joined|left|stopped \w+ the game|.* the game)\..*", log)
    print("\n".join(joins[-10:]))
    if UPSTREAM:
        # Ann is the slot all of Bifrost's tools connect as.
        check("upstream announces Bifrost clients", any(l.startswith(tuple("0123456789")) and "Ann (" in l for l in joins))
        check("…including in-logic UT runs", sum("Ann (" in l and "tracking" in l and "has joined" in l for l in joins) >= 2)
        check("upstream echoes Bifrost's !admin", "Ann: !admin login" in log)
    else:
        check("Bifrost clients never announced", not any("Ann (" in l for l in joins), [l for l in joins if "Ann (" in l][:3])
        check("no !admin echo from Bifrost", "Ann: !admin login" not in log)
    check("player join announced", any("Bob" in l and "has joined" in l for l in joins))
    check("player leave announced", any("Bob" in l and ("left the game" in l or "stopped viewing" in l) for l in joins))
    check("spoofed tag and wrong key still announced", sum("Bob (" in l and "viewing" in l and "has joined" in l for l in joins) == 3, joins)
    check("players' !admin attempts still echoed", log.count("Bob: !admin login") == 3)
    check("hint request still echoed", "!hint Map Width" in log)
    # Another player's `!players` output (a broadcast) arriving meanwhile
    # isn't taken for a refusal: the hint stays placed and paid for.
    ap_port = re.search(r"127\.0\.0\.1:(391\d\d)", get(root, f"/s/{sid}/admin").text + srv.logtext()).group(1)
    spam = subprocess.Popen([os.path.join(AP, "venv/bin/python"), "-c", f'''
import asyncio, json, websockets
async def main():
    async with websockets.connect("ws://127.0.0.1:{ap_port}") as ws:
        await ws.recv()
        await ws.send(json.dumps([{{"cmd": "Connect", "game": "", "name": "Bob", "uuid": "s",
            "version": {{"class": "Version", "major": 0, "minor": 6, "build": 8}}, "items_handling": 0,
            "tags": ["TextOnly"], "slot_data": False, "password": ""}}]))
        for _ in range(20):
            await ws.send(json.dumps([{{"cmd": "Say", "text": "!players"}}]))
            await asyncio.sleep(0.5)
asyncio.run(main())
'''])
    time.sleep(2)
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as c:
        c.execute("UPDATE hint_credit SET credits = 1 WHERE season = ?", (sid,))
    j = post(root, f"/s/{sid}/hint", data=dict(slot_id=ids[0], item="Map Height")).json()
    spam.wait(30)
    check("…even while another player sends !players", j.get("ok") and credits() == 0, j)
finally:
    srv.stop()

sys.exit(summary())
