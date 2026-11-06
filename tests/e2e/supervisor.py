"""The AP server supervisor: a live season's server that dies is restarted
(and the board reconnects); a console command reaches it; SIGTERM shuts
Bifrost down cleanly, stopping the server first."""
import os
import re
import sqlite3
import signal
import subprocess
import sys
import time

from lib import Server, ap_servers, check, first_game, get, new_season, post, summary, wait_for

YAML = b"name: Ann\ngame: ChecksFinder\nChecksFinder: {}\n"


def server_pid(srv):
    found = ap_servers(srv)
    return found[0][0] if found else None


srv = Server("supervisor", port=18192, ap_ports="39172-39174")
srv.start()
try:
    root = srv.setup_sysadmin()
    iso = re.search(r"isolation: (.*)", srv.logtext())
    check("jobs run under the syscall filter", iso and "syscall filter" in iso.group(1) and "no syscall filter" not in iso.group(1),
          iso and iso.group(1))
    sid = new_season(root, "Super")
    post(root, f"/s/{sid}/admin/signups", data=dict(open="1"))
    post(root, f"/s/{sid}/signup", files={"yaml": ("y.yaml", YAML)})
    # Signup files are per signup: a refused duplicate name, or a name that
    # makes the same file name, leaves the first one's YAML alone.
    bob = srv.invite_user(root, "bob")
    subs = os.path.join(srv.data, "seasons", str(sid), "submissions")
    r = post(bob, f"/s/{sid}/signup", files={"yaml": ("y.yaml", YAML.replace(b"ChecksFinder: {}", b"ChecksFinder: {}\n# bob"))})
    check("a duplicate slot name is refused", "already taken" in r.text, r.text[-300:])
    yamls = sorted(os.listdir(subs))
    check("…and leaves the first signup's YAML as it was",
          len(yamls) == 1 and open(os.path.join(subs, yamls[0]), "rb").read() == YAML, yamls)
    for name in (b"Bo b", b"Bo_b"):  # both make the file name Bo_b
        r = post(bob, f"/s/{sid}/signup", files={"yaml": ("y.yaml", b"name: " + name + b"\ngame: ChecksFinder\nChecksFinder: {}\n")})
        check(f"{name.decode()} signs up", f"Signed up: {name.decode()}" in r.text, r.text[-300:])
    check("…each into a file of its own", len(os.listdir(subs)) == 3, os.listdir(subs))
    # A signup is one game, so its file can't carry more players past the limit.
    two = b"name: One\ngame: ChecksFinder\nChecksFinder: {}\n---\nname: Two\ngame: ChecksFinder\nChecksFinder: {}\n"
    r = post(bob, f"/s/{sid}/signup", files={"yaml": ("y.yaml", two)})
    check("a file with several players is refused", "several players" in r.text and len(os.listdir(subs)) == 3, r.text[-300:])
    j = post(bob, f"/s/{sid}/signup/check", files={"yaml": ("y.yaml", two)}).json()
    check("…and the check step says so", not j["ok"] and "several players" in j["error"], j)
    j = post(bob, f"/s/{sid}/signup/check", files={"yaml": ("y.yaml", YAML)}).json()
    check("the check step flags a slot name that's taken", not j["ok"] and "already taken" in j["error"], j)
    # The seasons open for signup, and a game's own page.
    page = get(bob, "/signups").text
    check("the list shows the season", f'href="/s/{sid}/signup"' in page and "Super" in page, page[-300:])
    game = re.search(r'href="signup/game/([0-9a-z]+)">Bo b<', get(bob, f"/s/{sid}/signup").text).group(1)
    url = f"/s/{sid}/signup/game/{game}"
    post(bob, f"{url}/notes", data=dict(notes="No <b>double</b> jump"))
    check("the host's notes show on the card, escaped", "No &lt;b&gt;double&lt;/b&gt; jump" in root.get(root.base + f"/s/{sid}/signup").text)  # not lib.get, which decodes
    post(cat_early := srv.invite_user(root, "catearly"), f"{url}/notes", data=dict(notes="stranger was here"))
    check("nobody else can change them", "stranger was here" not in get(bob, url).text)
    check("nor sees the forms", 'action="signup/game' not in get(cat_early, url).text and "double" in get(cat_early, url).text)
    post(root, f"{url}/notes", data=dict(notes="root was here"))
    check("a moderator can edit them", "root was here" in get(bob, url).text)
    post(root, f"{url}/notes", data=dict(notes="No <b>double</b> jump"))
    r = post(bob, f"{url}/notes", data=dict(notes="x" * 2001))
    check("notes have a limit", "at most 2000" in r.text)
    before = set(os.listdir(subs))
    r = post(cat_early, f"{url}/yaml", files={"yaml": ("y.yaml", b"name: Stolen\ngame: ChecksFinder\nChecksFinder: {}\n")})
    check("only the host or a moderator can replace the YAML", "can't replace" in r.text and set(os.listdir(subs)) == before, r.text[-300:])
    r = post(bob, f"{url}/yaml", files={"yaml": ("y.yaml", YAML)})
    check("a replacement can't take another game's name", "already taken" in r.text and set(os.listdir(subs)) == before, r.text[-300:])
    r = post(bob, f"{url}/yaml", files={"yaml": ("y.yaml", b"name: Bo c\ngame: ChecksFinder\nChecksFinder: {}\n")})
    check("the host replaces it, renaming the slot", "<h1>Bo c</h1>" in r.text and "Save notes" in r.text and "Replace the YAML" in r.text, r.text[-300:])
    check("…leaving one file for it", len(os.listdir(subs)) == len(before), os.listdir(subs))
    r = post(root, f"{url}/yaml", files={"yaml": ("y.yaml", b"name: Bo m\ngame: ChecksFinder\nChecksFinder: {}\n")})
    check("a moderator can too", "<h1>Bo m</h1>" in r.text, r.text[-300:])
    post(bob, f"{url}/yaml", files={"yaml": ("y.yaml", b"name: Bo c\ngame: ChecksFinder\nChecksFinder: {}\n")})
    check("…and keeping its notes", "double" in get(bob, url).text and "Bo c" in get(bob, url).text)
    check("the game page escapes them too", "<b>double" not in bob.get(bob.base + url).text)
    other = new_season(root, "Elsewhere")
    check("a game isn't found through another season", "double" not in get(bob, f"/s/{other}/signup/game/{game}").text)
    post(bob, f"/s/{other}/signup/game/{game}/notes", data=dict(notes="elsewhere"))
    check("…nor changed", "elsewhere" not in get(bob, url).text)
    check("a game's page needs a sign-in", "double" not in get(srv.session(), url).text)
    check("the signup form has no co-op step", 'name="coop"' not in get(root, f"/s/{sid}/signup").text)
    for bad, why in ((b"Player{number}", "placeholders"), (b"50%%", "placeholders"), (b"seventeen letters", "too long"), (b"ANN", "already taken"), (b"123", "plain"), (b"[A, B]", "plain")):
        y = b"name: " + bad + b"\ngame: ChecksFinder\nChecksFinder: {}\n"
        r = post(cat_early, f"/s/{sid}/signup", files={"yaml": ("y.yaml", y)})
        check(f"the name {bad.decode()} is refused", why in r.text, r.text[-300:])
    y = b"name: Trig\ngame: ChecksFinder\nChecksFinder: {}\ntriggers:\n  - option_category: ChecksFinder\n    option_name: x\n    option_result: y\n    options:\n      null:\n        name: Other\n"
    j = post(cat_early, f"/s/{sid}/signup/check", files={"yaml": ("y.yaml", y)}).json()
    check("a name its triggers can change is refused", not j["ok"] and "triggers" in j.get("error", ""), j)
    j = post(cat_early, f"/s/{sid}/signup/check", files={"yaml": ("y.yaml", b"name: Player{number}\ngame: ChecksFinder\nChecksFinder: {}\n")}).json()
    check("…at the check step too", not j["ok"] and "placeholders" in j["error"], j)
    # Co-op: one-use links from the host, a name picked by the partner.
    cat, dee = srv.invite_user(root, "cat"), srv.invite_user(root, "dee")
    def new_link():
        before = set(re.findall(r"/seat/([0-9a-f]+)", get(bob, url).text))
        post(bob, f"{url}/invite")
        return (set(re.findall(r"/seat/([0-9a-f]+)", get(bob, url).text)) - before).pop()
    post(root, f"{url}/invite")
    check("only the host can invite", not re.findall(r"/seat/[0-9a-f]+", get(bob, url).text) and "Create an invite link" not in get(root, url).text)
    link = f"/s/{sid}/seat/" + new_link()
    page = get(cat, link).text
    check("a link says who invites you to what", "bob has invited you to join them for" in page and "ChecksFinder" in page and "disabled" in page, page[-400:])
    r = post(cat, link, data=dict(name="Ann"))
    check("a seat can't take a name in use", "already taken" in r.text, r.text[-300:])
    check("a link needs a sign-in", "invited you" not in get(srv.session(), link).text)
    r = post(cat, link, data=dict(name="seventeen letters"))
    check("a seat name follows the rules", "1 to 16" in r.text, r.text[-300:])
    r = post(cat, link, data=dict(name="Cat"))
    check("the partner joins", "Cat" in r.text and "<h1>Bo c</h1>" in r.text, r.text[-300:])
    check("…shown on the card", "+ Cat" in get(root, f"/s/{sid}/signup").text)
    r = post(dee, link, data=dict(name="Dee"))
    check("a link works once", "used or revoked" in r.text, r.text[-300:])
    post(cat, f"{url}/notes", data=dict(notes="Shared notes"))
    check("a partner edits the same notes", "Shared notes" in get(bob, url).text)
    check("…but can't replace the YAML", "Replace the YAML" not in get(cat, url).text and "Create an invite link" not in get(cat, url).text)
    seat = re.search(r'name="seat" value="([0-9a-z]+)"', get(cat, url).text).group(1)
    post(cat, f"{url}/seat/rename", data=dict(seat=seat, name="Kitty"))
    check("a partner renames their seat", "Kitty" in get(bob, url).text)
    r = post(cat, f"{url}/seat/rename", data=dict(seat=seat, name="KITTY"))
    check("…and can keep it, or change its case", "KITTY" in get(bob, url).text, r.text[-300:])
    post(cat, f"{url}/seat/rename", data=dict(seat=seat, name="Kitty"))
    r = post(cat, f"{url}/seat/rename", data=dict(seat=seat, name="Ann"))
    check("…but not to a name in use", "already taken" in r.text, r.text[-300:])
    r = post(dee, f"/s/{sid}/signup", files={"yaml": ("y.yaml", b"name: Kitty\ngame: ChecksFinder\nChecksFinder: {}\n")})
    check("a signup can't take a seat's name", "already taken" in r.text, r.text[-300:])
    link = f"/s/{sid}/seat/" + new_link()
    post(dee, link, data=dict(name="Dee"))
    dseat = [s for s in re.findall(r'name="seat" value="([0-9a-z]+)"', get(bob, url).text) if s != seat][0]
    post(cat, f"{url}/seat/remove", data=dict(seat=dseat))
    check("a partner can't remove anyone", "Dee" in get(bob, url).text)
    post(bob, f"{url}/seat/remove", data=dict(seat=dseat))
    check("the host can, before generation", "Dee" not in get(bob, url).text)
    link = f"/s/{sid}/seat/" + new_link()
    post(dee, link, data=dict(name="Dee"))
    check("rejoined", "Dee" in get(bob, url).text)
    post(dee, f"{url}/seat/leave")
    check("a partner can leave", "Dee" not in get(bob, url).text)
    token = new_link()
    post(bob, f"{url}/revoke", data=dict(token=token))
    check("the host revokes an unused link", "used or revoked" in get(dee, f"/s/{sid}/seat/{token}").text)
    r = post(dee, f"/s/{sid}/signup", files={"yaml": ("y.yaml", b"name: DeeCo\ngame: ChecksFinder\nChecksFinder: {}\n")})
    check("signing up leads to your game's page", "Signed up: DeeCo" in r.text and "Create an invite link" in r.text and "/signup/game/" in r.url, r.url)
    deeg = re.search(r'href="signup/game/([0-9a-z]+)">DeeCo<', get(dee, f"/s/{sid}/signup").text).group(1)
    post(dee, f"/s/{sid}/signup/game/{deeg}/invite")
    post(cat, f"/s/{sid}/seat/" + re.search(r"/seat/([0-9a-f]+)", get(dee, f"/s/{sid}/signup/game/{deeg}").text).group(1), data=dict(name="<b>x"))
    check("a seat's name is escaped", "&lt;b&gt;x" in cat.get(cat.base + f"/s/{sid}/signup").text and "<b>x" not in cat.get(cat.base + f"/s/{sid}/signup/game/{deeg}").text)
    check("a numeric season URL doesn't exist", get(root, "/s/1/signup").status_code == 404 and get(root, "/s/1/board").status_code == 404)
    check("nor a numeric game", get(root, f"/s/{sid}/signup/game/1").status_code in (400, 404))
    # Who can see a season: signups are open, pages after generation aren't.
    stranger = srv.invite_user(root, "stranger")
    check("a stranger sees open signups", "Bo c" in get(stranger, f"/s/{sid}/signup").text)
    check("…but an anonymous visitor doesn't, even on the board", "Bo c" not in get(srv.session(), f"/s/{sid}/board").text and "Bo c" not in get(srv.session(), "/").text)
    priv = other
    post(root, f"/s/{priv}/admin/signups", data=dict(open="1"))
    post(root, f"/s/{priv}/admin/privacy", data=dict(what="signup", private="1"))
    post(stranger, f"/s/{priv}/admin/privacy", data=dict(what="signup", private="0"))
    check("a stranger can't reach private signups", get(stranger, f"/s/{priv}/signup").status_code == 404)
    check("…and they aren't listed", f"/s/{priv}/signup" not in get(stranger, "/signups").text)
    check("…except to a moderator", f"/s/{priv}/signup" in get(root, "/signups").text)
    join = re.search(r"/signup/join/[0-9a-f]+", get(root, f"/s/{priv}/admin").text).group(0)
    check("the link is on the options tab", join)
    r = get(dee, f"/s/{priv}{join}")
    check("the link lets you in", r.status_code == 200 and f"/s/{priv}/signup" in r.url, r.url)
    check("…and keeps you in", get(dee, f"/s/{priv}/signup").status_code == 200)
    post(root, f"/s/{priv}/admin/privacy", data=dict(what="signup", private="1"))
    r = get(stranger, f"/s/{priv}{join}")
    check("a new link ends the old one", "isn't valid" in r.text and get(stranger, f"/s/{priv}/signup").status_code == 404)
    check("those already in stay", get(dee, f"/s/{priv}/signup").status_code == 200)
    # A player with a game gets a button for another, within the season's limit.
    fresh = srv.invite_user(root, "fresh")
    page = get(fresh, f"/s/{sid}/signup").text
    check("no game yet: the signup form, no button", 'id="wiz"' in page and "Add another game" not in page and 'id="wizbox"' not in page)
    page = get(bob, f"/s/{sid}/signup").text
    check("with a game: Add another game", "Add another game" in page and 'id="wizbox" hidden' in page and "Add this game" in page)
    post(root, f"/s/{sid}/admin/signups/limit", data=dict(games="2"))
    check("a moderator sets the limit", "Games per player: at most <b>2</b>" in get(root, f"/s/{sid}/admin").text)
    post(bob, f"/s/{sid}/admin/signups/limit", data=dict(games="9"))
    check("a player can't", "at most <b>2</b>" in get(root, f"/s/{sid}/admin").text)
    r = post(bob, f"/s/{sid}/signup", files={"yaml": ("y.yaml", b"name: Third\ngame: ChecksFinder\nChecksFinder: {}\n")})
    check("past the limit, a game is refused", "limit of 2" in r.text, r.text[-300:])
    check("…leaving no file", len(os.listdir(subs)) == 4, os.listdir(subs))
    page = get(bob, f"/s/{sid}/signup").text
    check("…and the page says so, with no form", "limit of 2" in page and 'id="wiz"' not in page)
    r = post(root, f"/s/{sid}/admin/signups/limit", data=dict(games="-1"))
    check("a bad limit is refused", "must be a whole number" in r.text)
    post(root, f"/s/{sid}/admin/signups/limit", data=dict(games="0"))
    post(root, f"/s/{sid}/admin/generate")
    wait_for(lambda: "Generated seed" in get(root, f"/s/{sid}/admin").text, 300, 3)
    r = post(bob, f"{url}/notes", data=dict(notes="too late"))
    check("once generated the notes are read-only", "can't change" in r.text and "Save notes" not in get(bob, url).text, r.text[-300:])
    r = post(bob, f"{url}/yaml", files={"yaml": ("y.yaml", b"name: Late\ngame: ChecksFinder\nChecksFinder: {}\n")})
    check("…and the YAML can't be replaced", "can't replace" in r.text and "Replace the YAML" not in get(bob, url).text, r.text[-300:])
    connected = lambda: '"connected":true' in get(root, f"/s/{sid}/board/fragment").text.replace(" ", "") \
        or "isn't connected" not in post(root, f"/s/{sid}/admin/console", data={"cmd": "/players"}).text
    pid = wait_for(lambda: server_pid(srv), 60, 1)
    check("the season's AP server runs", pid)
    r = wait_for(lambda: (t := post(root, f"/s/{sid}/admin/console", data={"cmd": "/players"}).text) and "Ann" in t and t, 60, 3)
    check("a console command reaches it", r)
    check("all three signups made it into the seed, the replaced one by its new name", r and "Bo c" in r and "Bo_b" in r, (r or "")[-300:])
    # A new season's pages are for its members once generated.
    anon = srv.session()
    check("anonymous gets the sign-in", get(anon, f"/s/{sid}/board", allow_redirects=False).status_code in (302, 303))
    check("a stranger's told there's no such season", get(stranger, f"/s/{sid}/board").status_code == 404)
    pages = ("log", "log/fragment", "board/fragment", f"game/{game}", f"game/{game}/hints", f"game/{game}/items", f"game/{game}/inlogic", f"game/{game}/live", f"game/{game}/output", f"game/{game}/poptracker/state", f"game/{game}/poptracker/asset?path=x", f"game/{game}/log/fragment", "signup", "claim", "minesweeper", "minesweeper/leaderboard", "season", "check")
    check("…on every page", all(get(stranger, f"/s/{sid}/{p}").status_code == 404 for p in pages), [p for p in pages if get(stranger, f"/s/{sid}/{p}").status_code != 404])
    check("a bare season URL goes to the board", get(anon, f"/s/{sid}", allow_redirects=False).headers.get("location") == f"/s/{sid}/board")
    check("there's no standalone YAML checker", get(anon, "/check").status_code == 404 and get(root, f"/s/{sid}/check").status_code == 404)
    other_mod = srv.invite_user(root, "othermod")
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as db:
        db.execute("UPDATE account SET role = 'moderator' WHERE username = 'othermod'")
    r = get(other_mod, "/admin/seasons")
    check("a moderator who isn't in it doesn't see it listed", r.status_code == 200 and "Super" not in r.text, r.status_code)
    check("a signed-in player who can't see any season gets no season cards", 'class="cover"' not in get(stranger, "/").text)
    vis = new_season(root, "Visible")
    check("a draft isn't on the home page for those who don't run it", "Visible" not in get(stranger, "/").text and f'href="/s/{vis}/admin">Visible<' in get(root, "/").text)
    post(root, f"/s/{vis}/admin/signups", data=dict(open="1"))
    check("the home page shows the seasons the viewer can see", f'href="/s/{vis}/signup">Visible<' in get(stranger, "/").text)
    check("a visitor who can't see any isn't sent to a season", f"/s/{sid}/board" not in get(anon, "/").text)
    check("…and can't hint", post(stranger, f"/s/{sid}/hint", data=dict(slot_id=1, item="x")).status_code == 404)
    check("the home page hides it from them", "Super" not in get(stranger, "/").text and "Super" not in get(anon, "/").text)
    check("a player sees it", get(bob, f"/s/{sid}/board").status_code == 200 and "Super" in get(bob, "/").text)
    check("a partner sees it", get(cat, f"/s/{sid}/board").status_code == 200)
    post(root, f"/s/{sid}/admin/privacy", data=dict(what="live", private="0"))
    check("a moderator opens it", get(stranger, f"/s/{sid}/board").status_code == 200 and get(anon, f"/s/{sid}/board").status_code == 200)
    post(root, f"/s/{sid}/admin/privacy", data=dict(what="live", private="1"))
    # General minesweeper is for any account; a season's is for its players.
    check("general minesweeper works for a stranger", get(stranger, "/minesweeper").status_code == 200 and post(stranger, "/minesweeper/new", data=dict(kind="standard")).json().get("ok"))
    check("…but not anonymously", get(anon, "/minesweeper", allow_redirects=False).status_code in (302, 303))
    post(root, f"/s/{sid}/admin/privacy", data=dict(what="live", private="0"))
    check("a season's minesweeper isn't for others, even when it's open", get(stranger, f"/s/{sid}/minesweeper").status_code == 403 and post(stranger, f"/s/{sid}/minesweeper/new", data=dict(kind="standard")).status_code == 403)
    post(root, f"/s/{sid}/admin/privacy", data=dict(what="live", private="1"))
    check("a player has it", get(bob, f"/s/{sid}/minesweeper").status_code == 200)
    post(other_mod, f"{url}/notes", data=dict(notes="other mod"))
    check("a moderator of other seasons can't edit a game's notes", "other mod" not in get(bob, url).text)
    post(root, f"{url}/notes", data=dict(notes="Live edit"))
    check("the season's moderators can, once live", "Live edit" in get(bob, url).text)
    # Once live: partners seated, nobody removed, and links still work.
    slots = get(root, f"/s/{sid}/claim").text
    check("a partner's seat is theirs after generation", re.search(r"Kitty</span><span><span class=\"by\">cat", slots), slots[-800:])
    post(bob, f"/s/{sid}/signup/delete", data=dict(id=game))
    post(root, f"/s/{sid}/admin/delete-player", data=dict(id=game))
    r = post(root, f"/s/{sid}/admin/clear", data=dict(what="signups"))
    page = get(root, f"/s/{sid}/signup").text
    check("a game can't be retracted once generated, by anyone", "Bo c" in page and "signups stay" in r.text and ">Remove<" not in page, page[-300:])
    sid_slot = re.search(r'name="slot_id" value="([0-9a-z]+)"', get(bob, f"/s/{sid}/claim").text)
    for op in ("claim", "unclaim", "assign"):
        post(bob, f"/s/{sid}/claim", data=dict(slot_id=sid_slot.group(1) if sid_slot else 1, op=op, account="bob"))
    check("players can't claim, unclaim or assign", get(root, f"/s/{sid}/claim").text == slots, slots[-300:])
    r = post(root, f"/s/{sid}/claim", data=dict(slot_id=sid_slot.group(1) if sid_slot else 1, op="assign", account="dee"))
    check("a moderator can't reassign a slot with a player", "already has a player" in r.text, r.text[-300:])
    ap = game  # A signup's game keeps its id once live.
    check("the host has an invite box on the live game", wait_for(lambda: "Create an invite link" in get(bob, f"/s/{sid}/game/{ap}").text, 60, 2))
    check("a partner has no invite box on the live game", "Create an invite link" not in get(cat, f"/s/{sid}/game/{ap}").text)
    post(cat, f"/s/{sid}/game/{ap}/invite")
    post(bob, f"/s/{sid}/game/{ap}/invite")
    token = re.findall(r"/seat/([0-9a-f]+)", get(bob, f"/s/{sid}/game/{ap}").text)
    check("the host makes a link on the live game", len(set(token)) == 1, token)
    late = srv.invite_user(root, "late")
    r = post(late, f"/s/{sid}/seat/{token[0]}", data=dict(name="Late"))
    check("someone joins mid-season", "Late" in get(root, f"/s/{sid}/claim").text and r.status_code == 200, r.text[-300:])
    check("…and is on the game", "Late" in get(root, f"/s/{sid}/game/{ap}").text)
    check("…and its card", "Late" in get(root, f"/s/{sid}/signup").text)
    # Connection details are on the season's own board.
    post(root, "/admin/settings", data=dict(domain="ap.example.org", home_url="", discords=""))
    post(root, f"/s/{sid}/admin/connect-password", data=dict(password="sesame-1"))
    page = get(bob, f"/s/{sid}/board").text
    check("a player sees the address and password on the board", re.search(r"ap\.example\.org:\d+", page) and "sesame-1" in page, page[:800])
    check("so does a partner", "sesame-1" in get(cat, f"/s/{sid}/board").text)
    check("…under the board's How to join tab", 'data-tab="tab-join">How to join<' in page and re.search(r'id="tab-join" hidden>.*sesame-1', page, re.S))
    check("…which links the player's own game", f'href="game/{game}"' in page.split('id="tab-join"')[1])
    home = get(bob, "/").text
    check("the home page names a player's own game on its season's card", re.search(rf'href="/s/{sid}/board">Super</a>.*?You play <a href="/s/{sid}/game/{game}">', home, re.S))
    check("the home page points there instead", f'<a class="cover" href="/s/{sid}/board">Super</a>' in home and "sesame-1" not in home and not re.search(r"ap\.example\.org:\d+", home))
    post(root, f"/s/{sid}/admin/privacy", data=dict(what="live", private="0"))
    page = get(stranger, f"/s/{sid}/board").text
    check("on an open season others see the address, not the password", re.search(r"ap\.example\.org:\d+", page) and "sesame-1" not in page, page[:800])
    check("…told who it's for", "for this season's players" in page)
    check("…nor does an anonymous visitor", "sesame-1" not in get(srv.session(), f"/s/{sid}/board").text)
    check("…or on the live updates", "sesame-1" not in get(stranger, f"/s/{sid}/board/fragment").text)
    check("…or a moderator of other seasons", "sesame-1" not in get(other_mod, f"/s/{sid}/board").text)
    check("the season's moderators do", "sesame-1" in get(root, f"/s/{sid}/board").text)
    post(root, f"/s/{sid}/admin/privacy", data=dict(what="live", private="1"))
    # A slot with no player: a moderator assigns it.
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as db:
        free = "abcdefghjkmnpqrs"
        db.execute("INSERT INTO slot (id, season_id, game_id, name, ap_slot, game) SELECT ?, season_id, game_id, 'Free', ap_slot, game FROM slot WHERE game_id = ? LIMIT 1", (free, ap))
    post(bob, f"/s/{sid}/claim", data=dict(slot_id=free, op="assign", account="bob"))
    check("a player can't assign an empty slot", "unclaimed" in get(root, f"/s/{sid}/claim").text)
    r = post(root, f"/s/{sid}/claim", data=dict(slot_id=free, op="assign", account="nobody"))
    check("an unknown account is refused", "no account" in r.text, r.text[-300:])
    post(root, f"/s/{sid}/claim", data=dict(slot_id=free, op="assign", account="dee"))
    check("a moderator assigns it", re.search(r"Free</span><span><span class=\"by\">dee", get(root, f"/s/{sid}/claim").text))
    os.kill(pid, signal.SIGKILL)
    check("the supervisor notices the exit", wait_for(lambda: "AP server exited" in srv.logtext(), 30, 1))
    new = wait_for(lambda: (p := server_pid(srv)) and p != pid and p, 60, 1)
    check("…and restarts it", new)
    r = wait_for(lambda: (t := post(root, f"/s/{sid}/admin/console", data={"cmd": "/players"}).text) and "Ann" in t and t, 90, 3)
    check("the restarted server answers", r)
    srv.proc.send_signal(signal.SIGTERM)
    try:
        code = srv.proc.wait(40)
    except subprocess.TimeoutExpired:
        code = None
    log = srv.logtext()
    check("SIGTERM shuts Bifrost down", code is not None and "shutting down" in log and "Bifrost stopped" in log, code)
    check("…with the AP server stopped", new and not os.path.exists(f"/proc/{new}"))
finally:
    srv.stop()

sys.exit(summary())
