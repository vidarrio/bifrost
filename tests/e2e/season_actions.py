"""What a moderator can do to a season at each stage: delete it before
generation, cancel a live one back to signups, delete an archived one (sysadmins
only), and nothing that wipes part of a running season."""
import os
import re
import sqlite3
import sys

from lib import Server, ap_servers, check, first_game, get, new_season, post, summary, wait_for

YAML = b"name: %s\ngame: ChecksFinder\nChecksFinder: {}\n"
srv = Server("season_actions", port=18192, ap_ports="39208-39210")
srv.start()
try:
    root = srv.setup_sysadmin()
    mod, ann, cat, late = (srv.invite_user(root, n) for n in ("mod", "ann", "cat", "late"))
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as db:
        db.execute("UPDATE account SET role = 'moderator' WHERE username = 'mod'")
    dirs = lambda s: os.path.join(srv.data, "seasons", s)

    # Before generation.
    sid = new_season(mod, "Draft one")
    check("the season has a directory", os.path.isdir(dirs(sid)))
    page = get(mod, f"/s/{sid}/admin").text
    check("no partial wipes on offer", all(w not in page for w in ("Full reset", "Clear session save", "Clear generated world", "Clear current players")))
    r = post(mod, f"/s/{sid}/admin/season/delete", data=dict(name="wrong"))
    check("deleting needs the season's name", "type the season" in r.text and get(mod, f"/s/{sid}/admin").status_code == 200, r.text[-200:])
    post(ann, f"/s/{sid}/admin/season/delete", data=dict(name="Draft one"))
    check("a player can't delete it", os.path.isdir(dirs(sid)))
    r = post(mod, f"/s/{sid}/admin/season/delete", data=dict(name="Draft one"))
    check("its moderator can", get(mod, f"/s/{sid}/admin").status_code == 404 and not os.path.exists(dirs(sid)), r.url)

    # A live season.
    sid = new_season(mod, "Live one")
    post(mod, f"/s/{sid}/admin/signups", data=dict(open="1"))
    post(ann, f"/s/{sid}/signup", files={"yaml": ("y.yaml", YAML % b"Ann")})
    game = re.search(r'href="signup/game/([0-9a-z]+)">Ann<', get(ann, f"/s/{sid}/signup").text).group(1)
    url = f"/s/{sid}/signup/game/{game}"
    post(ann, f"{url}/notes", data=dict(notes="Kept notes"))
    post(ann, f"{url}/invite")
    token = re.search(r"/seat/([0-9a-f]+)", get(ann, url).text).group(1)
    post(cat, f"/s/{sid}/seat/{token}", data=dict(name="Cat"))
    apworld = os.path.join(srv.data, "seasons", sid, "worlds", "x.apworld")
    post(mod, f"/s/{sid}/admin/generate")
    wait_for(lambda: "Generated seed" in get(mod, f"/s/{sid}/admin").text, 300, 3)
    check("the board comes up", wait_for(lambda: first_game(mod, sid) == game, 60, 2))
    live_url = f"/s/{sid}/game/{game}"
    wait_for(lambda: "Create an invite link" in get(ann, live_url).text, 60, 2)
    post(ann, f"{live_url}/invite")
    token = re.search(r"/seat/([0-9a-f]+)", get(ann, live_url).text).group(1)
    post(late, f"/s/{sid}/seat/{token}", data=dict(name="Late"))
    check("someone joined mid-season", "Late" in get(mod, f"/s/{sid}/claim").text)

    open(apworld, "wb").close()
    r = post(mod, f"/s/{sid}/admin/delete-apworld", data=dict(name="x.apworld"))
    check("apworlds are locked once live", os.path.exists(apworld) and "locked" in r.text, r.text[-200:])
    r = post(mod, f"/s/{sid}/admin/clear", data=dict(what="apworlds"))
    check("…all of them", os.path.exists(apworld) and "locked" in r.text, r.text[-200:])
    os.remove(apworld)
    for what in ("players", "save", "world", "reset"):
        post(mod, f"/s/{sid}/admin/clear", data=dict(what=what))
    check("the old wipes do nothing", "Ann" in get(mod, f"/s/{sid}/claim").text and "Generated seed" in get(mod, f"/s/{sid}/admin").text)
    r = post(mod, f"/s/{sid}/admin/season/delete", data=dict(name="Live one"))
    check("a live season can't be deleted", "Cancel generation" in r.text and os.path.isdir(dirs(sid)), r.text[-200:])
    r = post(mod, f"/s/{sid}/admin/cancel", data=dict(name="wrong"))
    check("cancelling needs the season's name", "type the season" in r.text and first_game(mod, sid) == game, r.text[-200:])
    post(ann, f"/s/{sid}/admin/cancel", data=dict(name="Live one"))
    check("a player can't cancel", first_game(mod, sid) == game)
    post(mod, f"/s/{sid}/admin/cancel", data=dict(name="Live one"))
    page = get(ann, f"/s/{sid}/signup").text
    check("cancelling reopens signups", 'id="addgame"' in page, page[-300:])
    game_page = get(ann, url).text
    check("…keeping the signup and its notes", "Kept notes" in game_page)
    check("…and partners from before generation", "Cat" in game_page)
    check("…but not who joined mid-season", "Late" not in game_page)
    check("…the slots are gone", first_game(mod, sid) is None)
    output = os.path.join(dirs(sid), "output")
    check("…and the seed", not [n for n in os.listdir(output) if n.endswith(".zip")], os.listdir(output))
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as db:
        row = db.execute("SELECT state, seed, ap_port FROM season WHERE id = ?", (sid,)).fetchone()
    check("…and its claim on a port", row == ("signup", None, None), row)
    check("the server has stopped", wait_for(lambda: not ap_servers(srv), 30, 1), ap_servers(srv))

    # It can go round again, then be archived.
    post(mod, f"/s/{sid}/admin/generate")
    wait_for(lambda: "Generated seed" in get(mod, f"/s/{sid}/admin").text, 300, 3)
    check("it generates again, as the same game", wait_for(lambda: first_game(mod, sid) == game, 60, 2))
    check("the partner still has their seat", "Cat" in get(mod, f"/s/{sid}/claim").text)
    archived = wait_for(lambda: "isn't connected" not in post(mod, f"/s/{sid}/admin/archive-season", data=dict(name="Live one")).text, 60, 3)
    check("it archives", archived and "summary" in get(mod, "/admin/seasons").text)

    # Archived.
    page = get(mod, f"/s/{sid}/admin").text
    check("its moderator isn't offered deleting it", "Delete this season" not in page)
    post(mod, f"/s/{sid}/admin/season/delete", data=dict(name="Live one"))
    check("…and can't", os.path.isdir(dirs(sid)))
    check("a sysadmin is", "Delete this season" in get(root, f"/s/{sid}/admin").text)
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as db:
        seed = db.execute("SELECT seed FROM season WHERE id = ?", (sid,)).fetchone()[0]
    bundle = os.path.join(srv.data, "archives", f"{seed}.tar.gz")
    check("it has a bundle", os.path.isfile(bundle), bundle)
    post(root, f"/s/{sid}/admin/season/delete", data=dict(name="Live one"))
    check("a sysadmin deletes it", get(root, f"/s/{sid}/season").status_code == 404 and not os.path.exists(dirs(sid)))
    check("…and its bundle", not os.path.exists(bundle))
    check("…and it's gone from the home page", "Live one" not in get(root, "/").text)
finally:
    srv.stop()

sys.exit(summary())
