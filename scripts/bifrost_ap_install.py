#!/usr/bin/env python3
"""Install an Archipelago release for Bifrost (settings page → Archipelago).

    bifrost_ap_install.py releases <owner/repo>
        Print the repo's recent GitHub releases as one JSON line.
    bifrost_ap_install.py install <owner/repo> <tag> <dest>
        Download the release's source, unpack it into <dest>, create its venv
        and install its requirements. Progress lines go to stdout; the last
        line is a JSON result.
    bifrost_ap_install.py download <owner/repo> <tag> <asset> <dest>
        Download a release asset to <dest>, checked against the release's
        SHA256SUMS (`bifrost update`). The last line is a JSON result, with
        SHA256SUMS and SHA256SUMS.sig in hex for Bifrost to verify.

Runs under a plain Python 3 (not an AP venv), outside the sandbox: it needs
the network, and it's what builds the interpreter the sandboxed helpers use.
Only the standard library is used. GUI-only requirements (Kivy/KivyMD) are
skipped — Bifrost never opens AP's client UI — and a world whose
requirements fail to install is reported, not fatal. AP's Cython speedups
are built when a C compiler is around; otherwise AP uses its pure-Python
fallback.
"""
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import urllib.request

UA = {"User-Agent": "bifrost-ap-install", "Accept": "application/vnd.github+json"}
# GitHub allows 60 anonymous API requests an hour per address; a token
# (GITHUB_TOKEN, e.g. in CI) lifts that. Taken out of the environment so the
# build steps below (pip, setup scripts of world requirements) never see it.
TOKEN = os.environ.pop("GITHUB_TOKEN", None)
MAX_TARBALL = 512 * 1024 * 1024
MAX_ASSET = 2 * 1024 * 1024 * 1024
# The API's address; the tests point it at a stand-in.
API = os.environ.get("BIFROST_GITHUB_API", "https://api.github.com").rstrip("/")
SKIP = re.compile(r"^\s*(kivy|kivymd)\b", re.I)


def say(msg):
    print(msg, flush=True)


def request(url):
    req = urllib.request.Request(url, headers=UA)
    # Only to the API, and not carried along if it redirects elsewhere.
    if TOKEN and url.startswith("https://api.github.com/"):
        req.add_unredirected_header("Authorization", f"Bearer {TOKEN}")
    return urllib.request.urlopen(req, timeout=60)


def fetch(url, limit):
    with request(url) as r:
        data = r.read(limit + 1)
    if len(data) > limit:
        raise RuntimeError(f"{url}: larger than {limit} bytes")
    return data


def releases(repo):
    data = json.loads(fetch(f"{API}/repos/{repo}/releases?per_page=15", 4 << 20))
    out = [
        {
            "tag": r["tag_name"],
            "name": r.get("name") or r["tag_name"],
            "prerelease": bool(r.get("prerelease")),
            "published": r.get("published_at") or "",
        }
        for r in data
        if not r.get("draft")
    ]
    print(json.dumps({"ok": True, "releases": out}))


def download(repo, tag, asset, dest):
    rel = json.loads(fetch(f"{API}/repos/{repo}/releases/tags/{tag}", 4 << 20))
    urls = {a["name"]: a["browser_download_url"] for a in rel.get("assets", [])}
    for name in (asset, "SHA256SUMS"):
        if name not in urls:
            raise RuntimeError(f"release {tag} has no {name}")
    want = None
    sums = fetch(urls["SHA256SUMS"], 1 << 20)
    # Its signature, when the release has one; Bifrost checks it (the helper
    # only passes the bytes on).
    sig = fetch(urls["SHA256SUMS.sig"], 4096) if "SHA256SUMS.sig" in urls else None
    for line in sums.decode().splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[1].lstrip("*") == asset:
            want = parts[0].lower()
    if not want:
        raise RuntimeError(f"SHA256SUMS of {tag} doesn't list {asset}")
    say(f"Downloading {asset}…")
    h, size = hashlib.sha256(), 0
    # A new file of our own ("x" refuses an existing one or a symlink), so
    # what's hashed is exactly what's on disk.
    f = open(dest, "xb")
    try:
        with f, request(urls[asset]) as r:
            while chunk := r.read(1 << 20):
                size += len(chunk)
                if size > MAX_ASSET:
                    raise RuntimeError(f"{asset}: larger than {MAX_ASSET} bytes")
                h.update(chunk)
                f.write(chunk)
            f.flush()
            os.fsync(f.fileno())
        if h.hexdigest() != want:
            raise RuntimeError(f"{asset} doesn't match its SHA256SUMS entry")
    except BaseException:
        os.remove(dest)
        raise
    print(json.dumps({
        "ok": True, "size": size, "sha256": want,
        "sums": sums.hex(),
        "sig": sig.hex() if sig is not None else None,
    }))


