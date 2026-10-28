"""Upgrades: a start under a different Bifrost version backs up the data dir
before touching the database (and the backup holds the old database), a
start under the same version doesn't, and a database a newer Bifrost has
migrated is refused rather than run on. CLI commands never back up or
migrate, and refuse a database on another schema. A failed pre-upgrade
backup stops the start only when the schema would change."""
import os
import sqlite3
import sys
import zipfile

from lib import Server, check, summary

PORT, AP_PORTS = 18193, "39175-39177"
ENV = {"BIFROST_BACKUP_HOUR": "off"}


def stamp(srv, version=None, schema=None):
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as db:
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
check("a first start takes no backup", backups(srv) == [], backups(srv))

srv = server()
srv.start()
srv.stop()
check("a restart on the same version takes no backup", backups(srv) == [], backups(srv))

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
        with sqlite3.connect(path) as db:
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
stamp(srv, version="0.0.0-old", schema=0)  # as if the schema were older
r = srv.cli("auth", "status")
check("a command refuses a database on an older schema", r.returncode == 1 and "start Bifrost" in r.stderr, (r.returncode, r.stderr))
srv = server(BIFROST_BACKUP_DIR=blocked)
check("a failed backup stops a start that would migrate", refused(srv) == 1)
check("it says why", "not migrating the database without one" in srv.logtext(), srv.logtext()[-1500:])
check("and leaves the database alone", stamp(srv) == (0, "0.0.0-old"), stamp(srv))
os.remove(os.path.join(srv.data, "blocker"))
srv = server()
srv.start()
srv.stop()
check("with room for the backup it upgrades", stamp(srv) == (schema, version), stamp(srv))

# A newer Bifrost migrated the database past this build: refuse it.
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

sys.exit(summary())
