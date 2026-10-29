"""The native single-file bundle: unpacks its runtime once, runs checks,
generation and a live server on its own Python + Archipelago (on this host,
under bubblewrap), doesn't re-unpack on restart, installs another release
with its Python; and runs on a bare Debian with no Python at all."""
import glob
import hashlib
import http.server
import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time

import requests

from lib import REPO, Server, check, get, post, summary, wait_for

_built = glob.glob(os.path.join(REPO, "dist", "bifrost-*-x86_64-linux"))
if not _built:
    sys.exit("no bundle in dist/: build one with packaging/bundle/build.sh")
BUNDLE = max(_built, key=os.path.getmtime)  # the newest build
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

# `bifrost update`, against a stand-in for GitHub's API serving this very
# bundle as release 9.9.9 (and broken releases next to it).
blob = open(BUNDLE, "rb").read()
sha = hashlib.sha256(blob).hexdigest()
releases = {
    "v9.9.9": (blob, sha),
    "v9.9.8": (blob, "0" * 64),           # the checksum doesn't match
    "v9.9.7": (b"#!/bin/sh\n", None),     # not a bundle (checksum matches)
    "v9.9.6": (blob, sha),                # no SHA256SUMS.sig
    "v9.9.5": (blob, sha),                # signed with another key
}
listing = []

# Release signing, as the release workflow does it: an ed25519 key signs
# SHA256SUMS; Bifrost is pointed at the test key's public half.
keys = os.path.join(srv.data, "..", "bundle-keys")
subprocess.run(["rm", "-rf", keys])
os.makedirs(keys)
for k in ("good", "other"):
    subprocess.run(["openssl", "genpkey", "-algorithm", "ed25519", "-out", os.path.join(keys, k)], check=True)
pub = subprocess.run(["openssl", "pkey", "-in", os.path.join(keys, "good"), "-pubout", "-outform", "DER"],
                     capture_output=True, check=True).stdout[-32:].hex()


def sums_for(tag):
    body, digest = releases[tag]
    return f"{digest or hashlib.sha256(body).hexdigest()}  bifrost-{tag[1:]}-x86_64-linux\n".encode()


def sign(data, key):
    f = os.path.join(keys, "msg")
    open(f, "wb").write(data)
    return subprocess.run(["openssl", "pkeyutl", "-sign", "-rawin", "-inkey", os.path.join(keys, key), "-in", f],
                          capture_output=True, check=True).stdout


class FakeGitHub(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def send(self, body, ctype="application/octet-stream"):
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        base = f"http://127.0.0.1:{self.server.server_port}"
        m = re.fullmatch(r"/repos/vidarrio/bifrost/releases/tags/(v[\d.]+)", self.path)
        d = re.fullmatch(r"/dl/(v[\d.]+)/(.+)", self.path)
        if self.path.startswith("/repos/vidarrio/bifrost/releases?"):
            self.send(json.dumps(listing).encode(), "application/json")
        elif m and m.group(1) in releases:
            tag = m.group(1)
            names = [f"bifrost-{tag[1:]}-x86_64-linux", "SHA256SUMS"]
            if tag != "v9.9.6":
                names.append("SHA256SUMS.sig")
            assets = [{"name": n, "browser_download_url": f"{base}/dl/{tag}/{n}"} for n in names]
            self.send(json.dumps({"tag_name": tag, "assets": assets}).encode(), "application/json")
        elif d and d.group(1) in releases:
            body, _ = releases[d.group(1)]
            if d.group(2) == "SHA256SUMS":
                self.send(sums_for(d.group(1)))
            elif d.group(2) == "SHA256SUMS.sig":
                self.send(sign(sums_for(d.group(1)), "other" if d.group(1) == "v9.9.5" else "good"))
            else:
                self.send(body)
        else:
            self.send_error(404)


api = http.server.ThreadingHTTPServer(("127.0.0.1", 0), FakeGitHub)
threading.Thread(target=api.serve_forever, daemon=True).start()
upd = os.path.join(srv.data, "..", "bundle-update")
subprocess.run(["rm", "-rf", upd])
os.makedirs(upd)
exe = os.path.join(upd, "bifrost")
shutil.copy2(BUNDLE, exe)
env = dict(srv.env, BIFROST_GITHUB_API=f"http://127.0.0.1:{api.server_port}", BIFROST_UPDATE_KEY=pub)


def update(*args):
    return subprocess.run([exe, "update", *args], env=env, capture_output=True, text=True, timeout=600)


def unchanged():
    return open(exe, "rb").read() == blob and sorted(os.listdir(upd)) == ["bifrost"]


try:
    r = update()
    check("update with no newer release says so", r.returncode == 0 and "newest release" in r.stdout, (r.stdout, r.stderr))
    r = update("9.9.8")
    check("a download that doesn't match SHA256SUMS is refused", r.returncode == 1 and "doesn't match" in r.stderr, r.stderr)
    check("…and changes nothing", unchanged(), os.listdir(upd))
    r = update("9.9.6")
    check("an unsigned release is refused", r.returncode == 1 and "isn't signed" in r.stderr, r.stderr)
    check("…and changes nothing", unchanged(), os.listdir(upd))
    r = update("9.9.5")
    check("a release signed with another key is refused", r.returncode == 1 and "not a genuine release" in r.stderr, r.stderr)
    check("…and changes nothing", unchanged(), os.listdir(upd))
    r = update("9.9.7")
    check("a file that isn't a single-file build is refused", r.returncode == 1 and "isn't a Bifrost single-file build" in r.stderr, r.stderr)
    check("…and changes nothing", unchanged(), os.listdir(upd))
    listing[:] = [{"tag_name": "v9.9.9", "name": "v9.9.9", "prerelease": False, "published_at": ""}]
    r = update()
    check("update fetches the newest release", r.returncode == 0 and "to 9.9.9" in r.stdout, (r.stdout, r.stderr))
    check("…swaps it in, executable", open(exe, "rb").read() == blob and os.access(exe, os.X_OK))
    check("…and keeps the previous binary", sorted(os.listdir(upd)) == ["bifrost", "bifrost.previous"], os.listdir(upd))
finally:
    api.shutdown()

# A bare system: no Python, no bubblewrap.
data = os.path.join(srv.data, "..", "bundle-bare")
subprocess.run(["rm", "-rf", data])
os.makedirs(data)
name = "bfbare"
subprocess.run(["podman", "rm", "-f", name], capture_output=True)
r = subprocess.run(["podman", "run", "-d", "--name", name, "-p", "18194:8080",
                    "-e", "BIFROST_INSECURE_COOKIES=1", "-e", "BIFROST_DATA=/data", "-e", "BIFROST_UPDATE_CHECK=off",
                    "-v", f"{BUNDLE}:/bifrost:ro", "-v", f"{os.path.abspath(data)}:/data:U",
                    "docker.io/library/debian:bookworm-slim", "/bifrost"], capture_output=True, text=True)
check("bare container starts", r.returncode == 0, r.stderr)
try:
    base = "http://127.0.0.1:18194"

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
