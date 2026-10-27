"""Backups: back up now from the settings page, contents and exclusions,
download/delete, retention over a history of nightly backups, the size cap,
and restore (refused while running; brings the data back; old data kept)."""
import io
import os
import re
import sys
import time
import zipfile

from lib import Server, check, get, post, summary, wait_for

DAY = 86400


def fake(dirpath, at, size=1000):
    name = time.strftime("bifrost-%Y%m%d-%H%M%S.zip", time.gmtime(at))
    with open(os.path.join(dirpath, name), "wb") as f:
        f.write(b"\0" * size)
    return name


srv = Server("backup", port=18183, ap_ports="39120-39122")
srv.start()
try:
    root = srv.setup_sysadmin()
    alice = srv.invite_user(root, "alice")
    post(root, "/admin/seasons", data=dict(name="Kept"))
    os.makedirs(os.path.join(srv.data, "ap", "0.0.1"), exist_ok=True)
    open(os.path.join(srv.data, "ap", "0.0.1", "big.bin"), "wb").write(b"x" * 1000)
    os.makedirs(os.path.join(srv.data, "tmp"), exist_ok=True)
    open(os.path.join(srv.data, "tmp", "scratch"), "w").write("x")
    os.makedirs(os.path.join(srv.data, "seasons", "1", "output"), exist_ok=True)
    open(os.path.join(srv.data, "seasons", "1", "output", "current.apsave"), "wb").write(b"save")

    page = get(root, "/admin/settings").text
    check("settings shows the policy", "Nightly at 04:00 UTC" in page and "7 days, 4 weeks and 3 months" in page and "No backups yet" in page)
    check("a user can't see backups", get(alice, "/admin/settings/backups/download?name=x").status_code != 200)

    post(root, "/admin/settings/backups/now")
    page = wait_for(lambda: (p := get(root, "/admin/settings").text) and "Backing up" not in p and p, 60)
    m = re.search(r"Backed up to (bifrost-\d{8}-\d{6}\.zip)", page or "")
    check("back up now", m, re.findall(r'banner[^>]*>[^<]*', page or ""))
    name = m.group(1)
    bdir = os.path.join(srv.data, "backups")
    check("backup is owner-only", oct(os.stat(os.path.join(bdir, name)).st_mode & 0o777) == "0o600")
    check("backups dir is owner-only", oct(os.stat(bdir).st_mode & 0o777) == "0o700")

    r = get(root, f"/admin/settings/backups/download?name={name}")
    check("download works", r.ok and r.headers.get("content-disposition", "").endswith(f'"{name}"'), r.status_code)
    z = zipfile.ZipFile(io.BytesIO(r.content))
    names = z.namelist()
    check("backup has the database", "bifrost.db" in names)
    check("backup has season files", z.read("seasons/1/output/current.apsave") == b"save", names[:10])
    check("backup leaves out ap/, tmp/, backups/ and raw db files",
          not any(n.startswith(("ap/", "tmp/", "backups/")) or n.startswith("bifrost.db-") for n in names), names)
    db = z.read("bifrost.db")
    check("database snapshot is a SQLite file", db[:16] == b"SQLite format 3\x00")
    check("traversal names are refused", get(root, "/admin/settings/backups/download?name=../bifrost.db").status_code == 404)

    # retention over 120 nights of history
    now = int(time.time())
    hist = [fake(bdir, now - d * DAY - 3600) for d in range(1, 121)]
    post(root, "/admin/settings/backups/now")
    wait_for(lambda: "Backing up" not in get(root, "/admin/settings").text, 60)
    left = sorted(f for f in os.listdir(bdir) if f.startswith("bifrost-"))
    check("retention prunes to daily+weekly+monthly", 9 <= len(left) <= 14, len(left))
    ages = [(now - time.mktime(time.strptime(f[8:23], "%Y%m%d-%H%M%S")) + time.timezone) / DAY for f in left]
    check("nothing older than ~3 months kept", max(ages) < 100, max(ages))
    check("the last week is all there", sum(1 for a in ages if a < 7) >= 7, sorted(ages)[:10])

    newest = max(left)
    post(root, "/admin/settings/backups/delete", data={"name": min(left)})
    check("delete works", min(left) not in os.listdir(bdir))

    # restore: change something, refuse while running, then restore offline
    post(root, "/admin/seasons", data=dict(name="AfterBackup"))
    check("second season exists", "AfterBackup" in get(root, "/admin/seasons").text)
    r = srv.cli("backup", "restore", os.path.join(bdir, newest))
    check("restore refused while Bifrost runs", r.returncode != 0 and "stop Bifrost" in r.stderr, r.stderr)
finally:
    srv.stop()

r = srv.cli("backup", "restore", os.path.join(bdir, newest))
check("restore succeeds offline", r.returncode == 0 and "previous contents are in" in r.stdout, (r.stdout, r.stderr))
aside = [d for d in os.listdir(srv.data) if d.startswith(".pre-restore-")]
check("previous data kept aside", aside and os.path.exists(os.path.join(srv.data, aside[0], "bifrost.db")))
check("installed AP releases and backups stay put", os.path.exists(os.path.join(srv.data, "ap", "0.0.1", "big.bin")) and os.path.isdir(bdir))
srv.start()
try:
    root = srv.session()
    post(root, "/login", data=dict(username="root", password="correct horse battery"))
    seasons = get(root, "/admin/seasons").text
    check("restored state is the backup's", "Kept" in seasons and "AfterBackup" not in seasons)
finally:
    srv.stop()

# size cap: a tight cap keeps only the newest
cap = Server("backup", port=18183, ap_ports="39120-39122", fresh=False, extra_env={"BIFROST_BACKUP_MAX_MB": "1"})
for d in range(1, 6):
    fake(bdir, int(time.time()) - d * DAY, 400_000)
r = cap.cli("backup", "now")
left = sorted(f for f in os.listdir(bdir) if f.startswith("bifrost-"))
total = sum(os.path.getsize(os.path.join(bdir, f)) for f in left)
check("size cap enforced by the CLI backup", r.returncode == 0 and total <= 1 << 20 and r.stdout.strip().endswith(left[-1]), (r.stdout, r.stderr, left, total))
r = cap.cli("backup", "list")
check("backup list", r.returncode == 0 and left[-1] in r.stdout)

sys.exit(summary())
