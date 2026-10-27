"""Archipelago installs from the settings page: check releases, install one
(test generation included), switch new seasons to it, live seasons stay on
the version that generated them (also across a restart), remove rules."""
import os
import re
import subprocess
import sys

from lib import Server, ap_servers, bundled_ap_version, check, get, post, summary, wait_for

TAG = os.environ.get("AP_TAG", "0.6.7-bifrost.2")  # a release of the fork
BUNDLED = bundled_ap_version()


def yaml(name):
    return f"name: {name}\ngame: ChecksFinder\nChecksFinder: {{}}\n".encode()


def server_pythons():
    """Python executables of this instance's running AP servers."""
    return [args[0] for _, args in ap_servers(srv)]


srv = Server("apupdate", port=18182, ap_ports="39110-39112")
srv.start()
try:
    root = srv.setup_sysadmin()
    page = get(root, "/admin/settings").text
    check("bundled version listed and active", f"{BUNDLED} · bundled" in page and "active" in page)

    post(root, "/admin/settings/ap/check")
    page = get(root, "/admin/settings").text
    check("release check lists the tag", f'<b class="mono">{TAG}</b>' in page, page[-1500:])
    check("bundled version isn't offered", f'<b class="mono">{BUNDLED}</b>' not in page)

    post(root, "/admin/settings/ap/install", data={"version": TAG})
    page = get(root, "/admin/settings").text
    check("install runs in the background", "this page refreshes" in page)
    r = post(root, "/admin/settings/ap/install", data={"version": "0.6.6"})
    check("a second install is refused while one runs", "already running" in r.text)
    done = wait_for(lambda: (p := get(root, "/admin/settings").text) and "this page refreshes" not in p and p, 1800, 10)
    ok = done and "passed a test generation" in done
    check("install finished and passed its test generation", ok, re.findall(r'class="banner[^"]*">[^<]*', done or "")[-2:])
    if not ok:
        print(re.findall(r"<pre[^>]*>(.*?)</pre>", done or "", re.S))
        raise SystemExit(summary())
    check("installed version listed", f"{TAG}</td>" in done and os.path.isfile(os.path.join(srv.data, "ap", TAG, ".bifrost-ready")))

    def live_season(name, player):
        post(root, "/admin/seasons", data=dict(name=name))
        sid = max(int(x) for x in re.findall(r"/s/(\d+)/admin", get(root, "/admin/seasons").text))
        post(root, f"/s/{sid}/admin/signups", data=dict(open="1"))
        r = post(root, f"/s/{sid}/signup", files={"yaml": ("p.yaml", yaml(player))})
        assert "Signed up" in r.text, r.text[-300:]
        post(root, f"/s/{sid}/admin/generate")
        t = wait_for(lambda: (x := get(root, f"/s/{sid}/admin").text) and ("Generated seed" in x or "Failed" in x) and x, 300, 3)
        m = re.search(r"(Generated seed[^<]*|Failed[^<]*)", t or "")
        check(f"{name} generated", m and m.group(0).startswith("Generated"), m and m.group(0))
        return sid

    a = live_season("Alpha", "Ann")
    check("Alpha runs on the bundled install", wait_for(lambda: any(p.startswith(srv.env["BIFROST_AP_DIR"]) for p in server_pythons()), 30), server_pythons())

    post(root, "/admin/settings/ap/use", data={"version": TAG})
    page = get(root, "/admin/settings").text
    row = lambda v: re.search(rf"<tr><td class=\"mono\">{re.escape(v)}.*?</tr>", page, re.S).group(0)
    check("new version active", "active" in row(TAG))
    check("Alpha still pinned to the bundled version", "Alpha (live)" in row(BUNDLED))
    check("home page shows the active version", f"Archipelago {TAG.split('-bifrost.')[0]}" in get(srv.session(), "/").text)

    b = live_season("Beta", "Bea")
    tag_py = os.path.join(srv.data, "ap", TAG, "venv/bin/python")
    check("Beta's server runs the new install", wait_for(lambda: tag_py in server_pythons(), 30), server_pythons())
    page = get(root, "/admin/settings").text
    check("Beta pinned to the new version", "Beta (live)" in row(TAG))

    r = post(root, "/admin/settings/ap/remove", data={"version": TAG})
    check("can't remove the active version", "active version" in r.text)
    post(root, "/admin/settings/ap/use", data={"version": BUNDLED})
    r = post(root, "/admin/settings/ap/remove", data={"version": TAG})
    check("can't remove a version a season runs on", "still runs on that version" in r.text)
    r = post(root, "/admin/settings/ap/remove", data={"version": BUNDLED})
    check("can't remove the bundled version", "can't be removed" in r.text)

    # restart: pins survive
    srv.stop()
    srv.start()
    root = srv.session()
    post(root, "/login", data=dict(username="root", password="correct horse battery"))
    pys = wait_for(lambda: len(server_pythons()) >= 2 and server_pythons(), 60)
    check("after restart Beta still runs the new install, Alpha the bundled",
          pys and tag_py in pys and any(p.startswith(srv.env["BIFROST_AP_DIR"]) for p in pys), pys)

    # Right after a restart the board hasn't seen the room yet: archiving
    # (final) is refused until it has, rather than freeze every game at 0.
    archive = lambda: post(root, f"/s/{b}/admin/archive-season", data={"name": ""}).text
    first = archive()
    if "isn't connected" in first:
        check("archive waits for the live board", True)
        check("…then archives", wait_for(lambda: "isn't connected" not in archive(), 90, 3))
    page = get(root, "/admin/settings").text
    check("archived season no longer pins", "Beta" not in row(TAG))
    post(root, "/admin/settings/ap/remove", data={"version": TAG})
    check("unused version removed", not os.path.exists(os.path.join(srv.data, "ap", TAG)) and f"{TAG}</td>" not in get(root, "/admin/settings").text)
finally:
    srv.stop()

sys.exit(summary())
