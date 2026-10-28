"""Upgrades: a start under a different Bifrost version backs up the data dir
before touching the database (and the backup holds the old database), a
start under the same version doesn't, and a database a newer Bifrost has
migrated is refused rather than run on."""
import os
import sqlite3
import sys
import zipfile

from lib import Server, check, summary

PORT, AP_PORTS = 18193, "39175-39177"


def stamp(srv, version=None, schema=None):
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as db:
        if version is not None:
            db.execute("UPDATE setting SET value = ? WHERE key = 'bifrost_version'", (version,))
        if schema is not None:
            db.execute(f"PRAGMA user_version = {schema}")
        row = db.execute("SELECT value FROM setting WHERE key = 'bifrost_version'").fetchone()
        return db.execute("PRAGMA user_version").fetchone()[0], row and row[0]


def backups(srv):
    return sorted(n for n in os.listdir(os.path.join(srv.data, "backups")) if n.endswith(".zip")) \
        if os.path.isdir(os.path.join(srv.data, "backups")) else []


srv = Server("upgrade", port=PORT, ap_ports=AP_PORTS, extra_env={"BIFROST_BACKUP_HOUR": "off"})
srv.start()
try:
    root = srv.setup_sysadmin()
finally:
    srv.stop()
schema, version = stamp(srv)
check("a fresh data dir is stamped with a schema version and this Bifrost's", schema >= 1 and version, (schema, version))
check("a first start takes no backup", backups(srv) == [], backups(srv))

# Same version again: nothing to do.
srv = Server("upgrade", port=PORT, ap_ports=AP_PORTS, fresh=False, extra_env={"BIFROST_BACKUP_HOUR": "off"})
srv.start()
srv.stop()
check("a restart on the same version takes no backup", backups(srv) == [], backups(srv))

# Pretend an older Bifrost ran here last.
stamp(srv, version="0.0.0-old")
srv = Server("upgrade", port=PORT, ap_ports=AP_PORTS, fresh=False, extra_env={"BIFROST_BACKUP_HOUR": "off"})
srv.start()
try:
    log = srv.logtext()
    taken = backups(srv)
    check("an upgrade backs up first", len(taken) == 1, taken)
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

# A newer Bifrost migrated the database past this build: refuse it.
stamp(srv, version="99.0.0", schema=schema + 1)
srv = Server("upgrade", port=PORT, ap_ports=AP_PORTS, fresh=False, extra_env={"BIFROST_BACKUP_HOUR": "off"})
try:
    srv.start()
    check("a database from a newer Bifrost is refused", False, "it started")
    srv.stop()
except RuntimeError:
    log = srv.logtext()
    check("a database from a newer Bifrost is refused", srv.proc.returncode == 1, srv.proc.returncode)
    check("with a message naming the version and the way back",
          "last used by Bifrost 99.0.0" in log and "bifrost backup restore" in log, log[-2000:])
    srv.log.close()
check("and left alone", stamp(srv) == (schema + 1, "99.0.0"), stamp(srv))

sys.exit(summary())
