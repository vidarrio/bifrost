#!/usr/bin/env python3
"""Fill a fresh demo build with seasons in each state, for a demo snapshot.

    seed.py <bifrost url> <its data dir>

Run it once against a demo build (`cargo build --features demo`) started on an
empty data dir, then stop Bifrost and copy the data dir: that copy is the
snapshot BIFROST_DEMO_SNAPSHOT restores. See README.md next to this file.

It signs in through /demo/as/<role>, so it only works on a demo build. The
seasons play real games that need no ROM, and their games are part played:
the seed checks locations, adds hints and marks some games finished through
the season console, which a demo only allows on seasons it doesn't start
with, and stars a couple of the hints as priorities. It reads which
locations to check from the seed's spoiler in the data dir.

Two things go straight into the data dir: the common game's folder, since the
demo locks the common-games console, and a history of minesweeper results for
the leaderboards, since a result's time is how long the board took to play.

Generating the seasons and playing them takes a few minutes.
"""
import html
import os
import random
import re
import shlex
import shutil
import sqlite3
import sys
import time
import zipfile

import requests

HERE = os.path.dirname(os.path.abspath(__file__))
GUIDE = os.path.join(HERE, "..", "..", "docs", "examples", "common", "minecraft-26.2", "guide.md")
COMMON = "minecraft"
# Every game here generates without a ROM or any other file of the player's.
# (account, slot, game)
SUMMER = [
    ("player", "Ann", "A Short Hike"),
    ("player", "AnnDice", "Yacht Dice"),
    ("moderator", "Mia", "Celeste 64"),
    ("sysadmin", "Sam", "Timespinner"),
]
AUTUMN = [
    ("player", "Ann", "Hollow Knight"),
    ("player", "AnnTunic", "TUNIC"),
    ("moderator", "Mia", "The Witness"),
    ("sysadmin", "Sam", "Risk of Rain 2"),
    ("sysadmin", "SamDash", "Muse Dash"),
]
WINTER = [
    ("player", "Ann", "Stardew Valley"),
    ("moderator", "Mia", "Terraria"),
]
# Which account plays each slot.
ACCOUNT = {slot: account for account, slot, _ in SUMMER + AUTUMN + WINTER}
# How far each slot has got: how many of its locations it has checked, or
# "goal" for a game marked finished, which releases the rest of its items. A
# check goes through the season console, one location a command, and each
# command takes a second or so: keep the numbers small.
SUMMER_PLAY = {"Ann": "goal", "Mia": "goal", "Sam": "goal", "AnnDice": 30}
AUTUMN_PLAY = {"SamDash": "goal", "Mia": 30, "Ann": 25, "Sam": 20, "AnnTunic": 12, "Commons": 5}
HINTS = 4
# How many of those hints the item's owner has starred as a priority, in the
# live season. The archived one has none to give: most of its games finished.
PRIORITIZED = 2
LINE = re.compile(r"^\s*(?P<loc>.+) \((?P<finder>[^()]+)\): (?P<item>.+) \((?P<owner>[^()]+)\)\s*$")
rng = random.Random(26)


def yaml(slot, game):
    return f"name: {slot}\ngame: {game}\n{game}: {{}}\n"


