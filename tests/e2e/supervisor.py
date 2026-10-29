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
    check("a duplicate slot name is refused", "already signed up" in r.text, r.text[-300:])
    yamls = sorted(os.listdir(subs))
    check("…and leaves the first signup's YAML as it was",
          len(yamls) == 1 and open(os.path.join(subs, yamls[0]), "rb").read() == YAML, yamls)
    for name in (b"Bo b", b"Bo_b"):  # both make the file name Bo_b
        r = post(bob, f"/s/{sid}/signup", files={"yaml": ("y.yaml", b"name: " + name + b"\ngame: ChecksFinder\nChecksFinder: {}\n")})
        check(f"{name.decode()} signs up", f"Signed up: {name.decode()}" in r.text, r.text[-300:])
    check("…each into a file of its own", len(os.listdir(subs)) == 3, os.listdir(subs))
    post(root, f"/s/{sid}/admin/generate")
    wait_for(lambda: "Generated seed" in get(root, f"/s/{sid}/admin").text, 300, 3)
    connected = lambda: '"connected":true' in get(root, f"/s/{sid}/board/fragment").text.replace(" ", "") \
        or "isn't connected" not in post(root, f"/s/{sid}/admin/console", data={"cmd": "/players"}).text
    pid = wait_for(lambda: server_pid(srv), 60, 1)
    check("the season's AP server runs", pid)
    r = wait_for(lambda: (t := post(root, f"/s/{sid}/admin/console", data={"cmd": "/players"}).text) and "Ann" in t and t, 60, 3)
    check("a console command reaches it", r)
    check("all three signups made it into the seed", r and "Bo b" in r and "Bo_b" in r, (r or "")[-300:])
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
