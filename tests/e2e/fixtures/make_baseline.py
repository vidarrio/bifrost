"""Make baseline.db: a database at schema version 1, the baseline 0.3.0
freezes, holding a bit of everything Bifrost keeps, written by Bifrost itself
through its pages. The `upgrade` suite opens it with the current build, so
every migration step after the baseline is tried on real data.

    tests/e2e/fixtures/make_baseline.py

needs a debug build and Archipelago (tests/e2e/setup-ap.sh), like the suites.
Remake it only while the baseline can still change, before 0.3.0 is tagged,
and from the build that's tagged: after that it stands for what 0.3.0 wrote.
Its accounts' password is `correct horse battery`."""
import os
import re
import shutil
import sqlite3
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from lib import FIX, Server, fixture, get, new_season, post, wait_for  # noqa: E402

OUT = os.path.join(FIX, "baseline.db")
YAML = "name: {}\ngame: ChecksFinder\nChecksFinder: {{}}\n"


def generated(s, sid):
    t = wait_for(lambda: (x := get(s, f"/s/{sid}/admin").text) and ("Generated seed" in x or "Failed" in x) and x, 300, 3)
    assert t and "Generated seed" in t, (t or "")[-800:]


srv = Server("baseline", port=18199, ap_ports="39190-39192")
srv.start()
try:
    root = srv.setup_sysadmin()
    ann, bob, cat = (srv.invite_user(root, n) for n in ("ann", "bob", "cat"))
    post(root, "/admin/settings", data=dict(domain="ap.example", home_url="https://example.org",
                                            discords="Archipelago | https://discord.gg/archipelago | official"))
    post(root, "/admin/settings/export", data={"do": "new"})
    post(root, "/admin/catalog/add", files={"apworld": ("foo.apworld", fixture("foo.apworld"))},
         data=dict(game="", version="", note="vetted"))
    post(root, "/admin/common-games/new", data=dict(key="vanilla", name="Vanilla", slot="Commons",
                                                    yaml=YAML.format("Commons")))
    post(root, "/admin/common-games/vanilla", data=dict(
        name="Vanilla", slot="Commons", address="mc.example:25565", kind="minecraft", label="Minecraft username",
        pattern="[A-Za-z0-9_]{3,16}", hint="", guide="Join **{address}**.", yaml=YAML.format("Commons")))

    new_season(root, "Draft")

    sid = new_season(root, "Signups")
    post(root, f"/s/{sid}/admin/signups", data=dict(open="1"))
    post(ann, f"/s/{sid}/signup", files={"yaml": ("y.yaml", YAML.format("Ann").encode())})
    game = re.search(r'href="signup/game/([0-9a-z]+)">Ann<', get(ann, f"/s/{sid}/signup").text).group(1)
    post(ann, f"/s/{sid}/signup/game/{game}/notes", data=dict(notes="Bring snacks"))
    post(ann, f"/s/{sid}/signup/game/{game}/invite")
    token = re.search(r"/seat/([0-9a-f]+)", get(ann, f"/s/{sid}/signup/game/{game}").text).group(1)
    post(cat, f"/s/{sid}/seat/{token}", data=dict(name="Cat"))
    post(ann, f"/s/{sid}/signup/game/{game}/invite")  # one left unused

    sid = new_season(root, "Live")
    post(root, f"/s/{sid}/admin/common", data={"do": "add", "key": "vanilla"})
    post(root, f"/s/{sid}/admin/signups", data=dict(open="1"))
    for who, name in ((ann, "Ann"), (bob, "Bob")):
        post(who, f"/s/{sid}/signup", files={"yaml": ("y.yaml", YAML.format(name).encode())})
    post(root, f"/s/{sid}/admin/generate")
    generated(root, sid)
    post(ann, f"/s/{sid}/common/vanilla/ident", data=dict(name="Steve"))
    post(bob, f"/s/{sid}/common/vanilla/ident", data=dict(name="Alex"))

    sid = new_season(root, "Archived")
    post(root, f"/s/{sid}/admin/signups", data=dict(open="1"))
    post(cat, f"/s/{sid}/signup", files={"yaml": ("y.yaml", YAML.format("Cat").encode())})
    post(root, f"/s/{sid}/admin/generate")
    generated(root, sid)
    assert wait_for(lambda: "isn't connected" not in post(root, f"/s/{sid}/admin/archive-season", data=dict(name="Archived")).text, 90, 3)
finally:
    srv.stop()

tmp = OUT + ".tmp"
if os.path.exists(tmp):
    os.remove(tmp)
with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as db:
    assert db.execute("PRAGMA user_version").fetchone()[0] == 1, "not the baseline schema"
    db.execute("VACUUM INTO ?", (tmp,))
shutil.move(tmp, OUT)
print("wrote", OUT, os.path.getsize(OUT), "bytes")
