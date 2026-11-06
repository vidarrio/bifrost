#!/usr/bin/env python3
"""Fill a fresh demo build with seasons in each state, for a demo snapshot.

    seed.py <bifrost url> <its data dir>

Run it once against a demo build (`cargo build --features demo`) started on an
empty data dir, then stop Bifrost and copy the data dir: that copy is the
snapshot BIFROST_DEMO_SNAPSHOT restores. See README.md next to this file.

It signs in through /demo/as/<role>, so it only works on a demo build. The demo
locks the common-games console, so the common game's folder is written straight
into the data dir. Generating the seasons takes a minute or two.
"""
import os
import re
import shutil
import sys
import time

import requests

HERE = os.path.dirname(os.path.abspath(__file__))
GUIDE = os.path.join(HERE, "..", "..", "docs", "examples", "common", "minecraft-26.2", "guide.md")
YAML = "name: {}\ngame: ChecksFinder\nChecksFinder: {{}}\n"
COMMON = "minecraft"


def main(base, data):
    base = base.rstrip("/")

    def session(role):
        s = requests.Session()
        r = s.post(f"{base}/demo/as/{role}", allow_redirects=False)
        if r.status_code != 303 or not r.headers.get("x-bifrost-demo"):
            sys.exit(f"seed: {base} isn't a demo build (/demo/as/{role} answered {r.status_code})")
        # Over plain HTTP, a Secure session cookie is never sent back.
        if s.get(f"{base}/api/me").status_code != 200:
            sys.exit("seed: the session cookie didn't come back. Over plain HTTP, start the "
                     "instance you seed with BIFROST_INSECURE_COOKIES=1")
        return s

    player, mod, admin = (session(r) for r in ("player", "moderator", "sysadmin"))

    def post(s, path, **kw):
        r = s.post(base + path, timeout=120, **kw)
        r.raise_for_status()
        return r

    def season(s, name):
        r = post(s, "/admin/seasons", data=dict(name=name))
        m = re.search(r"/s/([0-9a-z]+)/admin", r.url)
        if not m:
            sys.exit(f"seed: creating {name!r} failed: {r.text[:300]}")
        return m.group(1)

    def signup(s, sid, name):
        r = post(s, f"/s/{sid}/signup", files={"yaml": ("player.yaml", YAML.format(name).encode())})
        if 'class="banner ok"' not in r.text:
            sys.exit(f"seed: {name}'s signup failed: {r.text[:300]}")

    def generate(sid):
        post(admin, f"/s/{sid}/admin/generate")
        for _ in range(120):
            t = admin.get(f"{base}/s/{sid}/admin").text
            if "Generated seed" in t:
                return
            if re.search(r'class="banner bad">\s*Failed', t):
                sys.exit(f"seed: generation failed in season {sid}")
            time.sleep(3)
        sys.exit(f"seed: season {sid} didn't generate in time")

    # The common game: a Minecraft server as the demo shows it (no apworld or
    # client files: its slot plays ChecksFinder, which ships with Archipelago).
    d = os.path.join(data, "common", COMMON)
    os.makedirs(os.path.join(d, "client"), exist_ok=True)
    with open(os.path.join(d, "game.toml"), "w") as f:
        f.write('name = "Minecraft 26.2"\nslot = "Commons"\naddress = "mc.example.org:25565"\n\n'
                '[identifier]\nkind = "minecraft"\nlabel = "Minecraft username"\n'
                'pattern = "[A-Za-z0-9_]{3,16}"\nhint = "3 to 16 letters, digits or underscores"\n\n'
                '[[download]]\nfile = "client/README.txt"\nlabel = "Client files (a placeholder in the demo)"\n')
    with open(os.path.join(d, "player.yaml"), "w") as f:
        f.write(YAML.format("Commons"))
    with open(os.path.join(d, "client", "README.txt"), "w") as f:
        f.write("A real common game would offer its client files here: a mod loader, the mod, a modpack.\n")
    shutil.copy(GUIDE, os.path.join(d, "guide.md"))

    print("seed: an archived season", flush=True)
    old = season(admin, "Summer season")
    post(admin, f"/s/{old}/admin/signups", data=dict(open="1"))
    signup(player, old, "Ann")
    signup(mod, old, "Mia")
    generate(old)
    for _ in range(30):
        if "isn't connected" not in post(admin, f"/s/{old}/admin/archive-season", data=dict(name="Summer season")).text:
            break
        time.sleep(3)
    if admin.get(f"{base}/s/{old}/season").status_code != 200:
        sys.exit(f"seed: season {old} didn't archive")

    print("seed: a live season with a common game", flush=True)
    live = season(admin, "Autumn season")
    post(admin, f"/s/{live}/admin/common", data={"do": "add", "key": COMMON})
    post(admin, f"/s/{live}/admin/minesweeper", data=dict(open="1"))
    post(admin, f"/s/{live}/admin/signups", data=dict(open="1"))
    signup(player, live, "Ann")
    signup(mod, live, "Mia")
    signup(admin, live, "Sam")
    generate(live)
    post(player, f"/s/{live}/common/{COMMON}/ident", data=dict(name="Steve"))

    print("seed: a season taking signups, run by the moderator", flush=True)
    nxt = season(mod, "Winter season")
    post(mod, f"/s/{nxt}/admin/signups", data=dict(open="1"))
    signup(player, nxt, "Ann")

    print("seed: done; stop Bifrost and copy its data dir for the snapshot", flush=True)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
