"""Compare the release pins Dependabot can't watch with the latest releases.

- The bundle's Python: PY_URL / PY_SHA256 in packaging/bundle/Containerfile
  (a python-build-standalone release, same minor version).
- The Archipelago build: AP_VERSION (Dockerfile and bundle Containerfile) and
  AP_SHA256 (bundle Containerfile), a release of Bifrost's fork.
- Upstream Archipelago releases the fork hasn't built yet.

Writes a Markdown report to the file named by the first argument and prints
`stale=true|false` (for $GITHUB_OUTPUT). Uses GITHUB_TOKEN for the API if set.
"""
import hashlib
import json
import os
import re
import sys
import urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
CONTAINERFILE = os.path.join(ROOT, "packaging", "bundle", "Containerfile")
DOCKERFILE = os.path.join(ROOT, "Dockerfile")
PBS = "astral-sh/python-build-standalone"
FORK = "vidarrio/Archipelago"
UPSTREAM = "ArchipelagoMW/Archipelago"
FORK_TAG = re.compile(r"^(\d+)\.(\d+)\.(\d+)-bifrost\.(\d+)$")
UPSTREAM_TAG = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")


def get(url, raw=False):
    req = urllib.request.Request(url, headers={"User-Agent": "bifrost-check-pins", "Accept": "application/vnd.github+json"})
    token = os.environ.get("GITHUB_TOKEN")
    if token and url.startswith("https://api.github.com/"):
        req.add_unredirected_header("Authorization", f"Bearer {token}")
    with urllib.request.urlopen(req, timeout=120) as r:
        data = r.read()
    return data if raw else json.loads(data)


def args(path):
    out = {}
    for line in open(path, encoding="utf-8"):
        m = re.match(r"^ARG (\w+)=(\S+)", line)
        if m:
            out.setdefault(m.group(1), m.group(2))
    return out


def releases(repo):
    return [r for r in get(f"https://api.github.com/repos/{repo}/releases?per_page=100")
            if not r["draft"] and not r["prerelease"]]


def python_pin(bundle):
    url, sha = bundle["PY_URL"], bundle["PY_SHA256"]
    m = re.search(r"/download/(\d+)/cpython-(\d+)\.(\d+)\.(\d+)\+\d+-(.+)$", url)
    tag, major, minor, suffix = m.group(1), m.group(2), m.group(3), m.group(5)
    latest = get(f"https://api.github.com/repos/{PBS}/releases/latest")
    want = re.compile(rf"^cpython-{major}\.{minor}\.(\d+)\+{latest['tag_name']}-{re.escape(suffix)}$")
    asset = next((a for a in latest["assets"] if want.match(a["name"])), None)
    if asset is None or latest["tag_name"] <= tag:
        return None
    new_sha = (asset.get("digest") or "").removeprefix("sha256:") or hashlib.sha256(get(asset["browser_download_url"], raw=True)).hexdigest()
    return (f"### Bundle Python\n\n"
            f"python-build-standalone {latest['tag_name']} has Python {major}.{minor}.{want.match(asset['name']).group(1)} "
            f"(pinned: release {tag}). In `packaging/bundle/Containerfile`:\n\n"
            f"```\nARG PY_URL={asset['browser_download_url'].replace('%2B', '+')}\nARG PY_SHA256={new_sha}\n```\n")


def ap_pin(bundle, image):
    pinned = bundle["AP_VERSION"]
    fork = [(tuple(map(int, m.groups())), r["tag_name"]) for r in releases(FORK) if (m := FORK_TAG.match(r["tag_name"]))]
    up = [(tuple(map(int, m.groups())), r["tag_name"]) for r in releases(UPSTREAM) if (m := UPSTREAM_TAG.match(r["tag_name"]))]
    parts = []
    newest_fork = max(fork)[1] if fork else pinned
    if FORK_TAG.match(pinned) and newest_fork != pinned and max(fork)[0] > tuple(map(int, FORK_TAG.match(pinned).groups())):
        tarball = get(f"https://github.com/{FORK}/archive/refs/tags/{newest_fork}.tar.gz", raw=True)
        parts.append(f"### Archipelago build\n\n"
                     f"The fork has a newer build, `{newest_fork}` (pinned: `{pinned}`). Set it in both "
                     f"`Dockerfile` and `packaging/bundle/Containerfile`, with the source tarball's checksum "
                     f"in the latter:\n\n```\nARG AP_VERSION={newest_fork}\nARG AP_SHA256={hashlib.sha256(tarball).hexdigest()}\n```\n")
    if up and fork:
        newest_up, built = max(up), {v[:3] for v, _ in fork}
        if newest_up[0] not in built and newest_up[0] > max(built):
            parts.append(f"### Upstream Archipelago\n\n"
                         f"Upstream released `{newest_up[1]}`, which the fork has no build of yet. The maintainer "
                         f"cuts one with `packaging/archipelago/release.sh {newest_up[1]}` (see "
                         f"`packaging/archipelago/README.md`); this check then reports the new build.\n")
    if image.get("AP_VERSION") != pinned:
        parts.append(f"### Mismatch\n\n`Dockerfile` pins `{image.get('AP_VERSION')}` but the bundle pins `{pinned}`.\n")
    return "\n".join(parts) or None


def main():
    bundle, image = args(CONTAINERFILE), args(DOCKERFILE)
    sections = [s for s in (python_pin(bundle), ap_pin(bundle, image)) if s]
    report = ("Some release pins that Dependabot doesn't watch are behind (checked by "
              "`.github/workflows/pins.yml`). Bump them in a `chore:` pull request; CI builds "
              "the image and bundle with the new values.\n\n" + "\n".join(sections)) if sections else ""
    with open(sys.argv[1], "w", encoding="utf-8") as f:
        f.write(report)
    print(f"stale={'true' if sections else 'false'}")


if __name__ == "__main__":
    main()
