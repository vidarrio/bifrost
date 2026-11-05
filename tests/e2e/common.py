"""Common games: the sysadmin's catalog of setups under common/, several in one
season, each played as one shared slot. A setup's slot name is reserved once
added; its apworld is staged and trusted, and uploads can't take its name; its
guide is shown safely with the season's address filled in; its downloads are
only the files it lists; players with a game of their own register a name per
kind of game, unique per kind; and the admin downloads the season's allowlist.
The lineup is fixed once generated, and a re-roll keeps it."""
import io
import json
import os
import re
import shutil
import socket
import sqlite3
import struct
import subprocess
import sys
import tempfile
import threading
import zipfile

from lib import Server, check, fixture, get, new_season, post, summary, wait_for

srv = Server("common", port=18198, ap_ports="39187-39189")
cat = os.path.join(srv.data, "common")


def setup(key, toml, files):
    d = os.path.join(cat, key)
    os.makedirs(os.path.join(d, "client"), exist_ok=True)
    with open(os.path.join(d, "game.toml"), "w") as f:
        f.write(toml)
    for name, body in files.items():
        with open(os.path.join(d, name), "wb") as f:
            f.write(body)


MC = '[identifier]\nkind = "minecraft"\nlabel = "Minecraft username"\npattern = "[A-Za-z0-9_]{3,16}"\nhint = "3 to 16 letters, digits or underscores"\n'
setup("vanilla.1", 'name = "Clique Vanilla"\nslot = "Vanilla"\n' + MC
      + '[[download]]\nfile = "client/mod.jar"\nlabel = "The mod"\n',
      {"player.yaml": b"name: Vanilla\ngame: ChecksFinder\nChecksFinder: {}\n",
       "client/mod.jar": b"JARBYTES",
       "secret.txt": b"not listed",
       "guide.md": b"Connect to **{address}** as `{slot}`.\n\n<script>alert(1)</script>\n\n[bad](javascript:alert(1))\n"})
setup("modded", 'name = "Foo Modded"\nslot = "Modded"\napworld = "foo.apworld"\naddress = "modded.example:25565"\n' + MC,
      {"player.yaml": b"name: Modded\ngame: Foo Finder\nFoo Finder: {}\n",
       "foo.apworld": fixture("foo.apworld")})
setup("broken", 'name = "Broken"\n', {"player.yaml": b"name: X\n"})

