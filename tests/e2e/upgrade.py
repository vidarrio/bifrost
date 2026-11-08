"""Upgrades: a start under a different Bifrost version backs up the data dir
before touching the database (and the backup holds the old database), a
start under the same version doesn't, and a database a newer Bifrost has
migrated is refused rather than run on. CLI commands never back up or
migrate, and refuse a database on another schema. A failed pre-upgrade
backup stops the start only when the schema would change."""
import contextlib
import glob
import os
import shutil
import stat
import sqlite3
import sys
import tarfile
import zipfile

from lib import FIX, Server, check, get, post, summary, wait_for

PORT, AP_PORTS = 18193, "39175-39177"
ENV = {"BIFROST_BACKUP_HOUR": "off"}


def stamp(srv, version=None, schema=None):
    with srv.db() as db:
        if version is not None:
            db.execute("UPDATE setting SET value = ? WHERE key = 'bifrost_version'", (version,))
        if schema is not None:
            db.execute(f"PRAGMA user_version = {schema}")
        row = db.execute("SELECT value FROM setting WHERE key = 'bifrost_version'").fetchone()
        return db.execute("PRAGMA user_version").fetchone()[0], row and row[0]


def backups(srv):
    d = os.path.join(srv.data, "backups")
    return sorted(n for n in os.listdir(d) if n.endswith(".zip")) if os.path.isdir(d) else []


def server(**env):
    return Server("upgrade", port=PORT, ap_ports=AP_PORTS, fresh=False, extra_env={**ENV, **env})


def refused(srv):
    """Start srv expecting it to exit; its exit code (or None if it ran)."""
    try:
        srv.start()
    except RuntimeError:
        srv.log.close()
        return srv.proc.returncode
    srv.stop()
    return None


srv = Server("upgrade", port=PORT, ap_ports=AP_PORTS, extra_env=ENV)
srv.start()
try:
    srv.setup_sysadmin()
finally:
    srv.stop()
schema, version = stamp(srv)
check("a fresh data dir is stamped with a schema version and this Bifrost's", schema >= 1 and version, (schema, version))


def schema_of(srv):
    """A database's schema: what an upgraded one has to match a fresh one in."""
    with srv.db() as db:
        return db.execute("SELECT type, name, tbl_name, sql FROM sqlite_master "
                          "WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name").fetchall()


fresh_schema = schema_of(srv)
check("a first start takes no backup", backups(srv) == [], backups(srv))

srv = server()
srv.start()
srv.stop()
check("a restart on the same version takes no backup", backups(srv) == [], backups(srv))
mode = lambda *p: stat.S_IMODE(os.stat(os.path.join(srv.data, *p)).st_mode)
loose = os.path.join(srv.data, "loose.txt")
link = os.path.join(srv.data, "linked")
outside = os.path.join(srv.data, "..", "upgrade-outside.txt")
open(loose, "w").close()
open(outside, "w").close()
os.chmod(loose, 0o644)
os.chmod(outside, 0o644)
os.symlink(outside, link)
srv = server()
srv.start()
srv.stop()
check("a loose file is tightened", mode("loose.txt") == 0o600, oct(mode("loose.txt")))
check("a link isn't followed", stat.S_IMODE(os.stat(outside).st_mode) == 0o644, oct(os.stat(outside).st_mode))
for f in (loose, link, outside):
    os.remove(f)
check("the data dir is kept to Bifrost's user",
      (mode(), mode("bifrost.db"), mode("seasons")) == (0o711, 0o600, 0o700), (oct(mode()), oct(mode("bifrost.db")), oct(mode("seasons"))))
snap = os.path.join(srv.data, "snap.db")
shutil.copy(os.path.join(srv.data, "bifrost.db"), snap)
with contextlib.closing(sqlite3.connect(snap)) as db, db:
    tables = db.execute("SELECT count(*) FROM sqlite_master").fetchone()[0]
os.remove(snap)
for f in glob.glob(snap + "-*"):
    os.remove(f)
check("a clean stop folds the write-ahead log into the database", tables > 15, tables)

