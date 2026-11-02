"""The AP server supervisor: a live season's server that dies is restarted
(and the board reconnects); a console command reaches it; SIGTERM shuts
Bifrost down cleanly, stopping the server first."""
import os
import re
import signal
import subprocess
import sys
import time

from lib import Server, ap_servers, check, get, post, summary, wait_for

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
    post(root, "/admin/seasons", data=dict(name="Super"))
    sid = int(re.search(r"/s/(\d+)/admin", get(root, "/admin/seasons").text).group(1))
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
    game = re.search(r'href="signup/game/(\d+)">Bo b<', get(bob, f"/s/{sid}/signup").text).group(1)
    url = f"/s/{sid}/signup/game/{game}"
    post(bob, f"{url}/notes", data=dict(notes="No <b>double</b> jump"))
    check("the host's notes show on the card, escaped", "No &lt;b&gt;double&lt;/b&gt; jump" in root.get(root.base + f"/s/{sid}/signup").text)  # not lib.get, which decodes
    post(root, f"{url}/notes", data=dict(notes="root was here"))
    check("nobody else can change them", "root was here" not in get(bob, url).text)
    check("nor sees the forms", 'action="signup/game' not in get(root, url).text and "double" in get(root, url).text)
    r = post(bob, f"{url}/notes", data=dict(notes="x" * 2001))
    check("notes have a limit", "at most 2000" in r.text)
    before = set(os.listdir(subs))
    r = post(root, f"{url}/yaml", files={"yaml": ("y.yaml", b"name: Stolen\ngame: ChecksFinder\nChecksFinder: {}\n")})
    check("only the host can replace the YAML", "can't replace" in r.text and set(os.listdir(subs)) == before, r.text[-300:])
    r = post(bob, f"{url}/yaml", files={"yaml": ("y.yaml", YAML)})
    check("a replacement can't take another game's name", "already taken" in r.text and set(os.listdir(subs)) == before, r.text[-300:])
    r = post(bob, f"{url}/yaml", files={"yaml": ("y.yaml", b"name: Bo c\ngame: ChecksFinder\nChecksFinder: {}\n")})
    check("the host replaces it, renaming the slot", "<h1>Bo c</h1>" in r.text and "Save notes" in r.text and "Replace the YAML" in r.text, r.text[-300:])
    check("…leaving one file for it", len(os.listdir(subs)) == len(before), os.listdir(subs))
    check("…and keeping its notes", "double" in get(bob, url).text and "Bo c" in get(bob, url).text)
    check("the game page escapes them too", "<b>double" not in bob.get(bob.base + url).text)
    post(root, "/admin/seasons", data=dict(name="Elsewhere"))
    other = max(int(n) for n in re.findall(r"/s/(\d+)/admin", get(root, "/admin/seasons").text))
    check("a game isn't found through another season", "double" not in get(bob, f"/s/{other}/signup/game/{game}").text)
    post(bob, f"/s/{other}/signup/game/{game}/notes", data=dict(notes="elsewhere"))
    check("…nor changed", "elsewhere" not in get(bob, url).text)
    check("a game's page needs a sign-in", "double" not in get(srv.session(), url).text)
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
    check("…leaving no file", len(os.listdir(subs)) == 3, os.listdir(subs))
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
