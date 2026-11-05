"""Common games: the sysadmin's catalog of setups under common/, several in one
season, each played as one shared slot. A setup's slot name is reserved once
added; its apworld is staged and trusted, and uploads can't take its name; its
guide is shown safely with the season's address filled in; its downloads are
only the files it lists; players with a game of their own register a name per
kind of game, unique per kind; and the admin downloads the season's allowlist.
The lineup is fixed once generated, and a re-roll keeps it."""
import json
import os
import re
import shutil

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
    check("and so does registering", "Anon1" not in get(root, f"/s/{sid}/admin/common/vanilla.1/allowlist").text)
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