def unpack(blob, dest):
    """Extract a GitHub source tarball into dest, dropping its top folder and
    refusing anything that would land outside dest or isn't a plain file/dir."""
    root = os.path.realpath(dest)
    with tarfile.open(fileobj=io.BytesIO(blob), mode="r:gz") as tf:
        for m in tf.getmembers():
            parts = m.name.split("/", 1)
            if len(parts) < 2 or not parts[1]:
                continue
            target = os.path.realpath(os.path.join(root, parts[1]))
            if not target.startswith(root + os.sep):
                raise RuntimeError(f"unsafe path in archive: {m.name}")
            if m.isdir():
                os.makedirs(target, exist_ok=True)
            elif m.isfile():
                os.makedirs(os.path.dirname(target), exist_ok=True)
                with tf.extractfile(m) as src, open(target, "wb") as dst:
                    shutil.copyfileobj(src, dst)
                if m.mode & 0o111:
                    os.chmod(target, 0o755)
            # links, devices etc. are skipped: AP's source has none it needs


def filtered(req_file, out_file):
    with open(req_file) as f:
        lines = [l for l in f if not SKIP.match(l)]
    with open(out_file, "w") as f:
        f.writelines(lines)


def pip(venv_py, *args):
    r = subprocess.run([venv_py, "-m", "pip", "--disable-pip-version-check", "--no-input", "--no-cache-dir", *args],
                       capture_output=True, text=True)
    return r.returncode == 0, (r.stderr or r.stdout).strip().splitlines()[-3:]


def install(repo, tag, dest):
    if os.path.exists(dest):
        shutil.rmtree(dest)
    os.makedirs(dest)
    say(f"Downloading {repo} {tag}…")
    blob = fetch(f"https://github.com/{repo}/archive/refs/tags/{tag}.tar.gz", MAX_TARBALL)
    say(f"Unpacking ({len(blob) // (1 << 20)} MiB)…")
    unpack(blob, dest)
    if not os.path.isfile(os.path.join(dest, "Utils.py")):
        raise RuntimeError("that release doesn't look like Archipelago (no Utils.py)")

    say("Creating the Python environment…")
    subprocess.run([sys.executable, "-m", "venv", os.path.join(dest, "venv")], check=True,
                   capture_output=True)
    py = os.path.join(dest, "venv", "bin", "python")
    ok, err = pip(py, "install", "--upgrade", "pip", "setuptools<81", "wheel")
    if not ok:
        raise RuntimeError("upgrading pip: " + " | ".join(err))

    say("Installing Archipelago's requirements…")
    core = os.path.join(dest, ".bifrost-requirements.txt")
    filtered(os.path.join(dest, "requirements.txt"), core)
    ok, err = pip(py, "install", "-r", core)
    if not ok:
        raise RuntimeError("core requirements: " + " | ".join(err))

    failed = []
    worlds = sorted(
        d for d in os.listdir(os.path.join(dest, "worlds"))
        if os.path.isfile(os.path.join(dest, "worlds", d, "requirements.txt"))
    )
    say(f"Installing requirements for {len(worlds)} worlds…")
    for w in worlds:
        req = os.path.join(dest, "worlds", w, ".bifrost-requirements.txt")
        filtered(os.path.join(dest, "worlds", w, "requirements.txt"), req)
        ok, err = pip(py, "install", "-r", req)
        if not ok:
            failed.append(w)
            say(f"  {w}: failed ({' | '.join(err)[-200:]})")

    speedups = False
    if shutil.which("cc") or shutil.which("gcc"):
        say("Building AP's speedups…")
        cy = os.path.join(dest, "venv", "bin", "cythonize")
        r = subprocess.run([cy, "-b", "-i", "_speedups.pyx"], cwd=dest, capture_output=True, text=True)
        speedups = r.returncode == 0
        shutil.rmtree(os.path.join(dest, "build"), ignore_errors=True)
        if not speedups:
            say("  speedups didn't build; AP will use its pure-Python fallback")
    print(json.dumps({"ok": True, "failed_worlds": failed, "speedups": speedups}))


def main():
    try:
        if sys.argv[1:2] == ["releases"] and len(sys.argv) == 3:
            releases(sys.argv[2])
        elif sys.argv[1:2] == ["install"] and len(sys.argv) == 5:
            install(*sys.argv[2:5])
        elif sys.argv[1:2] == ["download"] and len(sys.argv) == 6:
            download(*sys.argv[2:6])
        else:
            sys.exit(__doc__)
    except Exception as e:  # reported to Bifrost as the last line
        print(json.dumps({"ok": False, "error": f"{type(e).__name__}: {e}"}))
        sys.exit(1)


if __name__ == "__main__":
    main()