srv.start()
try:
    root = srv.setup_sysadmin()
    ann, bob, carl = (srv.invite_user(root, n) for n in ("ann", "bob", "carl"))
    sid = new_season(root, "Commons")
    admin = lambda: get(root, f"/s/{sid}/admin").text
    a = admin()
    check("the catalog's setups can be added", "Clique Vanilla (plays as Vanilla)" in a and "Foo Modded (plays as Modded)" in a, a[-2000:])
    check("a broken setup says why", "<code>broken</code> can't be used" in a and "slot" in a.split("<code>broken</code>")[1][:200], a[-1500:])
    for key in ("vanilla.1", "modded"):
        post(root, f"/s/{sid}/admin/common", data={"do": "add", "key": key})
    a = admin()
    check("a season takes several common games", "plays as <b>Vanilla</b>" in a and "plays as <b>Modded</b>" in a, a[-2000:])
    r = post(root, f"/s/{sid}/admin/common", data={"do": "add", "key": "broken"})
    check("a broken setup can't be added", "can't be used" in r.text, r.text[-400:])
    post(root, f"/s/{sid}/admin/common", data={"do": "address", "key": "vanilla.1", "address": "vanilla.example:25565"})

    # Open to any user, so someone without a game sees its pages.
    post(root, f"/s/{sid}/admin/privacy", data=dict(what="live", private="0"))
    post(root, f"/s/{sid}/admin/signups", data=dict(open="1"))
    r = post(ann, f"/s/{sid}/signup", files={"yaml": ("y.yaml", b"name: vanilla\ngame: ChecksFinder\nChecksFinder: {}\n")})
    check("a signup can't take a common game's slot name", "already taken" in r.text, r.text[-400:])
    r = post(ann, f"/s/{sid}/signup/apworld", files={"apworld": ("foo.apworld", fixture("foo.apworld"))})
    check("an upload can't take a common game's apworld name", "reserved" in r.text, r.text[-300:])
    for who, name in ((ann, "Ann"), (bob, "Bob")):
        post(who, f"/s/{sid}/signup", files={"yaml": ("y.yaml", f"name: {name}\ngame: ChecksFinder\nChecksFinder: {{}}\n".encode())})
    post(root, f"/s/{sid}/admin/generate")
    t = wait_for(lambda: (x := admin()) and ("Generated seed" in x or "Failed" in x) and x, 300, 3)
    gen = re.search(r"(Generated seed[^<]*|Failed[^<]*)", t or "")
    check("the season generates with both", gen and "2 common games" in gen.group(1), gen and gen.group(1))
    m = re.search(r"foo\.apworld <span class=\"statline\">· ([^<]*)<", t or "")
    check("a common game's apworld is staged and trusted", m and "shipped with Bifrost" in m.group(1), m and m.group(1))
    board = lambda: get(root, f"/s/{sid}/board").text
    check("the board shows each by its name", wait_for(lambda: "Clique Vanilla" in board() and "Foo Modded" in board(), 60), board()[-2000:])
    b = board()
    check("each shows its own address", "vanilla.example:25565" in b and "modded.example:25565" in b)
    gid = lambda name: re.search(r'href="game/([0-9a-z]+)"><span class="st">[^<]*</span><div class="game">' + re.escape(name) + "<", board()).group(1)
    van = f"/s/{sid}/game/{gid('Clique Vanilla')}"

    page = wait_for(lambda: (x := get(carl, van).text) and "isn't up yet" not in x and x, 60)
    j = get(carl, van + "/live").json()
    check("a common game's page keeps its figures and hints live too", re.fullmatch(r"\d+ locations · sent \d+ · received \d+ · \d+ left", j.get("stats", "")) and isinstance(j.get("hints"), str), j)
    check("the guide fills in the address and slot", "<strong>vanilla.example:25565</strong>" in page and "<code>Vanilla</code>" in page, page[-3000:])
    # Raw: the helpers decode entities.
    guide = carl.get(carl.base + van).text.split('<div class="guide">')[1].split("</div>")[0]
    check("the guide can't run script", "<script" not in guide and "javascript:" not in guide and "&lt;script&gt;" in guide, repr(guide))
    check("someone without a game can't register", "Play a game this season" in page and 'action="common/vanilla.1/ident"' not in page)
    r = post(carl, f"/s/{sid}/common/vanilla.1/ident", data=dict(name="Carl1"))
    check("nor by posting", "Play a game this season first" in r.text, r.text[-300:])
    r = get(carl, f"/s/{sid}/common/vanilla.1/file/0")
    check("a listed download is served", r.content == b"JARBYTES" and "mod.jar" in r.headers.get("content-disposition", ""), r.headers)
    anon = srv.session()
    r = get(anon, f"/s/{sid}/common/vanilla.1/file/0")
    check("downloads need a sign-in", r.content != b"JARBYTES", r.status_code)
    r = post(anon, f"/s/{sid}/common/vanilla.1/ident", data=dict(name="Anon1"))
    check("and so does registering", "Play a game this season first" not in r.text and "Anon1" not in get(root, f"/s/{sid}/admin/common/vanilla.1/allowlist").text, r.text[-300:])
    r = get(carl, f"/s/{sid}/common/vanilla.1/file/1")
    check("only what the setup lists", r.content != b"not listed" and "isn't available" in r.text)
    r = get(carl, f"/s/{sid}/common/broken/file/0")
    check("only the season's common games", "isn't available" in r.text)

    post(ann, f"/s/{sid}/common/vanilla.1/ident", data=dict(name="Steve"))
    page = get(ann, van).text
    check("a player registers their name", "On the allowlist as <b>Steve</b>" in page, re.sub(r"\s+", " ", re.sub(r"<[^>]+>", " ", page.split("<main>")[-1]))[:600])
    r = post(bob, f"/s/{sid}/common/modded/ident", data=dict(name="steve"))
    check("a name is one player's per kind of game", "Someone else already registered" in r.text, r.text[-300:])
    r = post(bob, f"/s/{sid}/common/modded/ident", data=dict(name="a b"))
    check("a name follows the setup's rule", "3 to 16 letters" in r.text, r.text[-300:])
    post(bob, f"/s/{sid}/common/modded/ident", data=dict(name="Alex_2"))
    check("one name serves every setup of its kind", "On the allowlist as <b>Alex_2</b>" in get(bob, van).text)

    r = get(root, f"/s/{sid}/admin/common/vanilla.1/allowlist")
    check("the admin downloads the allowlist", r.text == "Alex_2\nSteve\n" and "attachment" in r.headers.get("content-disposition", ""), repr(r.text))
    r = get(root, f"/s/{sid}/admin/common/modded/allowlist?format=json")
    check("as JSON too", r.headers.get("content-type", "").startswith("application/json") and json.loads(r.text) == ["Alex_2", "Steve"], r.text)
    r = get(ann, f"/s/{sid}/admin/common/vanilla.1/allowlist")
    check("only admins get it", "Steve" not in r.text)
    check("the admin page counts it", "Allowlist: 2 Minecraft usernames" in admin())

    # The export, for a sysadmin's own sync.
    export = lambda path, tok, scheme="Bearer", **kw: srv.session().get(srv.base + path, headers={"Authorization": f"{scheme} {tok}"} if tok else {}, **kw)
    # A season that isn't live, with a common game: it stays out of the export.
    other = new_season(root, "Not yet")
    post(root, f"/s/{other}/admin/common", data={"do": "add", "key": "vanilla.1"})
    check("the export is off without a token", export("/export/commons", None).status_code == 401)
    r = post(ann, "/admin/settings/export", data={"do": "new"})
    check("only a sysadmin makes a token", "Sysadmins only" in r.text)
    r = root.post(srv.base + "/admin/settings/export", data={"do": "new"})
    token = re.search(r'<code class="token">([0-9a-f]+)</code>', r.text).group(1)
    check("a wrong token is refused", export("/export/commons", "0" * 64).status_code == 401)
    check("a session isn't a token", get(root, "/export/commons").status_code == 401)
    # A per-slot file for the common game and one for a player, as a world that
    # makes files would leave in the seed.
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as db:
        slot_of = dict(db.execute("SELECT name, ap_slot FROM slot WHERE season_id = ?", (sid,)).fetchall())
    zpath = os.path.join(srv.data, "seasons", sid, "output", "current.zip")
    seed = next(n for n in zipfile.ZipFile(zpath).namelist() if n.endswith(".archipelago")).rsplit(".", 1)[0]
    with zipfile.ZipFile(zpath, "a") as z:
        # In a folder, as some worlds keep theirs.
        z.writestr(f"Vanilla/{seed}_P{slot_of['Vanilla']}_Vanilla.apmc", b"VANILLA-SEED")
        z.writestr(f"{seed}_P{slot_of['Ann']}_Ann.apmc", b"ANN-SEED")
    r = export("/export/commons", token)
    idx = r.json()
    mine = [s for s in idx["seasons"] if s["id"] == sid]
    commons = {c["key"]: c for c in mine[0]["commons"]} if mine else {}
    check("the index lists the live season's common games", set(commons) == {"vanilla.1", "modded"} and mine[0]["seed"], r.text[:600])
    check("and only live seasons'", [s["id"] for s in idx["seasons"]] == [sid], [s["id"] for s in idx["seasons"]])
    v = commons.get("vanilla.1", {})
    al = v.get("allowlist") or {}
    check("with their address and allowlist", v.get("address") == "vanilla.example:25565" and al.get("count") == 2 and al.get("kind") == "minecraft", v)
    vfile = f"Vanilla/{seed}_P{slot_of['Vanilla']}_Vanilla.apmc"
    files = v.get("files") or []
    check("and their slot's files only", [(f["name"], f["filename"]) for f in files] == [(vfile, vfile.split("/")[-1])], files)
    r = export(files[0]["url"], token) if files else None
    check("a file downloads by its url", r is not None and r.content == b"VANILLA-SEED", r and r.status_code)
    r = export(f"/export/{sid}/vanilla.1/file", token, params={"name": files[0]["name"]}) if files else None
    check("and by its name as listed", r is not None and r.content == b"VANILLA-SEED", r and r.status_code)
    check("the scheme's case doesn't matter", export("/export/commons", token, scheme="bearer").status_code == 200)
    check("a malformed request without a token is refused, not parsed", export("/export/nope/vanilla.1/file", None).status_code == 401)
    r = export(f"/export/{sid}/vanilla.1/file", token, params={"name": f"{seed}_P{slot_of['Ann']}_Ann.apmc"})
    check("not another slot's file", r.status_code == 404 and b"ANN-SEED" not in r.content, r.status_code)
    r = export(f"/export/{sid}/vanilla.1/file", token, params={"name": f"{seed}.archipelago"})
    check("nor the server's", r.status_code == 404)
    r = export(f"/export/{sid}/vanilla.1/allowlist", token)
    check("the allowlist downloads with the token", r.text == "Alex_2\nSteve\n", repr(r.text))
    check("not without it", export(f"/export/{sid}/vanilla.1/allowlist", None).status_code == 401)

    # The example sync (docs/examples/common/sync), both ways.
    sync = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "docs", "examples", "common", "sync", "bifrost-common-sync")
    work = tempfile.mkdtemp(prefix="bifrost-sync-")
    base_env = {"PATH": os.environ["PATH"], "BIFROST_URL": srv.base, "BIFROST_EXPORT_TOKEN": token,
                "COMMON_KEY": "vanilla.1", "SEED_DIR": os.path.join(work, "seed")}
    run = lambda mode, **e: subprocess.run([sys.executable, "-I", sync, mode], env={**base_env, **e}, capture_output=True, text=True, timeout=60)
    copy_env = dict(STATE_DIR=os.path.join(work, "copy"), ALLOWLIST_FILE=os.path.join(work, "whitelist.txt"),
                    AFTER=f"touch {os.path.join(work, 'after')}")
    r = run("copy", **copy_env)
    seedfile = os.path.join(work, "seed", vfile.split("/")[-1])
    check("the copy sync writes the allowlist", r.returncode == 0 and open(copy_env["ALLOWLIST_FILE"]).read() == "Alex_2\nSteve\n", (r.returncode, r.stdout, r.stderr))
    check("…and the slot's seed file", os.path.exists(seedfile) and open(seedfile, "rb").read() == b"VANILLA-SEED")
    check("…and runs AFTER", os.path.exists(os.path.join(work, "after")))
    os.remove(os.path.join(work, "after"))
    r = run("copy", **copy_env)
    check("a second run with nothing new changes nothing", r.returncode == 0 and not os.path.exists(os.path.join(work, "after")) and r.stdout == "", (r.stdout, r.stderr))
    r = run("copy", **{**copy_env, "ALLOWLIST_FORMAT": "json", "ALLOWLIST_FILE": os.path.join(work, "w.json")})
    check("…and writes it as JSON", r.returncode == 0 and json.load(open(os.path.join(work, "w.json"))) == ["Alex_2", "Steve"], r.stderr)
    r = run("copy", **{**copy_env, "BIFROST_EXPORT_TOKEN": "0" * 64})
    check("a wrong token fails the sync", r.returncode == 1 and "401" in r.stderr, (r.returncode, r.stderr))

    # A stand-in Minecraft server's RCON.
    sent = []
    lsock = socket.socket()
    lsock.bind(("127.0.0.1", 0))
    lsock.listen(1)

    def rcon_server():
        conn, _ = lsock.accept()
        def recv(n):
            b = b""
            while len(b) < n:
                b += conn.recv(n - len(b))
            return b
        try:
            while True:
                (length,) = struct.unpack("<i", recv(4))
                rid, kind = struct.unpack("<ii", recv(8))
                body = recv(length - 8)[:-2].decode()
                if kind == 3:
                    reply = "" if body == "pw" else None
                    rid = rid if reply is not None else -1
                    reply = reply or ""
                else:
                    sent.append(body)
                    reply = ("There are 3 whitelisted player(s): Steve, Herobrine, Notch" if body == "whitelist list"
                             else "ok " + body)
                data = struct.pack("<ii", rid, 0) + reply.encode() + b"\0\0"
                conn.sendall(struct.pack("<i", len(data)) + data)
        except (struct.error, OSError):
            conn.close()

    t = threading.Thread(target=rcon_server, daemon=True)
    t.start()
    r = run("rcon", STATE_DIR=os.path.join(work, "rcon"), RCON_PORT=str(lsock.getsockname()[1]), RCON_PASSWORD="pw", KEEP="notch")
    t.join(10)
    check("the RCON sync whitelists who's missing and removes who's gone, keeping KEEP",
          r.returncode == 0 and sent == ["whitelist list", "whitelist add Alex_2", "whitelist remove Herobrine"], (sent, r.stdout, r.stderr))
    lsock.close()
    shutil.rmtree(work)
    check("only a season's own common games", export(f"/export/{sid}/broken/allowlist", token).status_code == 404)
    check("settings say a token is set", "A token is set" in get(root, "/admin/settings").text)
    r = root.post(srv.base + "/admin/settings/export", data={"do": "new"})
    token2 = re.search(r'<code class="token">([0-9a-f]+)</code>', r.text).group(1)
    check("replacing it stops the old one", export("/export/commons", token).status_code == 401)
    check("and the new one works", export("/export/commons", token2).status_code == 200)
    root.post(srv.base + "/admin/settings/export", data={"do": "revoke"})
    check("revoking turns the export off", "No token is set" in get(root, "/admin/settings").text and export("/export/commons", token2).status_code == 401)

    # Editing the catalog from the console.
    page = lambda key="": get(root, "/admin/common-games" + (f"/{key}" if key else "")).text
    check("the console lists the catalog and who uses it", "plays as <b>Vanilla</b>" in page() and "in Commons, Not yet" in page() and "Can't be used" in page(), page()[-2000:])
    r1 = get(ann, "/admin/common-games").text
    r2 = post(ann, "/admin/common-games/new", data=dict(key="x", name="X", slot="X", yaml="name: X\n")).text
    check("only sysadmins edit it", "New setup" not in r1 and not os.path.exists(os.path.join(cat, "x")), re.sub(r"\s+", " ", re.sub(r"<[^>]+>", " ", r1.split("<main>")[-1]))[:300] + " | " + re.sub(r"\s+", " ", re.sub(r"<[^>]+>", " ", r2.split("<main>")[-1]))[:300])
    r = post(root, "/admin/common-games/new", data=dict(key="gui", name="Bar Co-op", slot="Barco", yaml="name: Barco\r\ngame: Bar Finder\r\nBar Finder: {}\r\n"))
    check("a sysadmin makes a setup", r.url.endswith("/admin/common-games/gui") and "Bar Co-op" in r.text, r.text[-800:])
    r = post(root, "/admin/common-games/new", data=dict(key="gui2", name="X", slot="Barco", yaml="name: Other\n"))
    check("one that wouldn't load isn't made", "not the slot" in r.text and not os.path.exists(os.path.join(cat, "gui2")), r.text[-600:])
    form = dict(name="Bar Co-op", slot="Barco", address="bar.example:1", kind="bar", label="Bar name", pattern="[a-z]{2,8}", hint="", guide="Go to **{address}**", yaml="name: Barco\ngame: Bar Finder\nBar Finder: {}\n")
    r = post(root, "/admin/common-games/gui", data=form)
    check("its fields save", "Saved." in r.text and 'value="bar.example:1"' in r.text, r.text[-800:])
    r = post(root, "/admin/common-games/gui", data={**form, "pattern": "("})
    check("a change that breaks it is refused, nothing changed", "Not saved" in r.text and 'value="[a-z]{2,8}"' in r.text, r.text[-600:])
    r = post(root, "/admin/common-games/gui/apworld", files={"file": ("bar.apworld", fixture("bar.apworld"))})
    check("its apworld uploads", "Added." in r.text and "<code>bar.apworld</code>" in r.text, r.text[-600:])
    check("and counts as custom, not trusted", "runs as a custom world" in r.text)
    r = root.post(srv.base + "/admin/common-games/gui/apworld", files={"file": ("big.apworld", b"PK\x03\x04" + b"\0" * (33 << 20))})
    check("an apworld upload is capped like a player's", r.status_code == 413 or "Not added" in r.text, r.status_code)
    r = post(root, "/admin/common-games/gui/apworld", files={"file": ("x.apworld", b"not a zip")})
    check("an apworld has to be a zip", "Not added" in r.text and "zip" in r.text, r.text[-400:])
    r = post(root, "/admin/common-games/gui/download", data=dict(label="The client"), files={"file": ("../My Client.zip", b"CLIENT")})
    check("a download uploads, under a plain name", "client/My_Client.zip" in r.text and open(os.path.join(cat, "gui", "client", "My_Client.zip"), "rb").read() == b"CLIENT", r.text[-600:])
    r = post(root, f"/s/{other}/admin/common", data={"do": "add", "key": "gui"})
    check("an edited setup can be added to a season", "plays as <b>Barco</b>" in get(root, f"/s/{other}/admin").text, re.sub(r"\s+", " ", re.sub(r"<[^>]+>", " ", r.text.split("<main>")[-1]))[:400])
    r = post(root, "/admin/common-games/gui/delete")
    check("a setup a season uses can't be deleted", "still include it" in r.text and os.path.isdir(os.path.join(cat, "gui")), r.text[-400:])
    post(root, f"/s/{other}/admin/common", data={"do": "remove", "key": "gui"})
    post(root, "/admin/common-games/gui/remove", data=dict(what="download", file="client/My_Client.zip"))
    check("a download is removed with its file", not os.path.exists(os.path.join(cat, "gui", "client", "My_Client.zip")))
    r = post(root, "/admin/common-games/gui/delete")
    check("an unused setup is deleted", "Deleted gui" in r.text and not os.path.exists(os.path.join(cat, "gui")), r.text[-400:])
    check("a broken setup's page says why", "can't be used as it is" in page("broken"))
    r = post(root, "/admin/common-games/broken", data=dict(name="Fixed", slot="X", yaml="name: X\ngame: ChecksFinder\nChecksFinder: {}\n"))
    check("and saving fixes it", "Saved." in r.text and "Can't be used" not in page(), r.text[-600:])

    r = post(root, f"/s/{sid}/admin/common", data={"do": "remove", "key": "modded"})
    check("a generated season keeps its common games", "keeps its common games" in r.text and "plays as <b>Modded</b>" in admin(), r.text[-300:])
    r = post(root, f"/s/{sid}/admin/common", data={"do": "add", "key": "broken"})
    check("and takes no new ones", "before the season is generated" in r.text, r.text[-300:])

    post(root, f"/s/{sid}/admin/regenerate")
    t = wait_for(lambda: (x := admin()) and ("Regenerated seed" in x or "Failed" in x) and x, 300, 3)
    check("a re-roll works", t and "Regenerated seed" in t, (t or "")[-800:])
    check("and keeps the common games", wait_for(lambda: "Clique Vanilla" in board() and "Foo Modded" in board(), 60), board()[-1500:])

    # The catalog changes under a live season: its page still says what it is.
    shutil.rmtree(os.path.join(cat, "modded"))
    a = admin()
    check("a setup gone from the catalog is flagged", "isn't in the catalog anymore" in a, a[-1500:])
    check("its card stays", "Foo Modded" in board())

    # On a host with no sandbox for custom worlds, an apworld uploaded from the
    # browser doesn't get to run, while one the operator put in place does.
    post(root, "/admin/common-games/new", data=dict(key="upl", name="Uploaded", slot="Upl", yaml="name: Upl\ngame: Bar Finder\nBar Finder: {}\n"))
    r = post(root, "/admin/common-games/upl/apworld", files={"file": ("bar.apworld", fixture("bar.apworld"))})
    check("(set up a console-uploaded apworld)", "Added." in r.text, r.text[-300:])
    setup("hand", 'name = "By hand"\nslot = "Hand"\napworld = "baz.apworld"\n', {"player.yaml": b"name: Hand\ngame: Baz Finder\nBaz Finder: {}\n", "baz.apworld": fixture("baz.apworld")})
    srv.stop()
    srv = Server("common", port=18198, ap_ports="39187-39189", fresh=False, extra_env={"BIFROST_SANDBOX": "none"})
    srv.start()
    r = post(root, "/admin/common-games/upl/apworld", files={"file": ("bar.apworld", fixture("bar.apworld"))})
    check("without a sandbox the console takes no apworld", "no sandbox for custom" in r.text, r.text[-400:])
    check("and says so on the page", "can't be uploaded here" in get(root, "/admin/common-games/upl").text)
    post(root, f"/s/{other}/admin/common", data={"do": "remove", "key": "vanilla.1"})
    post(root, f"/s/{other}/admin/common", data={"do": "add", "key": "upl"})
    post(root, f"/s/{other}/admin/signups", data=dict(open="1"))
    post(ann, f"/s/{other}/signup", files={"yaml": ("y.yaml", b"name: Ann\ngame: ChecksFinder\nChecksFinder: {}\n")})
    gen_of = lambda s: (lambda t: re.search(r"(Generated seed[^<]*|Failed[^<]*)", t or ""))(
        wait_for(lambda: (x := get(root, f"/s/{s}/admin").text) and ("Generated seed" in x or "Failed" in x) and x, 300, 3))
    post(root, f"/s/{other}/admin/generate")
    g = gen_of(other)
    check("a console-uploaded apworld doesn't run without a sandbox", g and "can't run without a sandbox" in g.group(1) and "bar.apworld" in g.group(1), g and g.group(1))
    post(root, f"/s/{other}/admin/common", data={"do": "remove", "key": "upl"})
    post(root, f"/s/{other}/admin/common", data={"do": "add", "key": "hand"})
    post(root, f"/s/{other}/admin/generate")
    g = gen_of(other)
    check("one the operator put in place does", g and "Generated seed" in g.group(1), g and g.group(1))
    installed = get(root, f"/s/{other}/admin").text.split("<h2>Installed apworlds</h2>")[-1].split("<h2>")[0]
    check("and the uploaded one was cleared out", "bar.apworld" not in installed and "baz.apworld" in installed, installed[:600])

    # Back to signups without it: its apworld goes with it.
    post(root, f"/s/{sid}/admin/cancel", data=dict(name="Commons"))
    post(root, f"/s/{sid}/admin/common", data={"do": "remove", "key": "modded"})
    check("before generation a common game can be removed", "plays as <b>Modded</b>" not in admin())
    post(root, f"/s/{sid}/admin/generate")
    t = wait_for(lambda: (x := admin()) and ("Generated seed" in x or "Failed" in x) and x, 300, 3)
    gen = re.search(r"(Generated seed[^<]*|Failed[^<]*)", t or "")
    check("the season generates again", gen and "1 common game:" in gen.group(1), gen and gen.group(1))
    installed = (t or "").split("<h2>Installed apworlds</h2>")[-1].split("<h2>")[0]
    check("without the removed one's apworld", t and "foo.apworld" not in installed, installed[:600])
finally:
    srv.stop()
raise SystemExit(summary())
