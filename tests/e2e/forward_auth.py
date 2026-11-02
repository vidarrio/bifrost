"""Behind a forward-auth proxy: who a season's pages believe. No real proxy; the
requests carry what one would pass on."""
import os
import sqlite3
import sys

import requests

from lib import Server, check, summary

ENV = {
    "BIFROST_TRUSTED_PROXIES": "127.0.0.1/32",
    "BIFROST_FWD_ROLE_GROUPS": "sysadmin=adm,user=players",
}
ADMIN = {"x-auth-request-user": "alice", "x-auth-request-groups": "adm"}


def server(tag_env, fresh=False):
    return Server("fwdauth", port=18195, ap_ports="39202-39204", fresh=fresh, extra_env={**ENV, **tag_env})


def status(srv, path, headers=None):
    return requests.get(srv.base + path, headers=headers or {}, allow_redirects=False, timeout=10).status_code


srv = server({}, fresh=True).start()
try:
    srv.stop()
    r = srv.cli("auth", "mode", "forward-auth")
    check("switches to forward auth", r.returncode == 0, r.stderr)
    with sqlite3.connect(os.path.join(srv.data, "bifrost.db")) as db:
        db.execute("INSERT INTO season (id, name, state, created_at, private_live) VALUES (1, 'Secret', 'archived', 0, 1)")
        db.execute("INSERT INTO season (id, name, state, created_at, private_live, summary) VALUES (2, 'Open', 'archived', 0, 0, NULL)")

    srv = server({}).start()
    check("without season_pages, headers on a season page are ignored", status(srv, "/s/1/board", ADMIN) == 401)
    check("…on the log", status(srv, "/s/1/log", ADMIN) == 401)
    check("…and on the summary", status(srv, "/s/1/season", ADMIN) == 401)
    check("…so nobody can be provisioned there",
          sqlite3.connect(os.path.join(srv.data, "bifrost.db")).execute("SELECT COUNT(*) FROM account WHERE username = 'alice'").fetchone()[0] == 0)
    check("an open season's page needs no identity", status(srv, "/s/2/log") == 200)
    for bare in ("/s/1", "/s/1/", "/s/01"):
        r = requests.get(srv.base + bare, headers=ADMIN, allow_redirects=False, timeout=10)
        check(f"{bare} only redirects, whoever's asking", r.status_code == 303 and r.headers["location"].endswith("/board"), r.status_code)
    check("the home page doesn't believe headers", "Secret" not in requests.get(srv.base + "/", headers=ADMIN, timeout=10).text)
    srv.stop()

    srv = server({"BIFROST_FWD_SEASON_PAGES": "1"}).start()
    check("with season_pages, a member of the proxy's admins gets in", status(srv, "/s/1/log", ADMIN) == 200)
    check("…a player who isn't in the season doesn't",
          status(srv, "/s/1/log", {"x-auth-request-user": "bob", "x-auth-request-groups": "players"}) == 404)
    check("…and with no identity it's a sign-in", status(srv, "/s/1/log") == 401)
    check("the login path isn't a season page", status(srv, "/login") != 401)
finally:
    srv.stop()

sys.exit(summary())
