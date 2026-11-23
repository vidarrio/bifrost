"""Apworld catalog + approval: manual approval holds uploads and their
signups, approve/reject, catalog picks and catalog-matching uploads, auto mode."""
import io
import json
import os
import re
import sys
import time
import zipfile

from lib import Server, check, first_game, fixture, generate, get, new_season, post, summary, wait_for

srv = Server("catalog").start()
try:
    root = srv.setup_sysadmin()
    alice = srv.invite_user(root, "alice")
    bob = srv.invite_user(root, "bob")

    sid = new_season(root, "Spring")
    S = f"/s/{sid}"
    post(root, f"{S}/admin/signups", data=dict(open="1"))

    # --- permissions
    check("anonymous can't see the catalog", "Sysadmins" in get(srv.session(), "/admin/catalog").text
          or get(srv.session(), "/admin/catalog", allow_redirects=False).status_code in (302, 303, 401, 403))
    r = get(alice, "/admin/catalog", allow_redirects=False)
    check("a plain user can't see the catalog", r.status_code in (302, 303, 403) or "Sysadmins" in r.text, r.status_code)
    r = get(root, "/admin/catalog")
    check("sysadmin sees the catalog page, manual by default",
          r.ok and 'value="manual" checked' in r.text and "The catalog is empty" in r.text)

    def upload(s, name, data=None):
        files = {"apworld": (name, data if data is not None else fixture(name))}
        return post(s, f"{S}/signup/apworld", files=files).json()

    # --- manual: upload waits
    j = upload(alice, "foo.apworld")
    check("manual mode: upload is pending", j.get("ok") and j.get("pending") is True and j.get("world") == "foo.apworld", j)
    j = upload(bob, "foo.apworld")
    check("same file again: still pending (no error)", j.get("ok") and j.get("pending"), j)
    j = upload(bob, "foo.apworld", fixture("foo-v2.apworld"))
    check("different content under a pending name is refused", not j.get("ok") and "waiting for approval" in j.get("error", ""), j)
    r = post(alice, f"{S}/signup/check", files={"yaml": ("foo.yaml", fixture("foo.yaml"))}).json()
    check("pending world isn't loaded by the checker", not r.get("ok"), r)

    r = post(alice, f"{S}/signup", files={"yaml": ("foo.yaml", fixture("foo.yaml"))},
             data={"is_custom": "1", "pending_world": "foo.apworld"})
    check("held signup accepted", "held until a moderator approves foo.apworld" in r.text, r.text[-400:])
    r = post(bob, f"{S}/signup", files={"yaml": ("x.yaml", b"game: Foo Finder\n")},
             data={"is_custom": "1", "pending_world": "foo.apworld"})
    check("held signup needs a top-level name", "Couldn't find a top-level name" in r.text)
    r = post(bob, f"{S}/signup", files={"yaml": ("x.yaml", fixture("foo.yaml"))},
             data={"is_custom": "1", "pending_world": "nope.apworld"})
    check("held signup for an unknown pending world is refused", "isn't waiting for approval" in r.text)

    # A signup is one game: refused at upload when it's plain, on approval otherwise.
    second = b"\n---\nname: Two\ngame: Foo Finder\nFoo Finder: {}\n"
    r = post(bob, f"{S}/signup", files={"yaml": ("x.yaml", fixture("foo.yaml").replace(b"Pfoo", b"One") + second)},
             data={"is_custom": "1", "pending_world": "foo.apworld"})
    check("a held file with several players is refused", "several players" in r.text, r.text[-300:])
    r = post(bob, f"{S}/signup", files={"yaml": ("x.yaml", fixture("foo.yaml").replace(b"Pfoo", b"One") + second.replace(b"name:", b"\"name\":"))},
             data={"is_custom": "1", "pending_world": "foo.apworld"})
    check("…however it's spelled", "several players" in r.text, r.text[-300:])
    carl = srv.invite_user(root, "carl")  # signups are rate limited per player
    held = lambda y, who=carl: post(who, f"{S}/signup", files={"yaml": ("x.yaml", y)}, data={"is_custom": "1", "pending_world": "foo.apworld"})
    r = held(b"name: [unclosed\n")
    check("a held file has to be YAML", "valid YAML" in r.text, r.text[-300:])
    r = held(b"name: Wrong\ngame: Bar Finder\nBar Finder: {}\n")
    check("…for the world waiting", "the world waiting for approval is Foo Finder" in r.text, r.text[-300:])
    r = held(fixture("foo.yaml"))
    check("…with a free name", "already taken" in r.text, r.text[-300:])
    r = held(b"\xff\xfename: Latin\n")
    check("…as text", "UTF-8" in r.text, r.text[-300:])
    r = held(b"name: Yes\ngame: Foo Finder\nFoo Finder: {}\n")
    check("…with a plain name", "top-level name" in r.text, r.text[-300:])
    t = time.time()
    many = b"a: &a [" + b"x, " * 20000 + b"x]\nb: [" + b"*a, " * 2000 + b"*a]\nname: Many\ngame: Bar Finder\n"
    r = held(many)
    check("a large file doesn't take long", time.time() - t < 30 and "Foo Finder" in r.text, r.text[-300:])
    r = held(b"\xef\xbb\xbf" + fixture("foo.yaml").replace(b"Pfoo", b"Bommy"))
    check("a byte-order mark is fine", "held until" in r.text, r.text[-300:])
    bom = re.search(r'name="id" value="([0-9a-z]+)"><button class="ghost" type="submit">Remove', get(carl, f"{S}/signup").text).group(1)
    post(carl, f"{S}/signup/delete", data={"id": bom})
    # Only the world's code can tell this one fails.
    bad = fixture("foo.yaml").replace(b"Pfoo", b"Bofoo") + b"requires:\n  version: 99.0.0\n"
    r = held(bad, bob)
    check("what only the world can tell is held", "held until" in r.text, r.text[-300:])
    badg = re.search(r'href="signup/game/([0-9a-z]+)">Bofoo<', get(bob, f"{S}/signup").text).group(1)

    page = get(alice, f"{S}/signup").text
    game = re.search(r'href="signup/game/([0-9a-z]+)">Pfoo<', page).group(1)
    check("a held game's page says so", "waiting for foo.apworld" in get(alice, f"{S}/signup/game/{game}").text)
    r = post(alice, f"{S}/signup/game/{game}/yaml", files={"yaml": ("foo.yaml", b"name: [unclosed\n")})
    check("a held game's replacement gets the first look", "valid YAML" in r.text, r.text[-300:])
    r = post(alice, f"{S}/signup/game/{game}/yaml", files={"yaml": ("foo.yaml", fixture("foo.yaml").replace(b"Pfoo", b"Pfoo3"))})
    check("…and replaces it, still held", "<h1>Pfoo3</h1>" in r.text and "waiting for foo.apworld" in r.text, r.text[-400:])
    r = post(alice, f"{S}/signup/game/{game}/yaml", files={"yaml": ("foo.yaml", fixture("foo.yaml"))})
    check("…as often as needed", "<h1>Pfoo</h1>" in r.text, r.text[-400:])
    check("roster shows the held signup", "waiting for foo.apworld to be approved" in page)
    adm = get(root, f"{S}/admin").text
    check("admin page lists the request", "Apworlds waiting for approval" in adm and "Foo Finder" in adm)
    check("admin signup list marks it held", "held for foo.apworld" in adm)
    check("the request lists who's waiting", "signups waiting: " in adm and "Pfoo" in adm.split("Apworlds waiting")[1].split("<h2")[0])

    post(root, f"{S}/admin/generate")
    msg = wait_for(lambda: (m := re.search(r"Failed[^<]*", get(root, f"{S}/admin").text)) and m.group(0), 30)
    check("generate refuses while signups are held", msg and "held" in msg, msg)

    r = post(alice, f"{S}/admin/worlds/approve", data={"name": "foo.apworld"})
    check("a player can't approve", "Admins only" in r.text or r.status_code in (403, 302, 303), r.status_code)

    post(root, f"{S}/admin/worlds/approve", data={"name": "foo.apworld"})
    adm = get(root, f"{S}/admin").text
    check("approved world is installed with provenance", "approved by root" in adm and "Apworlds waiting" not in adm)
    released = wait_for(lambda: "waiting for foo.apworld" not in get(alice, f"{S}/signup").text, 120, 2)
    needs = wait_for(lambda: "needs attention" in (t := get(bob, f"{S}/signup").text) and t, 120, 2)
    check("others only see that it's held", "99.0.0" not in get(alice, f"{S}/signup").text and "99.0.0" not in get(alice, f"{S}/signup/game/{badg}").text)
    check("a failed check tells the player", needs and "Bofoo needs attention" in needs, get(bob, f"{S}/signup").text[-500:])
    adm = get(root, f"{S}/admin").text
    check("…and the moderators, who can dismiss it", "Signups that need attention" in adm and ">dismiss<" in adm)
    post(root, f"{S}/admin/generate")
    msg = wait_for(lambda: (m := re.search(r"Failed[^<]*", get(root, f"{S}/admin").text)) and "Bofoo" in m.group(0) and m.group(0), 30)
    check("generating names it", msg, msg)
    r = post(root, f"{S}/signup/game/{badg}/yaml", files={"yaml": ("x.yaml", fixture("foo.yaml").replace(b"Pfoo", b"Bofoo"))})
    check("a moderator's good replacement releases it", "needs attention" not in r.text and "Held" not in r.text and "<h1>Bofoo</h1>" in r.text, r.text[-400:])
    post(root, f"{S}/admin/delete-player", data={"id": badg})
    check("dismissing removes it", "Bofoo" not in get(bob, f"{S}/signup").text)
    page = get(alice, f"{S}/signup").text
    check("held signup released after its check", released and "Pfoo" in page and "held" not in page.split("Signed up")[-1], page[-600:])
    r = post(alice, f"{S}/signup/check", files={"yaml": ("foo.yaml", fixture("foo.yaml").replace(b"Pfoo", b"Pfoo2"))}).json()
    check("approved world now checks", r.get("ok"), r)

    j = upload(bob, "foo.apworld", fixture("foo-v2.apworld"))
    check("another build of a game the season has uses the season's", j.get("ok") and j.get("world") == "foo.apworld" and "already has Foo Finder" in j.get("note", ""), j)

    # --- reject
    j = upload(bob, "baz.apworld")
    post(bob, f"{S}/signup", files={"yaml": ("baz.yaml", fixture("baz.yaml"))},
         data={"is_custom": "1", "pending_world": "baz.apworld"})
    post(root, f"{S}/admin/worlds/reject", data={"name": "baz.apworld"})
    page = get(bob, f"{S}/signup").text
    check("rejected: held signup says so", "baz.apworld was rejected by root" in page, page[-500:])
    adm = get(root, f"{S}/admin").text
    check("rejected world isn't installed", "baz.apworld" not in adm.split("Installed apworlds")[1].split("Clear all apworlds")[0])
    # remove bob's dead signup
    sub_id = re.search(r'name="id" value="([0-9a-z]+)"><button class="ghost" type="submit">Remove', get(bob, f"{S}/signup").text)
    if sub_id:
        post(bob, f"{S}/signup/delete", data={"id": sub_id.group(1)})

    # --- catalog
    r = post(root, "/admin/catalog/add", files={"apworld": ("bar.apworld", fixture("bar.apworld"))},
             data={"note": "vetted"})
    check("catalog add reads the manifest", "Added Bar Finder 1.0.0 to the catalog" in r.text and "bar.apworld" in r.text, r.text[-800:])
    sha = re.search(r'name="sha256" value="([0-9a-f]{64})"', r.text).group(1)
    # The Games page, as public as the home page: vetted worlds with their
    # versions (no file details), and the active Archipelago's core games once
    # they've been listed.
    anon = srv.session()
    r = anon.get(srv.base + "/worlds")
    check("the Games page is open to anyone", r.status_code == 200 and "Bar Finder" in r.text, r.status_code)
    check("…with each vetted world's version, but not its note or file",
          '<li><span>Bar Finder</span><span class="v">1.0.0</span></li>' in r.text
          and "vetted" not in r.text.split("Custom games")[1] and "bar.apworld" not in r.text and sha not in r.text)
    core = wait_for(lambda: (t := anon.get(srv.base + "/worlds").text) and 'id="corelist"' in t and t, timeout=240, every=3)
    check("…and the core games, once listed", core and "<span>ChecksFinder</span>" in core and "Archipelago " in core,
          (core or "")[:300])
    check("…which the home page links to", 'href="/worlds"' in anon.get(srv.base + "/").text)
    page = get(bob, f"{S}/signup").text
    check("signup offers the catalog", f'data-kind="cat" data-value="{sha}" data-game="Bar Finder"><span>Bar Finder <span class="tag">custom</span></span><span class="v">1.0.0</span>' in page)
    j = post(bob, f"{S}/signup/apworld", files={"catalog": (None, sha)}).json()
    check("catalog pick installs right away", j.get("ok") and not j.get("pending") and j.get("world") == "bar.apworld", j)
    adm = get(root, f"{S}/admin").text
    check("catalog install shows provenance", "from the catalog" in adm)
    r = post(bob, f"{S}/signup/check", files={"yaml": ("bar.yaml", fixture("bar.yaml"))}).json()
    check("catalog world checks", r.get("ok"), r)

    # The wizard's starting YAMLs: written on first ask (202 until then).
    def template(sess, sid, game):
        r = None
        for _ in range(80):
            r = sess.get(srv.base + f"/s/{sid}/signup/template", params={"game": game})
            if r.status_code != 202:
                return r
            time.sleep(3)
        return r
    r = template(bob, S.removeprefix("/s/"), "Bar Finder")
    check("a custom world installed in the season has a starting YAML",
          r.status_code == 200 and b"game: Bar Finder" in r.content and b"Bar Finder:" in r.content, (r.status_code, r.content[:200]))
    r = template(bob, S.removeprefix("/s/"), "ChecksFinder")
    check("…and so does a core game", r.status_code == 200 and b"game: ChecksFinder" in r.content, (r.status_code, r.content[:200]))
    r = bob.get(srv.base + f"{S}/signup/template", params={"game": "No Such Game"})
    check("…but not a game the season can't play", r.status_code == 404, r.status_code)
    page = get(bob, f"{S}/signup").text
    check("the wizard lists the season's custom world with its version",
          re.search(r'data-kind="season" data-value="Bar Finder" data-game="Bar Finder"><span>Bar Finder <span class="tag">custom</span></span><span class="v">1.0.0</span>', page), page[-1500:])
    check("installed upload can be added to the catalog (button shown)", "add to catalog" in adm)
    post(root, f"{S}/admin/worlds/to-catalog", data={"name": "foo.apworld"})
    cat = get(root, "/admin/catalog").text
    check("season world added to the catalog", "foo.apworld" in cat and "Foo Finder" in cat)

    # a second season: an upload matching a catalog entry needs no approval
    sid2 = new_season(root, "Summer")
    post(root, f"/s/{sid2}/admin/signups", data=dict(open="1"))
    j = post(alice, f"/s/{sid2}/signup/apworld", files={"apworld": ("foo.apworld", fixture("foo.apworld"))}).json()
    check("upload matching the catalog installs in manual mode", j.get("ok") and j.get("pending") is False, j)

    # --- auto
    post(root, "/admin/catalog/approval", data={"mode": "auto"})
    check("approval toggled to auto", 'value="auto" checked' in get(root, "/admin/catalog").text)
    j = post(alice, f"/s/{sid2}/signup/apworld", files={"apworld": ("baz.apworld", fixture("baz.apworld"))}).json()
    check("auto mode installs uploads", j.get("ok") and j.get("pending") is False, j)
    adm = get(root, f"/s/{sid2}/admin").text
    check("auto install provenance", "auto-approved" in adm)
    post(root, "/admin/catalog/approval", data={"mode": "manual"})

    # --- an upload of a game the server already has uses the server's version
    def variant(name, game, version):
        """`name`'s apworld with its manifest's game and version changed."""
        src = zipfile.ZipFile(io.BytesIO(fixture(name)))
        out = io.BytesIO()
        with zipfile.ZipFile(out, "w") as z:
            for item in src.infolist():
                data = src.read(item.filename)
                if item.filename.endswith("/archipelago.json"):
                    m = json.loads(data)
                    m.update(game=game, world_version=version)
                    data = json.dumps(m).encode()
                z.writestr(item, data)
        return out.getvalue()

    pins = lambda sid: get(root, f"/s/{sid}/admin").text  # the season's installed worlds
    newer = variant("bar.apworld", "Bar Finder", "1.1.0")
    j = post(alice, f"/s/{sid2}/signup/apworld", files={"apworld": ("my-bar.apworld", newer)}).json()
    check("a newer upload of a catalog game installs the catalog's version",
          j.get("ok") and j.get("world") == "bar.apworld" and "vetted version 1.0.0" in j.get("note", ""), j)
    check("…and is passed to the admins", "passed to the server" in j.get("note", ""), j)
    check("…not installed itself", "my-bar.apworld" not in pins(sid2))
    cat = get(root, "/admin/catalog").text
    check("the catalog page offers it as a replacement, with its file to vet",
          "Newer versions players uploaded" in cat and "1.1.0" in cat and "replace the catalog" in cat and "/admin/catalog/offer/download?sha256=" in cat)
    j = post(alice, f"/s/{sid2}/signup/apworld", files={"apworld": ("old-bar.apworld", variant("bar.apworld", "Bar Finder", "0.9.0"))}).json()
    check("an older upload uses what the season has, and isn't offered",
          j.get("ok") and j.get("world") == "bar.apworld" and "already has Bar Finder 1.0.0" in j.get("note", "") and "passed to" not in j.get("note", ""), j)
    j = post(alice, f"/s/{sid2}/signup/apworld", files={"apworld": ("cf.apworld", variant("bar.apworld", "ChecksFinder", "9.9.9"))}).json()
    check("an upload of a core game installs nothing: the core game is used",
          j.get("ok") and j.get("world") == "" and "core games" in j.get("note", "") and "cf.apworld" not in pins(sid2), j)

    # Public notes: set on the catalog page, shown on the Games page; the
    # admins' own note isn't.
    r = post(root, "/admin/catalog/notes", data={"sha256": sha, "note": "from the dev's repo", "public_note": "Plays best <with> a friend"})
    check("a catalog world's notes can be edited", "Notes saved." in r.text)
    games = anon.get(srv.base + "/worlds").text
    check("the Games page shows the public note, escaped",
          '<span class="note">Plays best &lt;with&gt; a friend</span>' in games and "from the dev" not in games)
    offer = re.search(r'action="/admin/catalog/offer/promote".*?name="sha256" value="([0-9a-f]{64})"', cat, re.S).group(1)
    d = root.get(srv.base + f"/admin/catalog/offer/download?sha256={offer}")
    check("a sysadmin can download an offer to vet it", d.status_code == 200 and d.content == newer, d.status_code)
    check("…and nobody else can", alice.get(srv.base + f"/admin/catalog/offer/download?sha256={offer}").content != newer)
    r = post(root, "/admin/catalog/offer/promote", data={"sha256": offer})
    check("promoting the offer replaces the catalog's version",
          "It replaces version 1.0.0" in r.text and "Bar Finder 1.1.0" in r.text, re.findall(r'class="banner[^"]*">[^<]*', r.text))
    cat = get(root, "/admin/catalog").text
    check("…the old entry and the offer are gone", sha not in cat and "Newer versions players uploaded" not in cat)
    j = post(alice, f"/s/{sid2}/signup/apworld", files={"apworld": ("bar.apworld", fixture("bar.apworld"))}).json()
    check("a season that has the game keeps its version", j.get("ok") and "already has Bar Finder 1.0.0" in j.get("note", ""), j)
    check("…with one world for the game", "my-bar.apworld" not in pins(sid2))
    j = post(bob, f"/s/{sid2}/signup/apworld", files={"apworld": ("bar.apworld", newer)}).json()
    check("an upload of the catalog's newer version replaces the season's at once",
          j.get("ok") and j.get("pending") is False and j.get("world") == "bar.apworld" and "in place of the season's 1.0.0" in j.get("note", ""), j)
    check("…installed as the catalog's", open(os.path.join(srv.data, "seasons", sid2, "worlds", "bar.apworld"), "rb").read() == newer
          and "from the catalog" in pins(sid2))
    sid3 = new_season(root, "Autumn")
    post(root, f"/s/{sid3}/admin/signups", data=dict(open="1"))
    j = post(alice, f"/s/{sid3}/signup/apworld", files={"apworld": ("bar.apworld", fixture("bar.apworld"))}).json()
    check("…and a new season gets the new version", j.get("ok") and "vetted version 1.1.0" in j.get("note", ""), j)
    r = post(root, "/admin/catalog/add", files={"apworld": ("bar.apworld", fixture("bar.apworld"))})
    check("adding an older version of a catalog game is refused", "isn't newer" in r.text and "Remove that entry first" in r.text)
    # Offers: still made when the season has the game; one per player and game.
    j = post(alice, f"/s/{sid2}/signup/apworld", files={"apworld": ("b12.apworld", variant("bar.apworld", "Bar Finder", "1.2.0"))}).json()
    check("a newer upload is offered even when the season has the game (and would replace it)",
          j.get("pending") and "newer than the season's 1.1.0" in j.get("note", "") and "passed to the server" in j.get("note", ""), j)
    post(alice, f"/s/{sid2}/signup/apworld", files={"apworld": ("b13.apworld", variant("bar.apworld", "Bar Finder", "1.3.0"))})
    cat = get(root, "/admin/catalog").text
    check("…and a player's newer upload replaces their offer", "b13.apworld" in cat and "b12.apworld" not in cat)
    for o in re.findall(r'action="/admin/catalog/offer/dismiss".*?name="sha256" value="([0-9a-f]{64})"', cat, re.S):
        post(root, "/admin/catalog/offer/dismiss", data={"sha256": o})
    check("offers can be dismissed", "Newer versions players uploaded" not in get(root, "/admin/catalog").text)
    check("the Games page shows the new version, keeping the game's public note",
          '<li><span>Bar Finder<span class="note">Plays best &lt;with&gt; a friend</span></span><span class="v">1.1.0</span></li>' in anon.get(srv.base + "/worlds").text)

    # Uploads waiting for approval: one world per game holds there too.
    sid4 = new_season(root, "Late")
    # Fresh players: uploads are limited per user, and alice and bob have made many.
    dave, erin = srv.invite_user(root, "dave"), srv.invite_user(root, "erin")
    post(root, f"/s/{sid4}/admin/signups", data=dict(open="1"))
    j = post(dave, f"/s/{sid4}/signup/apworld", files={"apworld": ("late1.apworld", variant("baz.apworld", "Late Game", "1.0.0"))}).json()
    check("an upload of a new game waits for approval", j.get("ok") and j.get("pending") and j.get("world") == "late1.apworld", j)
    j = post(erin, f"/s/{sid4}/signup/apworld", files={"apworld": ("late2.apworld", variant("baz.apworld", "Late Game", "1.1.0"))}).json()
    check("another upload of that game waits for the first",
          j.get("ok") and j.get("pending") and j.get("world") == "late1.apworld" and "already waiting" in j.get("note", ""), j)
    # The game joins the catalog meanwhile, and a pick installs the catalog's copy.
    post(root, "/admin/catalog/add", files={"apworld": ("late.apworld", variant("baz.apworld", "Late Game", "2.0.0"))})
    j = post(erin, f"/s/{sid4}/signup/apworld", files={"apworld": ("late3.apworld", variant("baz.apworld", "Late Game", "1.2.0"))}).json()
    check("…the catalog's version is installed for the game", j.get("ok") and "vetted version 2.0.0" in j.get("note", ""), j)
    r = post(root, f"/s/{sid4}/admin/worlds/approve", data={"name": "late1.apworld"})
    check("…so approving the waiting upload is refused", "already has Late Game installed" in r.text, re.findall(r'class="banner[^"]*">[^<]*', r.text))
    installed = os.listdir(os.path.join(srv.data, "seasons", sid4, "worlds"))
    check("…and the season keeps one world for the game", "late1.apworld" not in installed and "late.apworld" in installed, installed)

    # --- a newer version of a game the season has replaces it once approved,
    # and the season's signups for that game are checked against it again.
    # (An apworld's file is named after the package in it, so a newer version
    # comes under the same name.)
    def foo_yaml(name, extra=b""):
        return fixture("foo.yaml").replace(b"Pfoo", name.encode()) + extra
    held_in = lambda page, name: re.search(rf"<span>{name} · [^<]*<span[^>]*>held", page)
    listed = lambda page, name: f"<span>{name} · " in page
    worlds_dir = os.path.join(srv.data, "seasons", S.removeprefix("/s/"), "worlds")
    installed_foo = lambda: open(os.path.join(worlds_dir, "foo.apworld"), "rb").read()
    pinned = b"requires:\n  game:\n    Foo Finder:\n      max: 1.0.0\n"
    r = post(root, f"{S}/signup", files={"yaml": ("r.yaml", foo_yaml("Rfoo", pinned))}, data={"is_custom": "1"})
    check("a YAML pinned to Foo Finder 1.0.0 signs up", "<h1>Rfoo</h1>" in r.text and "held" not in r.text.split("<h1>Rfoo</h1>")[1][:300], r.text[-400:])
    foo11 = variant("foo.apworld", "Foo Finder", "1.1.0")
    j = upload(bob, "foo.apworld", foo11)
    check("a newer version of the season's game waits for approval to replace it",
          j.get("ok") and j.get("pending") and j.get("world") == "foo.apworld" and "newer than the season's 1.0.0" in j.get("note", ""), j)
    r = post(bob, f"{S}/signup", files={"yaml": ("b.yaml", foo_yaml("Bfoo"))}, data={"is_custom": "1", "pending_world": "foo.apworld"})
    check("…and its uploader's signup is held for it", "held until a moderator approves foo.apworld" in r.text, r.text[-400:])
    check("…while the season keeps its version", installed_foo() == fixture("foo.apworld"))
    check("…which the admin page shows", held_in(get(root, f"{S}/admin").text, "Bfoo"))
    post(root, f"{S}/admin/worlds/approve", data={"name": "foo.apworld"})
    check("approving it replaces the season's version", installed_foo() == foo11)
    flagged = wait_for(lambda: "no later than 1.0.0" in (t := get(root, f"{S}/admin").text) and t, 180, 2)
    check("a signup whose YAML fails the new version is flagged", flagged and "Rfoo" in flagged.split("no later than 1.0.0")[0][-600:], (flagged or "")[-600:])
    adm = wait_for(lambda: not held_in(t := get(root, f"{S}/admin").text, "Pfoo") and not held_in(t, "Bfoo") and t, 180, 2)
    check("…the others go ahead on it", adm and listed(adm, "Pfoo") and listed(adm, "Bfoo"))
    check("…while the flagged one stays held", adm and held_in(adm, "Rfoo"))
    j = upload(bob, "fooold.apworld", fixture("foo.apworld"))
    check("an older version then uses the season's", j.get("ok") and j.get("world") == "foo.apworld" and "already has Foo Finder 1.1.0" in j.get("note", ""), j)
    # A rejected newer version: its signups use the season's version instead.
    upload(bob, "foo.apworld", variant("foo.apworld", "Foo Finder", "1.2.0"))
    r = post(bob, f"{S}/signup", files={"yaml": ("b2.yaml", foo_yaml("Bfoo2"))}, data={"is_custom": "1", "pending_world": "foo.apworld"})
    check("a second newer version holds its signup too", "held until a moderator approves foo.apworld" in r.text, r.text[-400:])
    check("…which the admin page shows", held_in(get(root, f"{S}/admin").text, "Bfoo2"))
    post(root, f"{S}/admin/worlds/reject", data={"name": "foo.apworld"})
    adm = wait_for(lambda: listed(t := get(root, f"{S}/admin").text, "Bfoo2") and not held_in(t, "Bfoo2") and t, 180, 2)
    check("rejecting a newer version moves its signups to the season's", adm and installed_foo() == foo11)
    rid = re.search(r'href="signup/game/([0-9a-z]+)">Rfoo<', get(root, f"{S}/signup").text)
    post(root, f"{S}/admin/delete-player", data={"id": rid.group(1) if rid else ""})

    # remove from catalog; installed copies stay
    newsha = re.search(r'<tr><td[^>]*>Bar Finder.*?action="/admin/catalog/remove".*?name="sha256" value="([0-9a-f]{64})"', get(root, "/admin/catalog").text, re.S)
    post(root, "/admin/catalog/remove", data={"sha256": newsha.group(1) if newsha else sha})
    check("removed from catalog", "Bar Finder" not in get(root, "/admin/catalog").text)
    check("season keeps its copy", "bar.apworld" in get(root, f"{S}/admin").text)

    # generate the first season now nothing is held
    gen = generate(root, S.removeprefix("/s/"), 240)
    check("generates once nothing is held", gen and gen.startswith("Generated"), gen)
finally:
    srv.stop()

sys.exit(summary())
