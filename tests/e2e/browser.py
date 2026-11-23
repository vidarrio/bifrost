"""Pages as a real browser shows them (headless Chromium; see web.py): no
script or console errors and no failing requests, a header that lines up and
doesn't shift or show a stale identity, no sideways scrolling on a phone, tabs
that keep the page's address, and the signup wizard's buttons beside their
fields.

Each check here stands for a bug a browser found that HTTP checks and the
fake-DOM node tests didn't. Add to it when a page script or layout breaks."""
import sys

from lib import Server, check, get, new_season, post, summary, wait_for
import web

srv = Server("browser", port=18199, ap_ports="39190-39192")
srv.start()
try:
    root = srv.setup_sysadmin()
    post(root, "/admin/settings", data=dict(domain="ap.example.org", home_url="https://home.example.org/", discords=""))
    sid = new_season(root, "Spring")
    post(root, f"/s/{sid}/admin/signups", data=dict(open="1"))
    # Pages a signed-in sysadmin sees, of each kind: the landing page (its own
    # template), the console, a season's console (tabs) and its signup wizard.
    pages = ["/", "/signups", "/worlds", "/admin/seasons", "/admin/users", "/admin/settings",
             "/admin/catalog", f"/s/{sid}/admin", f"/s/{sid}/signup"]
    for p in pages:
        check(f"{p} loads", get(root, p).status_code == 200)

    def settled(pg, path):
        """Open `path` and wait until the header has the server's answer."""
        with pg.expect_event("requestfinished", lambda r: r.url.endswith("/api/me")):
            pg.goto(srv.base + path)
        web.settle(pg)
        pg.wait_for_selector("#who:not(:empty)")

    with web.browser() as b:
        # --- the header, on every page
        ctx = web.context(b, srv, root)
        pg = ctx.new_page()
        settled(pg, "/admin/seasons")
        for p in pages:
            settled(pg, p)
            items = web.header(pg)
            mids = [round(i["mid"], 1) for i in items]
            check(f"{p}: header items share one centre line",
                  len(items) >= 4 and max(mids) - min(mids) <= 0.5, items)
            check(f"{p}: the header shows the identity and both pills",
                  any("signed in as root" in i["text"] for i in items)
                  and [i["text"] for i in items if "pill" in i["cls"]] == ["admin", "HOME"], items)

        # First paint (from the identity the last page cached) is the header
        # the page ends up with: nothing turns up late and shifts the rest.
        held = web.HeldMe(pg)
        for p in pages:
            pg.goto(srv.base + p, wait_until="domcontentloaded")
            held.wait_held()
            first = web.header(pg)
            held.release()
            after = web.header(pg)
            check(f"{p}: the header doesn't change once /api/me answers",
                  len(first) >= 4 and first == after, (first, after))
        held.stop()
        ctx.close()

        # --- a phone's width: nothing scrolls sideways
        ctx = web.context(b, srv, root, width=360)
        pg = ctx.new_page()
        for p in pages:
            settled(pg, p)
            check(f"{p}: no sideways scroll at 360 px", web.overflow_x(pg) <= 0, web.overflow_x(pg))
            # The identity takes a row of its own rather than wrapping in the bar.
            lines = pg.evaluate(web.LINES, "#who")
            check(f"{p}: at 360 px the header's identity is one line", lines == 1, lines)
        # Every tab is on-screen: nothing scrolled or wrapped out of view.
        for p in (f"/s/{sid}/admin", "/admin/users"):
            settled(pg, p)
            off = pg.evaluate(web.OFFSCREEN, ".gtab")
            check(f"{p}: at 360 px every tab is on-screen", pg.locator(".gtab").count() >= 3 and not off, off)
        # Tables: every field and action on-screen (rows become cards), and
        # no name or date broken mid-word.
        for p in ("/admin/users", "/admin/seasons", "/admin/settings"):
            settled(pg, p)
            off = pg.evaluate(web.OFFSCREEN, ".utable td, .utable td a, .utable td button, .utable td select")
            check(f"{p}: at 360 px every table field and action is on-screen",
                  pg.locator(".utable td").count() > 0 and not off, off)
            broken = pg.evaluate(web.BROKEN, ".utable td")
            check(f"{p}: at 360 px no word in a table breaks mid-word", not broken, broken)
        # The settings page swaps in a fresh load panel every few seconds; its
        # table needs its labels too, or its cards lose them.
        # (No table until the first load sample.)
        panel = wait_for(lambda: (t := root.get(srv.base + "/admin/settings/load").text) and "<table" in t and t,
                         timeout=60)
        check("the refreshed load panel's table cells are labelled", bool(panel) and 'data-label="Memory"' in panel,
              (panel or "")[:200])
        # A standard minesweeper board fits a phone.
        settled(pg, "/minesweeper")
        pg.wait_for_selector(".msgrid .cell")
        w = pg.evaluate("document.querySelector('.msgrid').getBoundingClientRect().width")
        check("at 360 px a standard minesweeper board fits the screen", w <= 360 - 32, w)
        ctx.close()

        # --- the Games page's filter narrows what's shown (rendered, not just
        # marked hidden). Its core list may still be being gathered.
        pg = web.context(b, srv, root).new_page()
        if wait_for(lambda: 'id="corelist"' in root.get(srv.base + "/worlds").text, timeout=240, every=3):
            pg.goto(srv.base + "/worlds")
            pg.fill("#gamefilter", "checksfinder")
            shown = pg.evaluate("[...document.querySelectorAll('#corelist li')].filter(li => li.getClientRects().length).map(li => li.firstElementChild.textContent)")
            check("the Games page filter shows only the matching games", shown == ["ChecksFinder"], shown)
        else:
            check("the Games page lists the core games within 4 minutes", False)
        pg.context.close()

        # --- tabs keep the page's address (season pages have <base href>)
        ctx = web.context(b, srv, root)
        pg = ctx.new_page()
        settled(pg, f"/s/{sid}/admin")
        pg.click(".gtab[data-tab='tab-options']")
        want = f"{srv.base}/s/{sid}/admin#tab-options"
        try:
            pg.wait_for_url(want, timeout=3000)
        except web.TimeoutError:
            pass
        check("a tab adds its name to the page's address", pg.url == want, pg.url)
        pg.reload()
        try:
            pg.wait_for_selector("#tab-options", state="visible", timeout=3000)
        except web.TimeoutError:
            pass
        check("…and reopens on reload", pg.is_visible("#tab-options") and not pg.is_visible("#tab-overview"))
        ctx.close()

        # --- the signup wizard: each button beside the field it acts on
        for width, beside in ((1100, True), (390, False)):
            ctx = web.context(b, srv, root, width=width)
            pg = ctx.new_page()
            settled(pg, f"/s/{sid}/signup")
            pg.click("#gamelist li[data-kind=upload]")
            f, btn = web.box(pg, "#apfile"), web.box(pg, "#apbtn")
            if not (f and btn):
                check(f"signup at {width} px: the apworld field and its button are shown", False, (f, btn))
            elif beside:
                check("signup: Upload apworld sits beside its field, as tall as it",
                      btn["y"] == f["y"] and btn["height"] == f["height"] and btn["x"] > f["x"], (f, btn))
            else:
                check("signup: on a phone, Upload apworld gets a full-width line under its field",
                      btn["y"] > f["y"] + f["height"] - 1 and btn["width"] == f["width"] and btn["height"] >= 40, (f, btn))
            ctx.close()

        # --- the signup wizard's game list: one searchable list, and a
        # starting YAML for the game picked
        ctx = web.context(b, srv, root)
        pg = ctx.new_page()
        if wait_for(lambda: "<span>ChecksFinder" in get(root, f"/s/{sid}/signup").text, timeout=240, every=3):
            settled(pg, f"/s/{sid}/signup")
            pg.fill("#gamefind", "checksf")
            shown = pg.evaluate("[...document.querySelectorAll('#gamelist li')].filter(li => li.getClientRects().length).map(li => li.dataset.kind + ':' + li.dataset.game)")
            check("searching narrows the game list (the upload entry stays)", shown == ["core:ChecksFinder", "upload:"], shown)
            check("Next waits for a game", pg.is_disabled("#tostep2"))
            pg.click("#gamelist li[data-game='ChecksFinder']")
            check("picking a core game lets you go on", not pg.is_disabled("#tostep2"))
            pg.click("#tostep2")
            check("step 2 offers that game's starting YAML", pg.is_visible("#tpllink") and "ChecksFinder" in (pg.text_content("#tpllink") or ""))
            with pg.expect_download(timeout=240_000) as dl:
                pg.click("#tpllink")
            path = dl.value.path()
            text = open(path, encoding="utf-8-sig").read() if path else ""
            check("…and downloads Archipelago's template for it",
                  dl.value.suggested_filename == "ChecksFinder.yaml" and "game: ChecksFinder" in text and "ChecksFinder:" in text,
                  (dl.value.suggested_filename, text[:200]))
        else:
            check("the signup wizard lists the core games within 4 minutes", False)
        ctx.close()

        # --- signing out and in: the next page never shows the old identity
        ctx = web.context(b, srv, root)
        pg = ctx.new_page()
        settled(pg, "/admin/seasons")
        held = web.HeldMe(pg)
        with pg.expect_navigation():
            pg.click("#signout")
        held.wait_held()
        check("after signing out, the first paint doesn't show the old name",
              "root" not in (pg.text_content("#who") or ""), pg.text_content("#who"))
        held.release()
        check("…and then offers to sign in", (pg.text_content("#who") or "").strip() == "sign in",
              pg.text_content("#who"))
        pg.goto(srv.base + "/login")
        held.release()
        pg.fill("input[name=username]", "root")
        pg.fill("input[name=password]", "correct horse battery")
        with pg.expect_navigation():
            pg.click("form[action='/login'] button[type=submit]")
        held.wait_held()
        check("after signing in, the first paint doesn't offer to sign in",
              "sign in" not in (pg.text_content("#who") or ""), pg.text_content("#who"))
        held.release()
        check("…and then shows who's signed in", "signed in as root" in (pg.text_content("#who") or ""),
              pg.text_content("#who"))
        held.stop()
        ctx.close()

    errors = web.take_errors()
    check("no script errors, console errors or failing requests on any page", not errors, errors)
finally:
    srv.stop()

sys.exit(summary())
