"""Bifrost's AP fork: Bifrost's own connections (board tracker, points, hint,
admin relay, in-logic) don't announce joins/leaves/!admin; a real player
client still does. Runs on the e2e Archipelago install (a fork build), or QUIET_AP=<dir>;
`quiet.py upstream` runs the control on a plain upstream install."""
import asyncio
import json
import os
import re
import signal
import subprocess
import sys
import time

from lib import AP_DIR, DEV, Server, ap_servers, check, first_game, get, new_season, post, summary, wait_for

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
    sid = new_season(root, "Quiet")
    post(root, f"/s/{sid}/admin/signups", data=dict(open="1"))
    for y in YAMLS:
        post(root, f"/s/{sid}/signup", files={"yaml": ("y.yaml", y)})
    post(root, f"/s/{sid}/admin/generate")
    t = wait_for(lambda: (x := get(root, f"/s/{sid}/admin").text) and ("Generated seed" in x or "Failed" in x) and x, 300, 3)
    check("generation", t and "Generated seed" in t)
    check("tracker connects", wait_for(lambda: "Ann" in get(root, f"/s/{sid}/board").text, 60))
    check("the board's join tab shows the upstream version", f"Archipelago {BASE}</strong>" in get(root, f"/s/{sid}/board").text)

    claim = get(root, f"/s/{sid}/claim").text
    ids = re.findall(r'name="slot_id" value="([0-9a-z]+)"', claim)
    get(root, f"/s/{sid}/game/{first_game(root, sid)}")                       # points relay
    hint = lambda: (j := post(root, f"/s/{sid}/hint", data=dict(slot_id=ids[0], item="Map Width")).json()) \
        and "isn't live" not in j.get("message", "") and j                             # hint relay, once the tracker's in
    check("hint relay answers", wait_for(hint, 60, 2))

    # Bonus hints: out of AP points, a credit places exactly one hint; a name
    # that isn't one of this world's items is refused and the credit kept.
    import sqlite3
    def credits():
        with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as c:
            row = c.execute("SELECT credits FROM hint_credit WHERE season_id = ?", (sid,)).fetchone()
            return row[0] if row else 0
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as c:
        acct = c.execute("SELECT id FROM account WHERE username = 'root'").fetchone()[0]
        c.execute("INSERT INTO hint_credit (season_id, account_id, credits) VALUES (?, ?, 1)", (sid, acct))
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
            return c.execute("SELECT count(*) FROM inlogic WHERE season_id = ?", (sid,)).fetchone()[0]
    check("in-logic tracker ran", wait_for(lambda: inlogic_rows() >= 2, 120, 3))

    # The game page's Universal Tracker tab follows a new result without a reload.
    game = first_game(root, sid)
    page = get(root, f"/s/{sid}/game/{game}").text
    check("the UT tab polls for its contents", 'id="utbody"' in page and f"game/'+slot+'/inlogic" in page)
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as c:
        (slot,) = c.execute("SELECT ap_slot FROM slot WHERE season_id = ? AND game_id = ?", (sid, game)).fetchone()
        # (The tracker only rewrites a row when the slot's items or checks change.)
        n = c.execute("UPDATE inlogic SET locations = '[\"Somewhere New\"]', count = 1, err = NULL WHERE season_id = ? AND ap_slot = ?", (sid, slot)).rowcount
    check("…with a result to replace", n == 1, n)
    j = get(root, f"/s/{sid}/game/{game}/inlogic").json()
    check("…and gets the latest result", "Somewhere New" in j.get("body", "") and j.get("avail") == " · 1 available", j)

    # Its header figures and hints list follow the game too.
    check("the game page polls its figures and hints", 'id="gamestats"' in page and f"game/'+slot+'/live" in page)
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as c:
        (hinted,) = c.execute("SELECT game_id FROM slot WHERE id = ?", (ids[0],)).fetchone()
    live = lambda: get(root, f"/s/{sid}/game/{hinted}/live").json()
    j = wait_for(lambda: (x := live()) and "Map Width" in x.get("hints", "") and x, 60, 3) or live()
    check("…which carry the slot's hints", "Map Width" in j.get("hints", ""), j)
    stats = re.search(r'id="gamestats"[^>]*>([^<]*)<', get(root, f"/s/{sid}/game/{hinted}").text)
    j0 = get(root, f"/s/{sid}/game/{hinted}/live", params={"hints": "0"}).json()
    check("…and leaves the hints out when their tab is hidden", j0.get("hints") is None and j0.get("stats") == j.get("stats"), j0)
    check("…and the header's figures", stats and re.fullmatch(r"\d+ locations · \d+ left · sent \d+ · received \d+", j.get("stats", "")) and j["stats"] == stats.group(1), (j.get("stats"), stats and stats.group(1)))

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
        c.execute("UPDATE hint_credit SET credits = 1 WHERE season_id = ?", (sid,))
    j = post(root, f"/s/{sid}/hint", data=dict(slot_id=ids[0], item="Map Height")).json()
    spam.wait(30)
    check("…even while another player sends !players", j.get("ok") and credits() == 0, j)

    if not UPSTREAM:
        # A slot's status is its player's. The tracker sits on the lowest slot.
        with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as c:
            (tracked, _, _), (other, other_slot, other_id) = c.execute(
                "SELECT name, ap_slot, id FROM slot WHERE season_id = ? ORDER BY ap_slot", (sid,)).fetchall()
        print("tracker on", tracked, "- watching", other, other_slot)
        def client(name, script):
            return subprocess.Popen([os.path.join(AP, "venv/bin/python"), "-u", "-c", f'''
import asyncio, json, websockets
async def main():
    async with websockets.connect("ws://127.0.0.1:{ap_port}") as ws:
        await ws.recv()
        await ws.send(json.dumps([{{"cmd": "Connect", "game": "ChecksFinder", "name": "{name}", "uuid": "c",
            "version": {{"class": "Version", "major": 0, "minor": 6, "build": 8}}, "items_handling": 0,
            "tags": [], "slot_data": False, "password": ""}}]))
{script}
asyncio.run(main())
'''], stdout=subprocess.PIPE, text=True)
        def card(name):
            for part in get(root, f"/s/{sid}/board").text.split('class="slot cardlink ')[1:]:
                if f'<span class="who">{name}</span>' in part.split('class="slot cardlink')[0]:
                    return part.split('"')[0]
        check("nobody on the tracker's slot: no status", wait_for(lambda: card(tracked) == "idle", 20), card(tracked))
        p = client(tracked, """
        await asyncio.sleep(5)
        await ws.send(json.dumps([{"cmd": "StatusUpdate", "status": 20}]))
        await asyncio.sleep(5)
""")
        check("a player there shows connected", wait_for(lambda: card(tracked) == "conn", 5, 0.5), card(tracked))
        check("…then playing", wait_for(lambda: card(tracked) == "playing", 8, 0.5), card(tracked))
        p.wait(30)
        check("…and clears when they leave, with the tracker still on it", wait_for(lambda: card(tracked) == "idle", 8, 0.5), card(tracked))

        # Bifrost's short connections don't touch a status, watched from another slot.
        key = f"_read_client_status_0_{other_slot}"
        watch = client(tracked, f"""
        await ws.send(json.dumps([{{"cmd": "SetNotify", "keys": ["{key}"]}}]))
        print("ready", flush=True)
        try:
            while True:
                print(await asyncio.wait_for(ws.recv(), 10), flush=True)
        except asyncio.TimeoutError:
            pass
""")
        watch.stdout.readline()
        j = post(root, f"/s/{sid}/hint", data=dict(slot_id=other_id, item="Map Width")).json()
        print("second slot's hint:", j)
        check("the watched slot's connection ran", j.get("ok") or "afford" in j.get("message", ""), j)
        seen = watch.communicate(timeout=40)[0]
        check("…and the watcher saw it", watch.returncode == 0 and "!hint Map Width" in seen, seen[-300:])
        check("Bifrost's connection didn't make a slot connected", key not in seen, seen[-300:])

        # A server killed with a player on: their status doesn't outlast it.
        p = client(tracked, """
        await ws.send(json.dumps([{"cmd": "StatusUpdate", "status": 20}]))
        await asyncio.sleep(300)
""")
        check("a player is playing", wait_for(lambda: card(tracked) == "playing", 8, 0.5), card(tracked))
        saved = post(root, f"/s/{sid}/admin/console", data=dict(cmd="/save")).json()
        check("…and saved", "Game saved" in json.dumps(saved), saved)
        os.kill(ap_servers(srv)[0][0], signal.SIGKILL)
        p.kill()
        check("…and not after the server's killed", wait_for(lambda: card(tracked) == "idle", 90, 2), card(tracked))
finally:
    srv.stop()

sys.exit(summary())