def main(base, data):
    base = base.rstrip("/")

    def session(role):
        s = requests.Session()
        r = s.post(f"{base}/demo/as/{role}", allow_redirects=False)
        if r.status_code != 303 or not r.headers.get("x-bifrost-demo"):
            sys.exit(f"seed: {base} isn't a demo build (/demo/as/{role} answered {r.status_code})")
        # Over plain HTTP, a Secure session cookie is never sent back.
        if s.get(f"{base}/api/me").status_code != 200:
            sys.exit("seed: the session cookie didn't come back. Over plain HTTP, start the "
                     "instance you seed with BIFROST_INSECURE_COOKIES=1")
        return s

    roles = {r: session(r) for r in ("player", "moderator", "sysadmin")}
    player, mod, admin = roles["player"], roles["moderator"], roles["sysadmin"]

    # post, season, signup and generate do what tests/e2e/lib.py's post,
    # new_season and generate (and the suites' signups) do, on the same pages.
    # This file ships, so it can't import lib.py. When a page's text that they
    # look for changes ("Generated seed", the banner classes, the season's
    # address), change both. tests/e2e/demo.py runs this seed, so CI catches a
    # copy that was missed.
    def post(s, path, **kw):
        r = s.post(base + path, timeout=120, **kw)
        r.raise_for_status()
        return r

    def season(s, name):
        r = post(s, "/admin/seasons", data=dict(name=name))
        m = re.search(r"/s/([0-9a-z]+)/admin", r.url)
        if not m:
            sys.exit(f"seed: creating {name!r} failed: {r.text[:300]}")
        return m.group(1)

    def signup(sid, role, slot, game):
        for _ in range(10):
            r = roles[role].post(f"{base}/s/{sid}/signup", timeout=120,
                                 files={"yaml": ("player.yaml", yaml(slot, game).encode())})
            if r.status_code != 429:
                break
            time.sleep(int(r.headers.get("retry-after", "5")))
        r.raise_for_status()
        if 'class="banner ok"' not in r.text:
            sys.exit(f"seed: {slot}'s signup ({game}) failed: {r.text[:300]}")

    def state(sid):
        """The season's state, from its row in the console's season list."""
        for row in admin.get(f"{base}/admin/seasons").text.split("<tr"):
            if f"/s/{sid}/" in row:
                m = re.search(r'<span class="tag">([a-z]+)</span>', row)
                return m and m.group(1)

    def generate(sid):
        post(admin, f"/s/{sid}/admin/generate")
        for _ in range(200):
            t = admin.get(f"{base}/s/{sid}/admin").text
            if "Generated seed" in t:
                return
            if re.search(r'class="banner bad">\s*Failed', t):
                sys.exit(f"seed: generation failed in season {sid}")
            time.sleep(3)
        sys.exit(f"seed: season {sid} didn't generate in time")

    def cards(sid):
        """slot name -> the Bifrost game id of its card on the live board."""
        out = {}
        for card in admin.get(f"{base}/s/{sid}/board").text.split('class="slot cardlink')[1:]:
            gid = re.match(r'[^"]*" href="game/([0-9a-z]+)"', card)
            for who in re.findall(r'<span class="who">([^<]+)</span>', card):
                if gid:
                    out[who] = gid.group(1)
        return out

    def wait_board(sid, slots):
        """Wait until the live board shows every slot, so the game server is up."""
        for _ in range(100):
            if set(slots) <= set(cards(sid)):
                return
            time.sleep(3)
        sys.exit(f"seed: season {sid}'s board never showed {slots}")

    def console(sid, cmd):
        # Right after generation, Bifrost may not be connected to the game
        # server yet: it answers "error: …" until it is.
        for _ in range(20):
            out = post(admin, f"/s/{sid}/admin/console", data=dict(cmd=cmd)).json().get("output", "")
            if not out.startswith(("error:", "AP connection not configured")):
                return out
            time.sleep(3)
        sys.exit(f"seed: season {sid}'s console kept failing: {out}")

    def spoiler(sid):
        """The seed's (finder, location, item, owner, progression) placements: the
        playthrough's first, in the order it finds them, then the rest. Events
        are left out. Stops the seed if the spoiler isn't there or reads as
        nothing, as it would if a new Archipelago changed its format."""
        z = zipfile.ZipFile(os.path.join(data, "seasons", sid, "output", "current.zip"))
        name = next((n for n in z.namelist() if n.endswith("_Spoiler.txt")), None)
        if not name:
            sys.exit(f"seed: season {sid}'s seed has no spoiler")
        text = z.read(name).decode("utf-8", "replace")
        if "\nLocations:" not in text:
            sys.exit(f"seed: season {sid}'s spoiler has no Locations section")
        locs = text[text.rfind("\nLocations:"):]
        play = text[text.find("\nPlaythrough:"):] if "\nPlaythrough:" in text else ""
        seen, out = set(), []
        # The playthrough lists only the placements of progression items.
        lines = [(l, True) for l in play.splitlines()] + [(l, False) for l in locs.splitlines()]
        for line, progression in lines:
            m = LINE.match(line)
            # Events (an item named for its location) and Hollow Knight's room
            # transitions ("Room_01[left1]") aren't locations anyone checks.
            if not m or m["loc"] == m["item"] or re.fullmatch(r"\w+\[\w+\]", m["loc"]):
                continue
            key = (m["finder"], m["loc"])
            if key not in seen:
                seen.add(key)
                out.append((m["finder"], m["loc"], m["item"], m["owner"], progression))
        if not out:
            sys.exit(f"seed: read no placements from season {sid}'s spoiler")
        return out

    def play(sid, plan, stars=0):
        # The common game's card shows "Everyone", not its slot.
        wait_board(sid, [s for s in plan if s != "Commons"])
        places = spoiler(sid)
        ids = cards(sid)
        console(sid, "/players")  # waits until Bifrost is connected to the game server
        # Goals first, so the newest activity on the board is ordinary checks
        # rather than the items a goal releases all at once.
        for slot in (s for s, how in plan.items() if how == "goal"):
            gid = ids.get(slot)
            if not gid:
                sys.exit(f"seed: no card for {slot} on season {sid}'s board")
            for _ in range(10):
                r = post(admin, f"/s/{sid}/game/{gid}/finish", data=dict(back=f"/game/{gid}"))
                if "Marked finished" in admin.get(f"{base}/s/{sid}/game/{gid}").text:
                    break
                time.sleep(3)
            else:
                notice = re.search(r'class="banner[^"]*">([^<]*)', r.text)
                sys.exit(f"seed: marking {slot} finished didn't take: {notice and notice.group(1)}")
            print(f"seed:   {slot}: goal", flush=True)
        done = set()
        for slot, want in plan.items():
            if want == "goal":
                continue
            mine = [p for p in places if p[0] == slot]
            sent, missed = 0, []
            for finder, loc, _, _, _ in mine:
                if sent >= want:
                    break
                out = console(sid, f"/send_location {shlex.quote(slot)} {shlex.quote(loc)}")
                # Whatever the reply, the location may be checked now, so it's
                # never hinted: silence can be a reply that came too late for the
                # console, a retried command that went through the first time, or
                # a goal's collect, and other connections' notices can come first.
                done.add((finder, loc))
                # "Ann sent X to Mia (loc)", or "Ann found their X (loc)".
                if re.search(rf"(?m)^{re.escape(slot)} (sent|found) ", out):
                    sent += 1
                elif not out.startswith("(command accepted"):
                    missed.append(f"{loc}: {out}")
            print(f"seed:   {slot}: {sent} of {len(mine)} locations checked", flush=True)
            # Far short of the plan, mostly through names the server didn't know,
            # means the spoiler's names no longer match the server's.
            if sent < want / 2 and len(missed) > sent:
                sys.exit(f"seed: {slot} checked only {sent} of {want}: " + "; ".join(missed[:3]))
        # A few hints: progression items still to find, in others' games. The
        # console doesn't always see a hint's reply (the server sends it to the
        # two slots involved only), so this counts the commands, not replies.
        wanted = [(owner, item, finder, loc) for finder, loc, item, owner, progression in places
                  if progression and owner in plan and finder != owner and plan.get(owner) != "goal"
                  and plan.get(finder) != "goal" and (finder, loc) not in done]
        # Each is between two people, so a starred hint shows as wanted on
        # someone else's card, and each item is hinted once: a /hint names every
        # copy. Only between games with a card of a player's own: the common
        # game's card shows "Everyone", so a hint it owns has no game page to be
        # starred from, and one it holds would show as wanted on the shared card
        # rather than a player's. Spread out where there are enough (each slot
        # owning and holding one at most), then any. The checks above use up the
        # playthrough's locations first, so there may not be many to pick from.
        wanted = [w for w in wanted if w[0] in ids and w[2] in ids
                  and ACCOUNT.get(w[0]) != ACCOUNT.get(w[2])]
        hinted, owners, finders, items = [], set(), set(), set()
        for spread in (True, False):
            for owner, item, finder, loc in wanted:
                if len(hinted) < HINTS and (owner, item) not in items \
                        and not (spread and (owner in owners or finder in finders)):
                    hinted.append((owner, item, finder, loc))
                    owners.add(owner)
                    finders.add(finder)
                    items.add((owner, item))
        if len(hinted) < stars:
            sys.exit(f"seed: only {len(hinted)} hints to give in season {sid} "
                     f"({len(wanted)} candidates), {stars} wanted for stars")
        for owner, item, _, _ in hinted:
            console(sid, f"/hint {shlex.quote(owner)} {shlex.quote(item)}")

        def hint_row(page, item, finder, loc):
            """The owner's game page's row for this copy of the item: a hint names
            every copy, so the row is found by where it is and who holds it
            ("item @ location · finder", as the page shows it). "found" if it's
            listed as found (" ✓"), which can't be starred."""
            text = admin.get(base + page).text
            for r in text.split('class="hintrow"')[1:]:
                shown = html.unescape(r.partition("<span>")[2].partition("</span>")[0])
                if shown == f"{item} @ {loc} · {finder}":
                    return r.split("</div>", 1)[0]
            for r in text.split('class="hintrow found"')[1:]:
                shown = html.unescape(r.partition("<span>")[2].partition("</span>")[0])
                if shown == f"{item} @ {loc} · {finder} ✓":
                    return "found"

        # Star a few of them, as their owners would, from the owner's game page
        # (where the hint shows once the board has seen it). One that shows as
        # found, or never shows, is passed over for the next.
        starred, unshown = 0, []
        for owner, item, finder, loc in hinted:
            if starred >= stars:
                break
            page = f"/s/{sid}/game/{ids[owner]}"
            for _ in range(20):
                row = hint_row(page, item, finder, loc)
                if row == "found" or (row and 'action="prioritize"' in row):
                    break
                time.sleep(3)
            else:
                row = None
            if row in ("found", None):
                why = "found already" if row else "never shown"
                unshown.append(f"{owner}'s {item} at {loc} ({why})")
                continue
            form = dict(re.findall(r'name="(\w+)" value="([^"]*)"', row.split("</form>", 1)[0]))
            post(admin, f"/s/{sid}/prioritize", data={k: html.unescape(v) for k, v in form.items()})
            # The button itself: the page's stylesheet names "★ prioritized" too.
            if "★ prioritized</button>" not in (hint_row(page, item, finder, loc) or ""):
                sys.exit(f"seed: prioritizing the hint for {owner}'s {item} didn't take")
            starred += 1
        if starred < stars:
            sys.exit(f"seed: only {starred} of {stars} hints starred in season {sid}; "
                     "not starrable on their game page: " + "; ".join(unshown))
        if hinted:
            print(f"seed:   {len(hinted)} hints, {starred} prioritized", flush=True)

    def minesweeper(sid, games):
        """Write a history of finished boards: (account, kind, boards, win share)."""
        db = sqlite3.connect(os.path.join(data, "bifrost.db"), timeout=30)
        now = int(time.time())
        with db:
            for user, kind, boards, wins in games:
                (acct,) = db.execute("SELECT id FROM account WHERE username=?", (user,)).fetchone()
                lo, hi = (45, 160) if kind == "standard" else (150, 420)
                for i in range(boards):
                    won = rng.random() < wins
                    secs = rng.uniform(lo, hi) if won else rng.uniform(8, lo)
                    ts = now - rng.randint(3600, 21 * 86400)
                    db.execute("INSERT INTO mine_result (season_id, account_id, won, duration_ms, ts, kind) "
                               "VALUES (?, ?, ?, ?, ?, ?)", (sid, acct, int(won), int(secs * 1000), ts, kind))
        db.close()

    # The common game: a Minecraft server as the demo shows it (no apworld or
    # client files: its slot plays ChecksFinder, which ships with Archipelago).
    d = os.path.join(data, "common", COMMON)
    os.makedirs(os.path.join(d, "client"), exist_ok=True)
    with open(os.path.join(d, "game.toml"), "w") as f:
        f.write('name = "Minecraft 26.2"\nslot = "Commons"\naddress = "mc.example.org:25565"\n\n'
                '[identifier]\nkind = "minecraft"\nlabel = "Minecraft username"\n'
                'pattern = "[A-Za-z0-9_]{3,16}"\nhint = "3 to 16 letters, digits or underscores"\n\n'
                '[[download]]\nfile = "client/README.txt"\nlabel = "Client files (a placeholder in the demo)"\n')
    with open(os.path.join(d, "player.yaml"), "w") as f:
        f.write(yaml("Commons", "ChecksFinder"))
    with open(os.path.join(d, "client", "README.txt"), "w") as f:
        f.write("A real common game would offer its client files here: a mod loader, the mod, a modpack.\n")
    shutil.copy(GUIDE, os.path.join(d, "guide.md"))

    print("seed: general minesweeper results", flush=True)
    minesweeper(None, [
        ("player", "standard", 14, 0.65), ("player", "expert", 6, 0.35),
        ("moderator", "standard", 9, 0.8), ("moderator", "expert", 5, 0.5),
        ("sysadmin", "standard", 7, 0.5),
    ])

    print("seed: an archived season", flush=True)
    old = season(admin, "Summer season")
    post(admin, f"/s/{old}/admin/minesweeper", data=dict(open="1"))
    post(admin, f"/s/{old}/admin/signups", data=dict(open="1"))
    for role, slot, game in SUMMER:
        signup(old, role, slot, game)
    generate(old)
    play(old, SUMMER_PLAY)
    # Before archiving, which freezes the season's leaderboards.
    minesweeper(old, [("player", "standard", 6, 0.7), ("moderator", "standard", 5, 0.6),
                      ("sysadmin", "standard", 3, 0.67)])
    # Archiving waits for the live board to see the games, which can take a
    # few minutes after generation.
    for _ in range(100):
        post(admin, f"/s/{old}/admin/archive-season", data=dict(name="Summer season"))
        if state(old) == "archived":
            break
        time.sleep(3)
    else:
        sys.exit(f"seed: season {old} didn't archive")

    print("seed: a live season with a common game", flush=True)
    live = season(admin, "Autumn season")
    post(admin, f"/s/{live}/admin/common", data={"do": "add", "key": COMMON})
    post(admin, f"/s/{live}/admin/minesweeper", data=dict(open="1"))
    post(admin, f"/s/{live}/admin/signups", data=dict(open="1"))
    for role, slot, game in AUTUMN:
        signup(live, role, slot, game)
    generate(live)
    post(player, f"/s/{live}/common/{COMMON}/ident", data=dict(name="Steve"))
    play(live, AUTUMN_PLAY, PRIORITIZED)
    minesweeper(live, [("player", "standard", 4, 0.75), ("moderator", "standard", 6, 0.5),
                       ("sysadmin", "standard", 2, 1.0)])

    print("seed: a season taking signups, run by the moderator", flush=True)
    nxt = season(mod, "Winter season")
    post(mod, f"/s/{nxt}/admin/signups", data=dict(open="1"))
    for role, slot, game in WINTER:
        signup(nxt, role, slot, game)

    print("seed: done; stop Bifrost and copy its data dir for the snapshot", flush=True)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
