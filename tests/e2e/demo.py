"""The demo build (`cargo build --features demo`): role switching without
passwords, what it turns off, its caps, localhost-only game servers, and the
reset from a snapshot. Seeds itself with packaging/demo/seed.py.

Runs the demo binary at BIFROST_E2E_DEMO_BIN, or else builds one into
target/demo (`cargo build --features demo --target-dir target/demo`).
"""
import json
import os
import re
import shutil
import socket
import sqlite3
import threading
import subprocess
import sys
import time

import requests

from lib import DEV, REPO, Server, check, get, post, summary, wait_for

BIN = os.environ.get("BIFROST_E2E_DEMO_BIN")
if not BIN:  # build it, in a target dir of its own so it never replaces the normal build
    subprocess.run(["cargo", "build", "-q", "--features", "demo", "--target-dir", "target/demo"], cwd=REPO, check=True)
    BIN = os.path.join(REPO, "target/demo/debug/bifrost")
YAML = "name: {}\ngame: ChecksFinder\nChecksFinder: {{}}\n"


def as_role(srv, role):
    s = requests.Session()
    s.base = srv.base
    r = s.post(f"{srv.base}/demo/as/{role}?rd=/signups", allow_redirects=False)
    assert r.status_code == 303, (role, r.status_code, r.text[:200])
    return s


