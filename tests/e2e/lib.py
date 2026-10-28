"""Shared harness for Bifrost end-to-end runs: start a native Bifrost on a
throwaway data dir, do first-run setup, mint users, and check things.

Paths (environment):
  BIFROST_E2E_WORK    scratch space: run dirs, Archipelago installs
                      (default ~/.cache/bifrost-dev)
  BIFROST_E2E_AP_DIR  the Archipelago install Bifrost runs on (default
                      $BIFROST_E2E_WORK/archipelago-bifrost; setup-ap.sh makes it)
  BIFROST_E2E_BIN     the Bifrost binary (default target/debug/bifrost)
"""
import html
import os
import re
import shutil
import signal
import subprocess
import sys
import time

import requests

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
DEV = os.environ.get("BIFROST_E2E_WORK") or os.path.expanduser("~/.cache/bifrost-dev")
FIX = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures")
BIN = os.environ.get("BIFROST_E2E_BIN") or os.path.join(REPO, "target/debug/bifrost")
AP_DIR = os.environ.get("BIFROST_E2E_AP_DIR") or os.path.join(DEV, "archipelago-bifrost")

results = []


def check(name, cond, detail=""):
    results.append((name, bool(cond)))
    print(("PASS " if cond else "FAIL ") + name + ("" if cond else f"  [{detail}]"), flush=True)
    return cond


def summary():
    bad = [n for n, ok in results if not ok]
    print(f"\n{len(results) - len(bad)}/{len(results)} passed")
    if bad:
        print("failed:", *bad, sep="\n  ")
    return 1 if bad else 0


class Server:
    def __init__(self, tag, port=18181, ap_ports="39100-39102", extra_env=None, fresh=True, bin=BIN, bundle=False):
        self.data = os.path.join(DEV, "run", tag)
        if fresh:
            shutil.rmtree(self.data, ignore_errors=True)
        os.makedirs(self.data, exist_ok=True)
        self.port = port
        self.base = f"http://127.0.0.1:{port}"
        self.env = dict(
            os.environ,
            BIFROST_DATA=self.data,
            BIFROST_AP_DIR=AP_DIR,
            BIFROST_RUNTIME_DIR=REPO,
            BIFROST_PORT=str(port),
            BIFROST_AP_PORTS=ap_ports,
            BIFROST_AP_BIND="127.0.0.1",
            RUST_LOG="info",
            BIFROST_INSECURE_COOKIES="1",
            BIFROST_UPDATE_CHECK="off",
        )
        self.env.pop("BIFROST_AP_PYTHON", None)
        if bundle:  # a bundle brings its own AP, Python and scripts
            for k in ("BIFROST_AP_DIR", "BIFROST_RUNTIME_DIR"):
                self.env.pop(k)
        self.env.update(extra_env or {})
        self.bin = bin
        self.log_path = os.path.join(self.data, "bifrost.log")
        self.proc = None

    def start(self):
        self.log = open(self.log_path, "a")
        self.proc = subprocess.Popen([self.bin], env=self.env, stdout=self.log, stderr=subprocess.STDOUT)
        for _ in range(100):
            # Exited (e.g. its port is taken): don't let another process
            # answering on that port stand in for it.
            if self.proc.poll() is not None:
                raise RuntimeError(f"bifrost exited with {self.proc.returncode}; see {self.log_path}")
            try:
                if requests.get(self.base + "/health", timeout=1).ok:
                    return self
            except requests.RequestException:
                pass
            time.sleep(0.2)
        raise RuntimeError("bifrost didn't come up; see " + self.log_path)

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(40)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.log.close()

    def logtext(self):
        with open(self.log_path) as f:
            return re.sub(r"\x1b\[[0-9;]*m", "", f.read())

    def cli(self, *args):
        return subprocess.run([self.bin, *args], env=self.env, capture_output=True, text=True)

    def session(self):
        s = requests.Session()
        s.headers["Origin"] = self.base
        s.base = self.base
        return s

    def setup_sysadmin(self, user="root", pw="correct horse battery"):
        tok = re.search(r"/setup\?token=([0-9a-f]+)", self.logtext()).group(1)
        s = self.session()
        r = s.post(self.base + "/setup", data=dict(token=tok, username=user, password=pw, confirm=pw))
        assert r.ok and "bifrost_session" in s.cookies.get_dict(), r.text[:300]
        return s

    def invite_user(self, admin, user, pw="correct horse battery"):
        r = admin.post(self.base + "/admin/links/invite", data=dict(label=user, days=1))
        path = re.search(r'data-path="(/invite/[^"]+)"', r.text).group(1)
        s = self.session()
        r = s.post(self.base + path, data=dict(username=user, password=pw, confirm=pw))
        assert r.ok, r.text[:300]
        return s


def _decoded(r):
    """HTML pages come back with entities decoded (Bifrost escapes quotes and
    apostrophes), so checks can match the text a person reads."""
    if "text/html" in r.headers.get("content-type", ""):
        r._content = html.unescape(r.text).encode(r.encoding or "utf-8")
    return r


def get(s, path, **kw):
    return _decoded(s.get(s.base + path, **kw))


def post(s, path, **kw):
    return _decoded(s.post(s.base + path, **kw))


def descendants(pid):
    """Every process below `pid`."""
    out, todo = [], [pid]
    while todo:
        p = todo.pop()
        r = subprocess.run(["pgrep", "-P", str(p)], capture_output=True, text=True)
        kids = [int(x) for x in r.stdout.split()]
        out += kids
        todo += kids
    return out


def ap_servers(srv):
    """(pid, argv) of the AP servers this Bifrost runs (not other instances'),
    without the bubblewrap wrappers."""
    found = []
    for p in descendants(srv.proc.pid):
        try:
            argv = open(f"/proc/{p}/cmdline", "rb").read().split(b"\0")
        except OSError:
            continue
        args = [a.decode(errors="replace") for a in argv if a]
        if args and any("bifrost_server.py" in a for a in args) and "bwrap" not in os.path.basename(args[0]):
            found.append((p, args))
    return found


def bundled_ap_version():
    """The version of the Archipelago install the suites run on."""
    with open(os.path.join(AP_DIR, "BIFROST_VERSION")) as f:
        return f.read().strip()


def wait_for(fn, timeout=120, every=1.0):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(every)
    return None


def fixture(name):
    with open(os.path.join(FIX, name), "rb") as f:
        return f.read()


if __name__ == "__main__":
    sys.exit("library only")
