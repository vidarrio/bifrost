"""The native single-file bundle: unpacks its runtime once, runs checks,
generation and a live server on its own Python + Archipelago (on this host,
under bubblewrap), doesn't re-unpack on restart, installs another release
with its Python; and runs on a bare Debian with no Python at all."""
import glob
import os
import re
import subprocess
import sys
import time

import requests

from lib import REPO, Server, check, get, post, summary, wait_for

BUNDLE = sorted(glob.glob(os.path.join(REPO, "dist", "bifrost-*-x86_64-linux")))[-1]
YAML = b"name: Ann\ngame: ChecksFinder\nChecksFinder: {}\n"
print("bundle:", BUNDLE, os.path.getsize(BUNDLE) >> 20, "MiB")

srv = Server("bundle", port=18184, ap_ports="39130-39132", bin=BUNDLE, bundle=True)
srv.start()
try:
    log = srv.logtext()
    check("first run unpacks the runtime", "unpacking the bundled runtime" in log and "bundled runtime ready" in log)
    rt = glob.glob(os.path.join(srv.data, "runtime", "*", ".complete"))
    check("runtime under the data dir", len(rt) == 1, rt)
    rtdir = os.path.dirname(rt[0])
    check("runs on the bundled AP", f"bundled at {rtdir}/archipelago" in log, re.findall(r"Archipelago.*bundled at.*", log))
    check("strong isolation (host bwrap)", "isolation: strong" in log)

    root = srv.setup_sysadmin()
    post(root, "/admin/seasons", data=dict(name="Native"))
    sid = int(re.search(r"/s/(\d+)/admin", get(root, "/admin/seasons").text).group(1))
    post(root, f"/s/{sid}/admin/signups", data=dict(open="1"))
    r = post(root, f"/s/{sid}/signup/check", files={"yaml": ("y.yaml", YAML)}).json()
    check("YAML check runs on the bundled Python", r.get("ok"), r)
    post(root, f"/s/{sid}/signup", files={"yaml": ("y.yaml", YAML)})
    post(root, f"/s/{sid}/admin/generate")
    t = wait_for(lambda: (x := get(root, f"/s/{sid}/admin").text) and ("Generated seed" in x or "Failed" in x) and x, 300, 3)
    m = re.search(r"(Generated seed[^<]*|Failed[^<]*)", t or "")
    check("generation", m and m.group(0).startswith("Generated"), m and m.group(0))
    py = os.path.join(rtdir, "python/bin/python3")
    ps = lambda: subprocess.run(["ps", "-eo", "args"], capture_output=True, text=True).stdout
    check("live server on the bundled Python", wait_for(lambda: any(l.startswith(py) and "bifrost_server.py" in l for l in ps().splitlines()), 30))
    check("tracker connects", wait_for(lambda: "Ann" in get(srv.session(), f"/s/{sid}/board").text, 60))

    srv.stop()
    srv.start()
    check("restart doesn't unpack again", srv.logtext().count("unpacking the bundled runtime") == 1)
    root = srv.session()
    post(root, "/login", data=dict(username="root", password="correct horse battery"))

    post(root, "/admin/settings/ap/check")
    post(root, "/admin/settings/ap/install", data={"version": "0.6.7-bifrost.2"})
    done = wait_for(lambda: (p := get(root, "/admin/settings").text) and "this page refreshes" not in p and p, 1800, 10)
    check("installing a release works with the bundled Python", done and "passed a test generation" in done,
          re.findall(r'class="banner[^"]*">[^<]*', done or ""))
finally:
    srv.stop()

# A bare system: no Python, no bubblewrap.
data = os.path.join(srv.data, "..", "bundle-bare")
subprocess.run(["rm", "-rf", data])
os.makedirs(data)
name = "bfbare"
subprocess.run(["podman", "rm", "-f", name], capture_output=True)
r = subprocess.run(["podman", "run", "-d", "--name", name, "-p", "18191:8080",
                    "-e", "BIFROST_INSECURE_COOKIES=1", "-e", "BIFROST_DATA=/data",
                    "-v", f"{BUNDLE}:/bifrost:ro", "-v", f"{os.path.abspath(data)}:/data:U",
                    "docker.io/library/debian:bookworm-slim", "/bifrost"], capture_output=True, text=True)
check("bare container starts", r.returncode == 0, r.stderr)
try:
    base = "http://127.0.0.1:18191"

    def up():
        try:
            return requests.get(base + "/health", timeout=1).ok
        except requests.RequestException:
            return False

    ok = wait_for(up, 120)
    logs = subprocess.run(["podman", "logs", name], capture_output=True, text=True)
    text = re.sub(r"\x1b\[[0-9;]*m", "", logs.stdout + logs.stderr)
    check("runs on bare Debian bookworm", ok, text[-1500:])
    r = subprocess.run(["podman", "exec", name, "sh", "-c", "command -v python3 || echo none"], capture_output=True, text=True)
    check("…which has no Python of its own", r.stdout.strip() == "none", r.stdout)
    tok = re.search(r"/setup\?token=([0-9a-f]+)", text)
    s = requests.Session()
    s.headers["Origin"] = base
    s.base = base
    pw = "correct horse battery"
    s.post(base + "/setup", data=dict(token=tok.group(1), username="root", password=pw, confirm=pw))
    s.post(base + "/admin/seasons", data=dict(name="Bare"))
    sid = int(re.search(r"/s/(\d+)/admin", s.get(base + "/admin/seasons").text).group(1))
    s.post(base + f"/s/{sid}/admin/signups", data=dict(open="1"))
    r = s.post(base + f"/s/{sid}/signup/check", files={"yaml": ("y.yaml", YAML)}).json()
    check("YAML check on bare Debian", r.get("ok"), r)
finally:
    subprocess.run(["podman", "rm", "-f", name], capture_output=True)

sys.exit(summary())