PROXIED = {"BIFROST_TRUSTED_PROXIES": "127.0.0.1"}
srv = Server("demo", port=18200, ap_ports="39200-39209", bin=BIN, extra_env={"BIFROST_AP_BIND": "0.0.0.0", **PROXIED})
srv.start()
try:
    log = srv.logtext()
    check("no first-run setup in a demo build", "/setup?token=" not in log, log[-400:])
    check("the start warns it's a demo build", "DEMO BUILD" in log)
    r = requests.get(srv.base + "/")
    check("every response says it's a demo build", r.headers.get("x-bifrost-demo") == "1")
    check("pages carry the switcher bar", "demobar" in r.text and "/demo/as/" in r.text)
    check("and the note that points visitors to it", "demohint" in r.text and "Pick an account below" in r.text)

    admin, mod, player = (as_role(srv, r) for r in ("sysadmin", "moderator", "player"))
    for s, name, is_admin in ((admin, "sysadmin", True), (mod, "moderator", True), (player, "player", False)):
        me = get(s, "/api/me").json()
        check(f"/demo/as/{name} signs in as {name}", me.get("user") == name and me.get("admin") == is_admin, me)
    r = requests.post(srv.base + "/demo/as/player?rd=//evil.example", allow_redirects=False)
    check("…and only sends you back to a local page", r.headers.get("Location") == "/", r.headers.get("Location"))
    check("no other accounts to switch to", requests.post(srv.base + "/demo/as/root", allow_redirects=False).status_code == 404)
    check("switching isn't a link", requests.get(srv.base + "/demo/as/player", allow_redirects=False).status_code == 405)
    r = requests.post(srv.base + "/demo/as/sysadmin", headers={"Origin": "https://elsewhere.example", "Sec-Fetch-Site": "cross-site"}, allow_redirects=False)
    check("…nor a form on another site", r.status_code == 403 and "set-cookie" not in r.headers, r.status_code)
    s = as_role(srv, "player")
    old = s.cookies.get("bifrost_session")
    s.post(srv.base + "/demo/as/moderator", allow_redirects=False)
    xff = lambda ip: requests.post(srv.base + "/demo/as/player", headers={"X-Forwarded-For": ip}, allow_redirects=False).status_code
    codes = [xff("203.0.113.7") for _ in range(31)]
    check("switching is rate limited per visitor", codes[:29].count(303) == 29 and codes[-1] == 429, codes)
    check("…by the address the proxy forwards", xff("203.0.113.8") == 303)
    check("switching ends the session it replaces", requests.get(srv.base + "/api/me", cookies={"bifrost_session": old}).status_code == 401)
    check("a player is still no sysadmin", get(player, "/admin/users").status_code in (302, 303, 401, 403)
          or "Sysadmins only" in get(player, "/admin/users").text)
    check("the moderator can't reach the instance settings", "Sysadmins only" in get(mod, "/admin/settings").text
          or get(mod, "/admin/settings").status_code in (302, 303, 401, 403))

    # What the demo turns off, even for its sysadmin.
    check("settings still show", get(admin, "/admin/settings").ok)
    off = lambda r: r.status_code == 403 and "turned off in the demo" in r.text
    check("instance settings can't change", off(post(admin, "/admin/settings", data=dict(domain="x", home_url="https://evil.example", discords=""))))
    check("…and didn't", get(admin, "/api/me").json().get("home") == "")
    check("no invite links", off(post(admin, "/admin/links/invite", data=dict(label="x", days=1))))
    check("no role changes", off(post(admin, "/admin/users/role", data=dict(id="1", role="user"))))
    check("no Archipelago installs", off(post(admin, "/admin/settings/ap/install", data=dict(version="0.6.8"))))
    check("no backups", off(post(admin, "/admin/settings/backups/now")))
    check("no backup downloads", off(get(admin, "/admin/settings/backups/download?name=x")))
    check("no export token", off(post(admin, "/admin/settings/export")))
    check("no season imports", off(post(admin, "/admin/seasons/import", files={"bundle": ("x.tar.gz", b"x")})))
    check("no apworld uploads", off(post(admin, "/admin/catalog/add", files={"file": ("x.apworld", b"PK")})))
    check("no common-game edits", off(post(admin, "/admin/common-games/new", data=dict(key="x", name="x", slot="x", yaml="x"))))
    r = post(player, "/signup/apworld", files={"file": ("x.apworld", b"PK")})
    check("no apworlds with a signup", r.json().get("ok") is False and "turned off" in r.json().get("error", ""), r.text[:200])

    # The seed, which also shows the rest works: seasons, signups,
    # generation, archiving, a common game.
    seed = subprocess.run([sys.executable, os.path.join(REPO, "packaging/demo/seed.py"), srv.base, srv.data],
                          capture_output=True, text=True, timeout=1800)
    check("the seed script runs", seed.returncode == 0, (seed.stdout + seed.stderr)[-800:])
    seasons = get(admin, "/admin/seasons").text
    check("…and makes its seasons", all(n in seasons for n in ("Summer season", "Autumn season", "Winter season")))
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as db:
        summer = json.loads(db.execute("SELECT summary FROM season WHERE name = 'Summer season'").fetchone()[0] or "{}")
        autumn = db.execute("SELECT id FROM season WHERE name = 'Autumn season'").fetchone()[0]
        mines = dict(db.execute("SELECT season_id IS NULL, COUNT(*) FROM mine_result GROUP BY 1").fetchall())
        prios = db.execute("SELECT COUNT(*) FROM hint_priority WHERE season_id = ?", (autumn,)).fetchone()[0]
    games = summer.get("games", [])
    check("…plays real games", {"A Short Hike", "Celeste 64"} <= {g["game"] for g in games}, [g["game"] for g in games])
    check("…finishing some of the archived season's", sum(g["pct"] == 100 for g in games) >= 3, [(g["game"], g["pct"]) for g in games])
    check("…and freezing its minesweeper leaderboards", any(m["time"] for m in summer.get("minesweeper", [])))
    board = get(admin, f"/s/{autumn}/board").text
    sent = [int(n) for n in re.findall(r"· sent (\d+) ·", board)]
    check("…has the live season part played", "goaled" in board and sum(n > 0 for n in sent) >= 4, sent)
    check("…with prioritized hints on its board", prios >= 2 and "wanted from you" in board, prios)
    check("…and fills the minesweeper leaderboards", mines.get(1, 0) >= 20 and mines.get(0, 0) >= 20, mines)
    check("game servers listen on localhost only", re.search(r"AP server started on 127\.0\.0\.1:\d+", srv.logtext())
          and not re.search(r"AP server started on 0\.0\.0\.0", srv.logtext()))
    check("the demo shows no reset time without one", get(admin, "/demo/status").json().get("reset_in") is None)

    # Caps: seasons, and a season's games.
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as db:
        have = db.execute("SELECT COUNT(*) FROM season").fetchone()[0]
    made = [post(admin, "/admin/seasons", data=dict(name=f"Extra {i}")) for i in range(12 - have)]
    r = post(admin, "/admin/seasons", data=dict(name="One too many"))
    check("at most 12 seasons", "at most 12 seasons" in r.text and "One too many" not in get(admin, "/admin/seasons").text, r.text[-300:])
    sid = re.search(r"/s/([0-9a-z]+)/admin", made[-1].url).group(1)
    post(admin, f"/s/{sid}/admin/signups", data=dict(open="1"))
    def signup(name):  # paced: signups are rate limited
        for _ in range(30):
            r = post(player, f"/s/{sid}/signup", files={"yaml": ("y.yaml", YAML.format(name).encode())})
            if r.status_code != 429:
                return r
            time.sleep(2)
        return r
    oks = sum('class="banner ok"' in signup(f"P{i}").text for i in range(12))
    r = signup("P12")
    check("a season takes 12 games", oks == 12, oks)
    check("at most 12 games in a season", "at most 12 games" in r.text, re.findall(r'class="banner[^"]*">[^<]*', r.text))

    # Live seasons: the seed leaves one, so of four generated at once, three
    # go live and one is refused.
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as db:
        summer = db.execute("SELECT state FROM season WHERE name = 'Summer season'").fetchone()[0]
    check("the seed archives its archived season", summer == "archived", summer)
    extra = [re.search(r"/s/([0-9a-z]+)/admin", m.url).group(1) for m in made[:4]]
    for i, e in enumerate(extra):
        post(admin, f"/s/{e}/admin/signups", data=dict(open="1"))
        # From an address of its own: the caps above used up this one's signups.
        post(admin, f"/s/{e}/signup", files={"yaml": ("y.yaml", YAML.format(f"L{i}").encode())},
             headers={"X-Forwarded-For": f"198.51.100.{i + 1}"})
    gens = [threading.Thread(target=post, args=(admin, f"/s/{e}/admin/generate")) for e in extra]
    for g in gens:
        g.start()
    for g in gens:
        g.join()
    def outcome(e):
        t = get(admin, f"/s/{e}/admin").text
        return "live" if "Generated seed" in t else "capped" if "at most 4 live seasons" in t else None
    wait_for(lambda: all(outcome(e) for e in extra), 600, 5)
    outcomes = [outcome(e) for e in extra]
    check("at most 4 live seasons, even generated at once", sorted(outcomes) == ["capped", "live", "live", "live"], outcomes)
    went_live = {e for e, o in zip(extra, outcomes) if o == "live"}

    # Leave room under the season cap for the snapshot below: delete the
    # extras that aren't live.
    for i, m in enumerate(made):
        e = re.search(r"/s/([0-9a-z]+)/admin", m.url).group(1)
        if e not in went_live:
            post(admin, f"/s/{e}/admin/season/delete", data=dict(name=f"Extra {i}"))
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as db:
        left = db.execute("SELECT COUNT(*) FROM season").fetchone()[0]
    check("the extras that aren't live can be deleted", left == have + 3, left)