# Pretend an older Bifrost ran here last.
stamp(srv, version="0.0.0-old")
r = srv.cli("auth", "status")
check("a command on the same schema runs", r.returncode == 0 and "root" in r.stdout, (r.returncode, r.stderr))
check("but neither backs up nor restamps", backups(srv) == [] and stamp(srv)[1] == "0.0.0-old", (backups(srv), stamp(srv)))

srv = server()
srv.start()
try:
    log = srv.logtext()
    taken = backups(srv)
    check("an upgrade backs up first, as a pre-upgrade backup", len(taken) == 1 and taken[0].endswith("-pre-upgrade-from-0.0.0-old.zip"), taken)
    check("and says how to go back", "to go back to 0.0.0-old" in log and "bifrost backup restore" in log, log[-2000:])
    if taken:
        with zipfile.ZipFile(os.path.join(srv.data, "backups", taken[0])) as z:
            snap = z.read("bifrost.db")
        path = os.path.join(srv.data, "snap.db")
        open(path, "wb").write(snap)
        with contextlib.closing(sqlite3.connect(path)) as db, db:
            old = db.execute("SELECT value FROM setting WHERE key = 'bifrost_version'").fetchone()[0]
        os.remove(path)
        check("the backup holds the database as the old version left it", old == "0.0.0-old", old)
    s = srv.session()
    r = s.post(srv.base + "/login", data=dict(username="root", password="correct horse battery"))
    check("accounts survive the upgrade", "bifrost_session" in s.cookies.get_dict(), r.status_code)
finally:
    srv.stop()
check("the upgraded data dir is stamped with this version", stamp(srv)[1] == version, stamp(srv))

# The pre-upgrade backup can't be written.
open(os.path.join(srv.data, "blocker"), "w").close()
blocked = os.path.join(srv.data, "blocker", "backups")
stamp(srv, version="0.0.0-old")
srv = server(BIFROST_BACKUP_DIR=blocked)
check("a failed backup doesn't stop a start that keeps the schema", refused(srv) is None)
check("it says so", "pre-upgrade backup failed" in srv.logtext() and "starting anyway" in srv.logtext())
# Migrating needs a schema step, and there are none since the 0.3.0 baseline: when
# the first arrives, test here that a failed backup stops a start that would
# migrate, and that a migration backs up first and keeps existing rows.
os.remove(os.path.join(srv.data, "blocker"))
stamp(srv, version="99.0.0", schema=schema + 1)
srv = server()
code = refused(srv)
log = srv.logtext()
check("a database from a newer Bifrost is refused", code == 1, code)
check("with a message naming the version and the way back",
      "last used by Bifrost 99.0.0" in log and "pre-upgrade-from-" in log, log[-2000:])
r = srv.cli("auth", "status")
check("commands that use the database are refused too", r.returncode == 1 and "99.0.0" in r.stderr, (r.returncode, r.stderr))
r = srv.cli("backup", "list")
check("but the backups can still be listed", r.returncode == 0 and "pre-upgrade-from-0.0.0-old.zip" in r.stdout, (r.returncode, r.stdout, r.stderr))
check("and the database is left alone", stamp(srv) == (schema + 1, "99.0.0"), stamp(srv))

# Numeric season ids, from before 0.3.0, have no upgrade: not from a release (0.1
# has no schema version, 0.2 has 2), or from a build between releases.
def remove_db():
    for f in glob.glob(os.path.join(srv.data, "bifrost.db*")):
        os.remove(f)


for old in (0, 2, 6):
    remove_db()
    with srv.db() as db:
        db.execute("CREATE TABLE season (id INTEGER PRIMARY KEY, name TEXT)")
        db.execute(f"PRAGMA user_version = {old}")
    srv = server()
    check(f"a database from before 0.3.0 is refused (schema {old})", refused(srv) not in (None, 0))
    check("saying so", "before 0.3.0" in srv.logtext(), srv.logtext()[-800:])
    r = srv.cli("auth", "status")
    check("commands refuse it too", r.returncode == 1 and "before 0.3.0" in r.stderr, (r.returncode, r.stderr))
remove_db()

