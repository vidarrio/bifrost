"""Seasonal minesweeper's bonus-hint limit: a season setting (5 by default,
0 for no limit) that its moderators set from the season's admin page, and
the minesweeper page tells players."""
import re
import sys

from lib import Server, check, first_game, get, new_season, post, summary

srv = Server("minesweeper", port=18195, ap_ports="39178-39180")
srv.start()
try:
    root = srv.setup_sysadmin()
    alice = srv.invite_user(root, "alice")
    sid = new_season(root, "Mines")
    post(root, f"/s/{sid}/admin/minesweeper", data=dict(open="1"))
    # A season's minesweeper is for its players.
    post(root, f"/s/{sid}/admin/signups", data=dict(open="1"))
    post(alice, f"/s/{sid}/signup", files={"yaml": ("y.yaml", b"name: Alice\ngame: ChecksFinder\nChecksFinder: {}\n")})

    def limit():
        m = re.search(r'name="hints" value="(\d+)"', get(root, f"/s/{sid}/admin").text)
        return m and int(m.group(1))

    check("a new season allows 5 bonus hints a day", limit() == 5, limit())
    check("the minesweeper page says so", "up to 5 bonus hints in any 24 hours" in get(alice, f"/s/{sid}/minesweeper").text)

    post(root, f"/s/{sid}/admin/minesweeper/hints", data=dict(hints="12"))
    check("a moderator sets the limit", limit() == 12, limit())
    check("players see the new limit", "up to 12 bonus hints in any 24 hours" in get(alice, f"/s/{sid}/minesweeper").text)

    post(root, f"/s/{sid}/admin/minesweeper/hints", data=dict(hints="0"))
    page = get(alice, f"/s/{sid}/minesweeper").text
    check("0 means no limit", limit() == 0 and "Bonus hints per player: no limit" in get(root, f"/s/{sid}/admin").text)
    check("and the minesweeper page mentions none", "in any 24 hours" not in page and "bonus hint" in page)

    for bad in ["-1", "1001", "two", "1.5", ""]:
        r = post(root, f"/s/{sid}/admin/minesweeper/hints", data=dict(hints=bad))
        check(f"{bad!r} is refused", "must be a whole number" in r.text and limit() == 0, (r.status_code, limit()))

    post(alice, f"/s/{sid}/admin/minesweeper/hints", data=dict(hints="3"))
    check("a player can't change it", limit() == 0, limit())
finally:
    srv.stop()

sys.exit(summary())