finally:
    srv.stop()

# The snapshot: a demo that starts from it and resets back to it.
snap = os.path.join(DEV, "run", "demo-snapshot")
shutil.rmtree(snap, ignore_errors=True)
shutil.copytree(srv.data, snap, symlinks=True)
again = Server("demo-reset", port=18201, ap_ports="39210-39219", bin=BIN,
               extra_env={"BIFROST_DEMO_SNAPSHOT": snap, "BIFROST_DEMO_RESET_MINUTES": "0.5", "BIFROST_DEMO_DIRECT": "1"})
with open(os.path.join(again.data, "leftover"), "w") as f:
    f.write("from before the reset\n")
again.start()
try:
    admin = as_role(again, "sysadmin")
    check("a start restores the snapshot", "Autumn season" in get(admin, "/admin/seasons").text
          and not os.path.exists(os.path.join(again.data, "leftover")))
    with sqlite3.connect(os.path.join(again.data, "bifrost.db")) as db:
        autumn = db.execute("SELECT id FROM season WHERE name = 'Autumn season'").fetchone()[0]
    st = get(admin, "/demo/status").json()
    check("the bar learns which seasons are the demo's own", autumn in st.get("protected", []), st)
    r = post(admin, f"/s/{autumn}/admin/season/delete", data=dict(name="Autumn season"))
    check("the demo's own seasons can't be deleted", r.status_code == 403 and "stay as they are" in r.text, r.status_code)
    r = post(admin, f"/s/{autumn}/admin/regenerate")
    check("…or regenerated", r.status_code == 403)
    r = post(admin, "/admin/season/delete", data=dict(name="Autumn season"))
    check("…not even without the season in the address", r.status_code == 403 and "stay as they are" in r.text, r.status_code)
    game = re.search(r'href="game/([0-9a-z]+)"', get(admin, f"/s/{autumn}/board").text).group(1)
    r = post(admin, f"/s/{autumn}/game/{game}/finish")
    check("…nor a game in them finished", r.status_code == 403, r.status_code)
    r = post(admin, f"/s/{autumn}/admin/console", data=dict(cmd="/release Ann"))
    check("…nor items released from the console", r.json().get("ok") is False and "only runs" in r.json().get("output", ""), r.text[:200])
    r = post(admin, f"/s/{autumn}/hint", data=dict(slot_id="0", item="x"))
    check("…while playing in them still works", r.status_code != 403, r.status_code)
    # Signups on the demo's own season: a visitor's can be withdrawn, the seeded one stays.
    player = as_role(again, "player")
    with sqlite3.connect(os.path.join(again.data, "bifrost.db")) as db:
        winter = db.execute("SELECT id FROM season WHERE name = 'Winter season'").fetchone()[0]
        before = {r[0] for r in db.execute("SELECT id FROM submission WHERE season_id = ?", (winter,))}
        # The player's own seeded signup (the seed signs up others too).
        seeded = db.execute("SELECT s.id FROM submission s JOIN account a ON a.id = s.account_id "
                            "WHERE s.season_id = ? AND a.username = 'player'", (winter,)).fetchone()[0]
    r = post(player, f"/s/{winter}/signup", files={"yaml": ("y.yaml", YAML.format("Visitor").encode())})
    check("visitors can sign up to the demo's own season", 'banner ok' in r.text, re.findall(r'class="banner[^"]*">[^<]*', r.text))
    with sqlite3.connect(os.path.join(again.data, "bifrost.db")) as db:
        (mine,) = {r[0] for r in db.execute("SELECT id FROM submission WHERE season_id = ?", (winter,))} - before
    post(player, f"/s/{winter}/signup/delete", data=dict(id=mine))
    r = post(player, f"/s/{winter}/signup/delete", data=dict(id=seeded))
    check("withdrawing a seeded signup says why it can't", "stay as they are" in r.text, re.findall(r'class="banner[^"]*">[^<]*', r.text))
    with sqlite3.connect(os.path.join(again.data, "bifrost.db")) as db:
        left = {r[0] for r in db.execute("SELECT id FROM submission WHERE season_id = ?", (winter,))}
    check("…and withdraw their own signup, but not the seeded ones", left == before, (left, before))
    r = post(admin, f"/s/{winter}/signup/game/{seeded}/notes", data=dict(notes="defaced"))
    check("…nor rewrite its notes", "stay as they are" in r.text, re.findall(r'class="banner[^"]*">[^<]*', r.text))
    check("…but their pages still show", get(admin, f"/s/{autumn}/admin").ok and get(admin, f"/s/{autumn}/board").ok)
    r = post(admin, f"/s/{autumn}/admin/console", data=dict(cmd="/players"))
    check("…and the console still answers", r.ok and r.json().get("ok"), r.text[:200])
    mine = re.search(r"/s/([0-9a-z]+)/admin", post(admin, "/admin/seasons", data=dict(name="Mine")).url).group(1)
    r = post(admin, f"/s/{mine}/admin/season/delete", data=dict(name="Mine"))
    check("a visitor's own season can be deleted", r.status_code != 403 and "Mine" not in get(admin, "/admin/seasons").text, r.status_code)
    left = get(admin, "/demo/status").json().get("reset_in")
    check("the bar learns when it resets", isinstance(left, int) and 0 < left <= 30, left)
    post(admin, "/admin/seasons", data=dict(name="Made after the snapshot"))
    # A request that never finishes arriving mustn't hold the reset off.
    slow = socket.create_connection(("127.0.0.1", again.port))
    slow.sendall(b"POST /login HTTP/1.1\r\nHost: x\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 1000000\r\n\r\nu")
    t = time.time()
    wait_for(lambda: again.proc.poll() is not None, 120, 1)
    code = again.proc.poll()
    slow.close()
    check("Bifrost stops itself for the reset, even mid-request", code == 0 and time.time() - t < 90, (code, again.logtext()[-400:]))
    again.log.close()
    again.start()
    admin = as_role(again, "sysadmin")
    check("…and starts again as the snapshot was", "Made after the snapshot" not in get(admin, "/admin/seasons").text
          and "Autumn season" in get(admin, "/admin/seasons").text)