# The baseline 0.3.0 freezes, a data directory as Bifrost wrote it
# (fixtures/make_baseline.py): the current build starts on it, runs every step
# since, and finds everything.
remove_db()
srv = Server("upgrade", port=PORT, ap_ports=AP_PORTS, fresh=True, extra_env=ENV)
shutil.rmtree(srv.data + ".unpack", ignore_errors=True)
with tarfile.open(os.path.join(FIX, "baseline.tar.gz")) as tar:
    tar.extractall(srv.data + ".unpack", filter="data")
shutil.rmtree(srv.data)
os.rename(os.path.join(srv.data + ".unpack", "data"), srv.data)
os.rmdir(srv.data + ".unpack")
# It was made in another directory, as a moved or restored data directory is:
# what it keeps names its files from inside it, so nothing needs rewriting.
with srv.db() as db:
    stored = [r[0] for r in db.execute("SELECT yaml_file FROM submission")]
check("signups name their files from inside the data directory",
      stored and all(not p.startswith("/") and ".." not in p.split("/") and os.path.isfile(os.path.join(srv.data, p)) for p in stored), stored)
srv.start()
try:
    with srv.db() as db:
        version = db.execute("PRAGMA user_version").fetchone()[0]
        ids = dict(db.execute("SELECT name, id FROM season").fetchall())
        signup = db.execute("SELECT id FROM submission WHERE slot_name = 'Ann' AND season_id = ?", (ids["Signups"],)).fetchone()[0]
        problems = db.execute("PRAGMA foreign_key_check").fetchall() + [
            r for r in db.execute("PRAGMA integrity_check").fetchall() if r[0] != "ok"]
    r = srv.cli("auth", "status")
    check("the baseline database opens and migrates", version == schema and r.returncode == 0, (version, schema, r.stderr))
    check("with its references and pages intact", problems == [], problems)
    upgraded = schema_of(srv)
    check("and the same schema as a fresh one", upgraded == fresh_schema,
          [r for r in upgraded if r not in fresh_schema] + [r for r in fresh_schema if r not in upgraded])
    root = srv.session()
    r = root.post(srv.base + "/login", data=dict(username="root", password="correct horse battery"))
    check("its accounts still sign in", "bifrost_session" in root.cookies.get_dict(), r.text[-300:])
    seasons = get(root, "/admin/seasons").text
    check("its seasons are all there", all(n in seasons for n in ("Draft", "Signups", "Live", "Archived")), seasons[-1500:])
    page = get(root, f"/s/{ids['Signups']}/signup/game/{signup}").text
    check("with their signups, notes and seats", "Ann" in page and "Bring snacks" in page and "Cat" in page, page[-1200:])
    with srv.db() as db:
        (ann_yaml,) = db.execute("SELECT yaml_file FROM submission WHERE id = ?", (signup,)).fetchone()
    post(root, f"/s/{ids['Signups']}/signup/delete", data=dict(id=signup))
    with srv.db() as db:
        left = db.execute("SELECT count(*) FROM submission WHERE id = ?", (signup,)).fetchone()[0]
    check("and deleting one in the moved directory removes its file", left == 0 and not os.path.exists(os.path.join(srv.data, ann_yaml)), (left, ann_yaml))
    check("an archived season keeps its summary", "Cat" in get(root, f"/s/{ids['Archived']}/season").text)
    r = get(root, f"/s/{ids['Live']}/admin/common/vanilla/allowlist")
    check("a live season keeps its common game and allowlist", r.text == "Alex\nSteve\n", repr(r.text))
    check("the instance keeps its settings", 'value="ap.example"' in get(root, "/admin/settings").text and "A token is set" in get(root, "/admin/settings").text)
    check("and its catalogs", "vetted" in get(root, "/admin/catalog").text and "plays as <b>Commons</b>" in get(root, "/admin/common-games").text)
    check("the live season comes back up", wait_for(lambda: "Vanilla" in get(root, f"/s/{ids['Live']}/board").text and "Ann" in get(root, f"/s/{ids['Live']}/board").text, 90, 3))
finally:
    srv.stop()

# `bifrost update` is for the single-file build (see bundle.py); this one has
# no bundled runtime.
r = srv.cli("update")
check("a build without a bundled runtime refuses to update itself",
      r.returncode == 1 and "only the native single-file build" in r.stderr, (r.returncode, r.stderr))

sys.exit(summary())
