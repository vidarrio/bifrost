"""Live load tracking: the settings page's load panel shows a live season's
AP server, helpers and Bifrost, with charts, refreshed via its own endpoint
(sysadmin only); the season admin page shows its server's usage."""
import os
import re
import sys
import time

from lib import Server, check, get, post, summary, wait_for

YAML = b"name: Ann\ngame: ChecksFinder\nChecksFinder: {}\n"
SHOT = os.environ.get("SHOT")

srv = Server("load", port=18187, ap_ports="39150-39152")
srv.start()
try:
    root = srv.setup_sysadmin()
    first = get(root, "/admin/settings/load").text
    check("panel answers before/at the first sample", "Memory used" in first or "first sample" in first)
    post(root, "/admin/seasons", data=dict(name="Loaded"))
    sid = int(re.search(r"/s/(\d+)/admin", get(root, "/admin/seasons").text).group(1))
    post(root, f"/s/{sid}/admin/signups", data=dict(open="1"))
    post(root, f"/s/{sid}/signup", files={"yaml": ("y.yaml", YAML)})
    post(root, f"/s/{sid}/admin/generate")
    wait_for(lambda: "Generated seed" in get(root, f"/s/{sid}/admin").text, 300, 3)
    panel = wait_for(lambda: (p := get(root, "/admin/settings/load").text) and "Loaded — AP server" in p
                     and re.search(r"Loaded — AP server</td><td>([1-9]\d*) MiB", p) and p, 60, 5)
    check("the live season's server is listed with its memory", panel, (panel or get(root, "/admin/settings/load").text)[:600])
    if panel:
        mib = int(re.search(r"Loaded — AP server</td><td>(\d+) MiB", panel).group(1))
        check("…a plausible amount (20–2000 MiB)", 20 <= mib <= 2000, mib)
        check("host memory and CPU charts", panel.count('class="loadchart"') == 2 and "<polyline" in panel)
        check("helpers and Bifrost rows", "Other processes" in panel and "<td>Bifrost</td>" in panel)
    page = get(root, "/admin/settings").text
    check("settings page embeds the panel and refreshes it", 'id="loadpanel"' in page and "/admin/settings/load" in page)
    check("season admin page shows its server's usage",
          wait_for(lambda: "AP server:" in get(root, f"/s/{sid}/admin").text and "MiB memory" in get(root, f"/s/{sid}/admin").text, 30, 3))
    user = srv.invite_user(root, "pleb")
    check("players can't see the load", get(user, "/admin/settings/load").status_code in (401, 403))
    if SHOT:
        # Two more samples so the lines have a shape; the page's CSS is inline,
        # so a saved copy renders the same.
        time.sleep(25)
        import glob, subprocess
        html = SHOT.replace(".png", ".html")
        with open(html, "w") as f:
            f.write(get(root, "/admin/settings").text)
        shell = glob.glob(os.path.expanduser("~/.cache/ms-playwright/chromium_headless_shell-*/*/chrome-headless-shell"))[0]
        for name, width in ((SHOT, 1100), (SHOT.replace(".png", "-narrow.png"), 390)):
            r = subprocess.run([shell, "--no-sandbox", "--hide-scrollbars", f"--window-size={width},1600",
                                f"--screenshot={name}", "file://" + html], capture_output=True, text=True, timeout=60)
            print("screenshot", name, r.returncode)
finally:
    srv.stop()

sys.exit(summary())
