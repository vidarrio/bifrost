"""Seasonal minesweeper's bonus-hint limit: a season setting (5 by default,
0 for no limit) that its moderators set from the season's admin page, and
the minesweeper page tells players. Wins by scope: a season's earns a hint up
to that limit, a general one earns none, and each is recorded where it belongs."""
import os
import re
import sqlite3
import sys

from lib import Server, check, first_game, get, new_season, post, summary

def play(s, prefix, lose=False):
    """Play a new standard board: win it, or lose it on a mine it has worked out.
    Boards are solvable without guessing, but this solver is simpler than the
    server's, so a board it gets stuck on is thrown away for a new one. Returns
    the last response."""
    for _ in range(50):
        new = post(s, f"{prefix}/minesweeper/new", data=dict(kind="standard")).json()
        w, h = new["w"], new["h"]
        shown = {}
        mines = set()
        safe = {(h // 2, w // 2)}
        while True:
            if lose and mines:
                r, c = next(iter(mines))
                return post(s, f"{prefix}/minesweeper/reveal", data=dict(r=r, c=c, kind="standard")).json()
            if not safe:
                break
            r, c = safe.pop()
            if (r, c) in shown:
                continue
            res = post(s, f"{prefix}/minesweeper/reveal", data=dict(r=r, c=c, kind="standard")).json()
            if res.get("state") == "won" and not lose:
                return res
            if res.get("state") != "play":
                break
            shown.update({(x["r"], x["c"]): x["n"] for x in res["cells"]})
            # What each number still needs, then one number's cells inside another's.
            needs = []
            for (y, x), n in shown.items():
                around = {(y + a, x + b) for a in (-1, 0, 1) for b in (-1, 0, 1)
                          if (a or b) and 0 <= y + a < h and 0 <= x + b < w} - set(shown)
                left = n - len(around & mines)
                needs.append((around - mines, left))
            for cells, left in needs:
                if cells and left == 0:
                    safe |= cells
                elif cells and left == len(cells):
                    mines |= cells
            for a, m in needs:
                for b, n in needs:
                    if a and a < b:
                        if n == m:
                            safe |= b - a
                        elif n - m == len(b - a):
                            mines |= b - a
            safe -= mines
    raise AssertionError("no board solved in 50 tries")


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

    # Wins, by scope.
    def rows(sql, *args):
        db = sqlite3.connect(f"file:{os.path.join(srv.data, 'bifrost.db')}?mode=ro", uri=True, timeout=10)
        try:
            return db.execute(sql, args).fetchall()
        finally:
            db.close()

    def credit():
        got = rows("SELECT credits FROM hint_credit WHERE season_id = ?", sid)
        return got[0][0] if got else 0

    def results(season):
        where = "season_id IS NULL" if season is None else "season_id = ?"
        return rows(f"SELECT won, earned FROM mine_result WHERE {where}", *([] if season is None else [season]))

    post(root, f"/s/{sid}/admin/minesweeper/hints", data=dict(hints="1"))
    r = play(alice, "")
    check("a general win is a win", r.get("state") == "won", r)
    check("…that earns no hint", not r["earned"] and r["credits"] is None and not r["capped"] and credit() == 0, r)
    check("…and is recorded outside any season", results(None) == [(1, 0)] and results(sid) == [], results(None))

    r = play(alice, f"/s/{sid}")
    check("a season's win earns a hint", r.get("state") == "won" and r["earned"] and r["credits"] == 1 and credit() == 1, r)
    check("…and is recorded under the season", results(sid) == [(1, 1)], results(sid))

    r = play(alice, f"/s/{sid}")
    check("a win past the limit earns nothing", r.get("state") == "won" and not r["earned"] and r["capped"] and credit() == 1, r)
    check("…but is still recorded", sorted(results(sid)) == [(1, 0), (1, 1)], results(sid))

    r = play(alice, f"/s/{sid}", lose=True)
    check("a loss earns nothing", r.get("state") == "lost" and credit() == 1, r)
    check("…and is recorded under the season", sorted(results(sid)) == [(0, 0), (1, 0), (1, 1)], results(sid))
    check("general results didn't move", results(None) == [(1, 0)], results(None))
finally:
    srv.stop()

sys.exit(summary())