finally:
    again.stop()

# Game servers Bifrost doesn't run are out of the demo's reach.
ext = Server("demo-ext", port=18203, ap_ports="39223-39225", bin=BIN, extra_env={"BIFROST_AP_MODE": "external", **PROXIED})
try:
    ext.start()
    ext_started = True
except RuntimeError:
    ext_started = False
check("a demo build won't use an external game server", not ext_started and "managed game servers" in ext.logtext(), ext.logtext()[-400:])
if ext_started:
    ext.stop()

def refuses(tag, port, env, says):
    x = Server(tag, port=port, ap_ports=f"{port + 21100}-{port + 21102}", bin=BIN, extra_env=env)
    try:
        x.start()
    except RuntimeError:
        return check(f"a demo build won't start: {says}", says in x.logtext(), x.logtext()[-400:])
    x.stop()
    return check(f"a demo build won't start: {says}", False, "it started")


refuses("demo-noproxy", 18204, {}, "BIFROST_TRUSTED_PROXIES")
refuses("demo-badreset", 18205, {**PROXIED, "BIFROST_DEMO_RESET_MINUTES": "60m"}, "BIFROST_DEMO_RESET_MINUTES")

# A demo build signs in natively only.
fwd = Server("demo-fwd", port=18202, ap_ports="39220-39222", bin=BIN)
fwd.cli("auth", "mode", "forward-auth")
try:
    fwd.start()
    started = True
except RuntimeError:
    started = False
check("a demo build won't run under forward auth", not started and "native sign-in" in fwd.logtext(), fwd.logtext()[-400:])
if started:
    fwd.stop()

sys.exit(summary())
