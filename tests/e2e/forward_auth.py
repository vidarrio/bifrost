"""Behind a forward-auth proxy: who a season's pages believe. No real proxy; the
requests carry what one would pass on."""
import sys

import requests

from lib import Server, check, summary

ENV = {
    "BIFROST_TRUSTED_PROXIES": "127.0.0.1/32",
    "BIFROST_FWD_ROLE_GROUPS": "sysadmin=adm,user=players",
}
SECRET, OPEN = "abcdefghjkmnpqrs", "bcdefghjkmnpqrst"
ADMIN = {"x-auth-request-user": "alice", "x-auth-request-groups": "adm"}


def server(tag_env, fresh=False):
    return Server("fwdauth", port=18194, ap_ports="39202-39204", fresh=fresh, extra_env={**ENV, **tag_env})


def status(srv, path, headers=None):
    return requests.get(srv.base + path, headers=headers or {}, allow_redirects=False, timeout=10).status_code


srv = server({}, fresh=True).start()
try:
    srv.stop()
    r = srv.cli("auth", "mode", "forward-auth")
    check("switches to forward auth", r.returncode == 0, r.stderr)
    with srv.db() as db:
        db.execute("INSERT INTO season (id, name, state, created_at, private_live) VALUES (?, 'Secret', 'archived', 0, 1)", (SECRET,))
        db.execute("INSERT INTO season (id, name, state, created_at, private_live, summary) VALUES (?, 'Open', 'archived', 0, 0, NULL)", (OPEN,))

    srv = server({}).start()
    check("without season_pages, headers on a season page are ignored", status(srv, f"/s/{SECRET}/board", ADMIN) == 401)
    check("…on the log", status(srv, f"/s/{SECRET}/log", ADMIN) == 401)
    check("…and on the summary", status(srv, f"/s/{SECRET}/season", ADMIN) == 401)
    with srv.db() as db:
        alice = db.execute("SELECT COUNT(*) FROM account WHERE username = 'alice'").fetchone()[0]
    check("…so nobody can be provisioned there", alice == 0)
    check("an open season's page needs no identity", status(srv, f"/s/{OPEN}/log") == 200)
    for bare in (f"/s/{SECRET}", f"/s/{SECRET}/"):
        r = requests.get(srv.base + bare, headers=ADMIN, allow_redirects=False, timeout=10)
        check(f"{bare} only redirects, whoever's asking", r.status_code == 303 and r.headers["location"].endswith("/board"), r.status_code)
    check("the home page doesn't believe headers", "Secret" not in requests.get(srv.base + "/", headers=ADMIN, timeout=10).text)
    srv.stop()

    srv = server({"BIFROST_FWD_SEASON_PAGES": "1"}).start()
    check("with season_pages, a member of the proxy's admins gets in", status(srv, f"/s/{SECRET}/log", ADMIN) == 200)
    check("…a player who isn't in the season doesn't",
          status(srv, f"/s/{SECRET}/log", {"x-auth-request-user": "bob", "x-auth-request-groups": "players"}) == 404)
    check("…and with no identity it's a sign-in", status(srv, f"/s/{SECRET}/log") == 401)
    check("the login path isn't a season page", status(srv, "/login") != 401)
finally:
    srv.stop()

sys.exit(summary())
