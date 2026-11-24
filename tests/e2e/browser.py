"""Pages as a real browser shows them (headless Chromium; see web.py): no
script or console errors and no failing requests, a header that lines up and
doesn't shift or show a stale identity, no sideways scrolling on a phone, tabs
that keep the page's address, and the signup wizard's buttons beside their
fields.

Each check here stands for a bug a browser found that HTTP checks and the
fake-DOM node tests didn't. Add to it when a page script or layout breaks."""
import re
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

        # --- the mailbox: glows while anything's unread, opens a box of
        # notifications (links only to this site), and reading clears the glow
        with srv.db() as c:
            acct = c.execute("SELECT id FROM account WHERE username = 'root'").fetchone()[0]
            c.execute("INSERT INTO notification (account_id, season_id, text, link, created_at) VALUES (?, ?, ?, ?, strftime('%s','now'))",
                      (acct, sid, "Your signup <b>X</b> needs attention.", f"/s/{sid}/signup"))
            c.execute("INSERT INTO notification (account_id, text, link, created_at) VALUES (?, ?, ?, strftime('%s','now'))",
                      (acct, "Elsewhere", "https://evil.example/"))
            for bent in ("/\\evil.example/", "/\t/evil.example/", "//evil.example/"):
                c.execute("INSERT INTO notification (account_id, text, link, created_at) VALUES (?, ?, ?, strftime('%s','now'))",
                          (acct, "Bent " + repr(bent), bent))
        for w, h in ((1100, 800), (390, 844)):
            ctx = web.context(b, srv, root, width=w, height=h)
            pg = ctx.new_page()
            settled(pg, "/")
            if w == 1100:
                check("the mailbox glows with unread mail", "glow" in (pg.get_attribute("#mailbtn", "class") or ""))
            def lined_up(where):
                mb, pill = web.box(pg, "#mailbtn"), web.box(pg, "#adminslot .pill")
                check(f"the mail button lines up with the pills ({where})",
                      mb and pill and mb["height"] == pill["height"] and mb["y"] == pill["y"], (mb, pill))
            lined_up(f"landing page, {w}px")
            pg.click("#mailbtn")
            pg.wait_for_selector("#mailbox li")
            bx = web.box(pg, "#mailbox")
            check(f"the mailbox fits a {w}px screen", bx and bx["x"] >= 0 and bx["x"] + bx["width"] <= w, bx)
            texts = pg.eval_on_selector_all("#mailbox li", "els => els.map(e => [e.className, e.querySelector('a') ? e.querySelector('a').getAttribute('href') : null, e.querySelector('.txt').textContent])")
            if w == 1100:
                check("…opening it lists them, newest first, as text", texts[3][2] == "Elsewhere" and texts[4][2] == "Your signup <b>X</b> needs attention.", texts)
                check("…linking only to this site", texts[4][1] == f"/s/{sid}/signup" and all(t[1] is None for t in texts[:4]), texts)
                try:
                    pg.wait_for_function("!document.getElementById('mailbtn').classList.contains('glow')", timeout=10_000)
                    cleared = True
                except Exception:
                    cleared = False
                check("…and reading them stops the glow", cleared)
                # The whole entry is the link: pressing beside its words goes there too.
                with pg.expect_navigation():
                    pg.click("#mailbox li:nth-child(5) .when")
                check("…and pressing one takes you to its page", pg.url.endswith(f"/s/{sid}/signup"), pg.url)
                settled(pg, "/worlds")
                check("…on the next page too", "glow" not in (pg.get_attribute("#mailbtn", "class") or ""))
                pg.click("#mailbtn")
                pg.wait_for_selector("#mailbox li")
            pg.keyboard.press("Escape")
            check(f"…and Escape closes it ({w}px)", pg.query_selector("#mailbox") is None)
            check(f"no page errors around the mailbox ({w}px)", not web.take_errors(), web.ERRORS)
            # The other pages' copy of the rules, and the touch-screen one.
            settled(pg, "/worlds")
            lined_up(f"rendered page, {w}px")
            ctx.close()
        tctx = b.new_context(viewport={"width": 390, "height": 844}, has_touch=True, is_mobile=True)
        signed_in = web.context(b, srv, root)
        tctx.add_cookies(signed_in.cookies())
        signed_in.close()
        tpg = tctx.new_page()
        for path in ("/", "/worlds"):
            with tpg.expect_event("requestfinished", lambda r: r.url.endswith("/api/me")):
                tpg.goto(srv.base + path)
            tpg.wait_for_selector("#mailbtn")
            mb, pill = web.box(tpg, "#mailbtn"), web.box(tpg, "#adminslot .pill")
            check(f"the mail button lines up with the pills on a touch screen ({path})",
                  mb and pill and mb["height"] == pill["height"] == 27 and mb["y"] == pill["y"], (mb, pill))
        tctx.close()

        # --- the moderator log: numbered pages, each sized to fit the window
        with srv.db() as c:
            c.executemany("INSERT INTO mod_action (at, username, season_name, action, detail, status, outcome) VALUES (?, 'mia', ?, '/admin/generate', ?, 200, ?)",
                          [(1_000_000 + i, "A season with a very long name indeed, all eighty characters of it" if i % 5 == 0 else "Spring",
                            f"row {i}" + (" long" * 120 if i % 3 == 0 else ""), "unfinished" if i % 7 == 0 else "ok") for i in range(300)])
        per_of = lambda pg: int(re.search(r"rpp=(\d+)", pg.url).group(1))
        def settled_log(pg):
            """Wait for the fit to land, then for nothing more to happen."""
            pg.wait_for_url(lambda u: "fit=" in u, timeout=10_000)
            web.settle(pg)
            url = pg.url
            pg.wait_for_timeout(1500)
            return pg.url == url
        def fits(pg):
            return pg.evaluate("""(() => { const r = [...document.querySelectorAll('#modlog tr')].filter(t => t.querySelector('td'));
                return {rows: r.length, last: r[r.length-1].getBoundingClientRect().bottom,
                        doc: document.documentElement.scrollHeight, h: innerHeight}; })()""")
        for w, h in ((1100, 800), (390, 844)):
            ctx = web.context(b, srv, root, width=w, height=h)
            pg = ctx.new_page()
            pg.goto(srv.base + "/admin/audit?user=mia")
            check(f"a log page settles on its size ({w}x{h})", settled_log(pg), pg.url)
            f = fits(pg)
            check(f"…and fits the window, long details and all ({w}x{h})", f["rows"] >= 1 and f["last"] <= f["h"] and f["doc"] <= f["h"], f)
            cur = pg.text_content(".pager .pg.cur") or ""
            check(f"…with the activity log's pager, on page 1 ({w}px)", cur == "1" and pg.query_selector(".pager a.pgnav[title=older]") is not None
                  and pg.query_selector(".pager span.pg.pgnav.disabled") is not None, cur)
            check(f"…and nothing wider than the screen ({w}px)", not web.overflow_x(pg), web.overflow_x(pg))
            mid = pg.evaluate("""(() => { const t = document.getElementById('modlog').getBoundingClientRect();
                const b = [...document.querySelectorAll('.pager .pg')].map(e => e.getBoundingClientRect());
                return [Math.round((t.left + t.right) / 2), Math.round((Math.min(...b.map(r => r.left)) + Math.max(...b.map(r => r.right))) / 2)]; })()""")
            check(f"…its pager centred under the rows ({w}px)", abs(mid[0] - mid[1]) <= 2, mid)
            if w == 390:
                hs = pg.evaluate("[...document.querySelectorAll('#modlog tr[tabindex]')].map(r => Math.round(r.getBoundingClientRect().height))")
                check("…phone rows are two lines, the same height", len(set(hs)) == 1 and hs[0] < 60, hs)
            if w == 1100:
                per = per_of(pg)
                pg.click(".pager a.pg:text-is('2')")
                web.settle(pg)
                first = pg.text_content("#modlog tr:nth-child(2) td:nth-child(5)") or ""
                check("…page 2 goes on where page 1 ended, with the same size",
                      f"rpp={per}" in pg.url and first.startswith(f"row {299 - per}"), (pg.url, first[:30]))
                pg.set_viewport_size({"width": 1100, "height": 620})
                pg.wait_for_url(lambda u: "x620" in u, timeout=10_000)
                web.settle(pg)
                smaller = per_of(pg)
                f = fits(pg)
                check("…a smaller window gets fewer rows a page, still fitting", smaller < per and f["doc"] <= f["h"], (per, smaller, f))
                pg.go_back()
                pg.wait_for_url(lambda u: "x620" in u, timeout=10_000)
                web.settle(pg)
                f = fits(pg)
                check("…and going Back to a page fitted for a bigger window fits it again", f["doc"] <= f["h"], (pg.url, f))
                # A narrower window, at the same height, refits too (and here
                # crosses into the phone layout).
                pg.set_viewport_size({"width": 500, "height": 800})
                pg.wait_for_url(lambda u: "fit=500x800" in u, timeout=10_000)
                web.settle(pg)
                f = fits(pg)
                check("…as does a narrower one", f["doc"] <= f["h"] and not web.overflow_x(pg), (pg.url, f))
                pg.set_viewport_size({"width": 1100, "height": 800})
                pg.wait_for_url(lambda u: "fit=1100x800" in u, timeout=10_000)
                web.settle(pg)
                # Middle pages, whose pager takes more room, still fit.
                pg.click(".pager a.pg:not(.pgnav) >> nth=-1")
                web.settle(pg)
                for _ in range(4):
                    link = pg.query_selector(".pager a.pgnav[title=newer]")
                    if not link:
                        break
                    link.click()
                    settled_log(pg)
                f = fits(pg)
                check("…and a page further in fits as well", f["doc"] <= f["h"], (pg.url, f))
                # A row pressed shows all of itself.
                row = pg.query_selector("#modlog tr[tabindex]")
                row.click()
                check("pressing an entry shows all of it", row.get_attribute("aria-expanded") == "true", row.get_attribute("class"))
                url = pg.url
                pg.set_viewport_size({"width": 1100, "height": 760})
                pg.wait_for_timeout(1200)
                check("…and it stays open when the window's height changes", pg.url == url and row.get_attribute("aria-expanded") == "true", pg.url)
                cut = pg.evaluate("[...document.querySelectorAll('#modlog tr[tabindex]:not(.open) td:first-child')].some(td => td.scrollWidth > td.clientWidth)")
                check("…and no row's time is cut short", not cut)
            check(f"no page errors on the log ({w}px)", not web.take_errors(), web.ERRORS)
            ctx.close()
        # A single page has no pager, as in the activity log.
        ctx = web.context(b, srv, root, width=1100, height=800)
        pg = ctx.new_page()
        pg.goto(srv.base + "/admin/audit?user=root")
        web.settle(pg)
        check("one page of the log has no pager", pg.query_selector("#modlog") is not None and pg.query_selector(".pager") is None)
        ctx.close()
        # Storage blocked (a private window, a strict browser): it still settles.
        ctx = web.context(b, srv, root, width=1100, height=800)
        ctx.add_init_script("Object.defineProperty(window, 'sessionStorage', {get() { throw new DOMException('blocked', 'SecurityError'); }});"
                            "Object.defineProperty(window, 'localStorage', {get() { throw new DOMException('blocked', 'SecurityError'); }});")
        pg = ctx.new_page()
        navs = []
        pg.on("framenavigated", lambda fr: navs.append(fr.url) if fr == pg.main_frame else None)
        pg.goto(srv.base + "/admin/audit?user=mia")
        check("with storage blocked the log still settles, reloading at most once", settled_log(pg) and len(navs) <= 2, navs[:5])
        web.take_errors()
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
