"""The AP server supervisor: a live season's server that dies is restarted
(and the board reconnects); a console command reaches it; SIGTERM shuts
Bifrost down cleanly, stopping the server first."""
import os
import re
import signal
import subprocess
import sys
import time

from lib import Server, check, get, post, summary, wait_for

YAML = b"name: Ann\ngame: ChecksFinder\nChecksFinder: {}\n"


def descendants(pid):
    out, todo = [], [pid]
    while todo:
        p = todo.pop()
        r = subprocess.run(["pgrep", "-P", str(p)], capture_output=True, text=True)
        kids = [int(x) for x in r.stdout.split()]
        out += kids
        todo += kids
    return out


def server_pid(srv):
    for p in descendants(srv.proc.pid):
        try:
            cmd = open(f"/proc/{p}/cmdline", "rb").read().replace(b"\0", b" ")
        except OSError:
            continue
        if b"bifrost_server.py" in cmd and b"bwrap" not in cmd:
            return p
    return None


srv = Server("supervisor", port=18192, ap_ports="39172-39174")
srv.start()
try:
    root = srv.setup_sysadmin()
    post(root, "/admin/seasons", data=dict(name="Super"))
    sid = int(re.search(r"/s/(\d+)/admin", get(root, "/admin/seasons").text).group(1))
    post(root, f"/s/{sid}/admin/signups", data=dict(open="1"))
    post(root, f"/s/{sid}/signup", files={"yaml": ("y.yaml", YAML)})
    post(root, f"/s/{sid}/admin/generate")
    wait_for(lambda: "Generated seed" in get(root, f"/s/{sid}/admin").text, 300, 3)
    connected = lambda: '"connected":true' in get(root, f"/s/{sid}/board/fragment").text.replace(" ", "") \
        or "isn't connected" not in post(root, f"/s/{sid}/admin/console", data={"cmd": "/players"}).text
    pid = wait_for(lambda: server_pid(srv), 60, 1)
    check("the season's AP server runs", pid)
    r = wait_for(lambda: (t := post(root, f"/s/{sid}/admin/console", data={"cmd": "/players"}).text) and "Ann" in t and t, 60, 3)
    check("a console command reaches it", r)
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
