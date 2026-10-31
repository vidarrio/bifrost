"""The new settings-page features inside the container image: install an
Archipelago release (needs python3-venv + network in the image), the test
generation runs under bubblewrap there, and a backup is written."""
import os
import re
import subprocess
import sys
import time

import requests

from lib import check, summary, wait_for, tiny_pack

IMAGE = os.environ.get("IMAGE", "bifrost:dev")
TAG = os.environ.get("AP_TAG", "0.6.7-bifrost.2")
PORT = 18193
BASE = f"http://127.0.0.1:{PORT}"


def sh(*a, **kw):
    return subprocess.run(a, capture_output=True, text=True, **kw)


sh("podman", "rm", "-f", "bfc")
sh("podman", "volume", "rm", "bfc-data")
# GITHUB_TOKEN (when set, as in CI) lifts GitHub's anonymous rate limit for
# the release check and install.
token = ["-e", "GITHUB_TOKEN"] if os.environ.get("GITHUB_TOKEN") else []
r = sh("podman", "run", "-d", "--name", "bfc", "--stop-timeout", "30", "--security-opt", "unmask=/proc/*",
       "-e", "BIFROST_INSECURE_COOKIES=1", "-e", "BIFROST_UPDATE_CHECK=off", *token, "-p", f"{PORT}:8080", "-v", "bfc-data:/var/lib/bifrost", IMAGE)
assert r.returncode == 0, r.stderr
try:
    def up():
        try:
            return requests.get(BASE + "/health", timeout=1).ok
        except requests.RequestException:
            return False
    assert wait_for(up, 60), "the container didn't come up"
    logs = lambda: re.sub(r"\x1b\[[0-9;]*m", "", sh("podman", "logs", "bfc").stdout + sh("podman", "logs", "bfc").stderr)
    check("strong isolation in the container", "isolation: strong" in logs())
    tok = re.search(r"/setup\?token=([0-9a-f]+)", logs()).group(1)
    s = requests.Session()
    s.headers["Origin"] = BASE
    pw = "correct horse battery"
    s.post(BASE + "/setup", data=dict(token=tok, username="root", password=pw, confirm=pw))

    s.post(BASE + "/admin/settings/ap/check")
    page = s.get(BASE + "/admin/settings").text
    check("release check works from the container", f'<b class="mono">{TAG}</b>' in page)
    s.post(BASE + "/admin/settings/ap/install", data={"version": TAG})
    done = wait_for(lambda: (p := s.get(BASE + "/admin/settings").text) and "this page refreshes" not in p and p, 1800, 10)
    ok = bool(done) and "passed a test generation" in done
    check("install + sandboxed test generation in the container", ok, re.findall(r'class="banner[^"]*">[^<]*', done or ""))
    if not ok:
        print(re.findall(r"<pre[^>]*>(.*?)</pre>", done or "", re.S))

    r = sh("podman", "exec", "bfc", "sh", "-c", f"ls /var/lib/bifrost/ap/{TAG}/ | grep -c '^_speedups.*\\.so$'")
    check("installed release built its speedups (compiler in the image)", r.stdout.strip() == "1", r.stdout + r.stderr)

    s.post(BASE + "/admin/seasons", data=dict(name="Packs"))
    s.post(BASE + "/s/1/admin/signups", data=dict(open="1"))
    s.post(BASE + "/s/1/signup", files={"yaml": ("y.yaml", b"name: Ann\ngame: ChecksFinder\nChecksFinder: {}\n")})
    s.post(BASE + "/s/1/admin/generate")
    wait_for(lambda: "Generated seed" in s.get(BASE + "/s/1/admin").text, 300, 3)
    wait_for(lambda: "Ann" in s.get(BASE + "/s/1/board").text, 60)
    r = s.post(BASE + "/s/1/game/1/poptracker", files={"pack": ("p.zip", tiny_pack())})
    check("a PopTracker pack passes its worker in the container", "Pack rejected" not in r.text, r.text[:300])
    d = s.get(BASE + "/s/1/game/1/poptracker/state").json()
    check("…and tracks", d.get("state", {}).get("items", [{}])[0].get("active"), d)

    s.post(BASE + "/admin/settings/backups/now")
    page = wait_for(lambda: (p := s.get(BASE + "/admin/settings").text) and "Backing up" not in p and p, 120)
    check("backup in the container", page and "Backed up to bifrost-" in page)
    r = sh("podman", "exec", "bfc", "sh", "-c", "ls -l /var/lib/bifrost/backups /var/lib/bifrost/ap")
    print(r.stdout)
finally:
    t = time.time()
    sh("podman", "stop", "bfc")
    print(f"stopped in {time.time() - t:.1f}s")
    sh("podman", "rm", "-f", "bfc")
    sh("podman", "volume", "rm", "bfc-data")

sys.exit(summary())
