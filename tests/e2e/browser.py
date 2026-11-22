"""Pages as a real browser shows them (headless Chromium; see web.py): no
script errors, a header that lines up and doesn't shift or show a stale
identity, no sideways scrolling on a phone, tabs that keep the page's
address, and the signup wizard's buttons beside their fields.

Each check here stands for a bug a browser found that HTTP checks and the
fake-DOM node tests didn't. Add to it when a page script or layout breaks."""
import re
import sys

from lib import Server, check, get, new_season, post, summary
import web

srv = Server("browser", port=18195, ap_ports="39181-39183")
srv.start()
try:
    root = srv.setup_sysadmin()
    post(root, "/admin/settings", data=dict(domain="ap.example.org", home_url="https://home.example.org/", discords=""))
    sid = new_season(root, "Spring")
    post(root, f"/s/{sid}/admin/signups", data=dict(open="1"))
    # Pages a signed-in sysadmin sees, of each kind: the landing page (its own
    # template), the console, a season's console (tabs) and its signup wizard.
    pages = ["/", "/signups", "/admin/seasons", "/admin/users", "/admin/settings",
             "/admin/catalog", f"/s/{sid}/admin", f"/s/{sid}/signup"]
    for p in pages:
        check(f"{p} loads", get(root, p).status_code == 200)

    with web.browser() as b:
        # --- the header, on every page
        ctx = web.context(b, srv, root)
        pg = ctx.new_page()
        pg.goto(srv.base + "/admin/seasons")
        pg.wait_for_timeout(500)
        for p in pages:
            pg.goto(srv.base + p)
            pg.wait_for_timeout(400)
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
            pg.wait_for_timeout(300)
            first = web.header(pg)
            held.release()
            check(f"{p}: the header doesn't change once /api/me answers", first == web.header(pg),
                  (first, web.header(pg)))
        held.stop()
        ctx.close()

        # --- a phone's width: nothing scrolls sideways
        ctx = web.context(b, srv, root, width=360)
        pg = ctx.new_page()
        for p in pages:
            pg.goto(srv.base + p)
            pg.wait_for_timeout(400)
            check(f"{p}: no sideways scroll at 360 px", web.overflow_x(pg) <= 0, web.overflow_x(pg))
        ctx.close()

        # --- tabs keep the page's address (season pages have <base href>)
        ctx = web.context(b, srv, root)
        pg = ctx.new_page()
        pg.goto(srv.base + f"/s/{sid}/admin")
        pg.click(".gtab[data-tab='tab-options']")
        check("a tab adds its name to the page's address",
              pg.url == f"{srv.base}/s/{sid}/admin#tab-options", pg.url)
        pg.reload()
        pg.wait_for_timeout(300)
        check("…and reopens on reload", pg.is_visible("#tab-options") and not pg.is_visible("#tab-overview"))
        ctx.close()

        # --- the signup wizard: each button beside the field it acts on
        for width, beside in ((1100, True), (390, False)):
            ctx = web.context(b, srv, root, width=width)
            pg = ctx.new_page()
            pg.goto(srv.base + f"/s/{sid}/signup")
            pg.check("input[name=gtype][value=custom]")
            f, btn = web.box(pg, "#apfile"), web.box(pg, "#apbtn")
            if beside:
                check("signup: Upload apworld sits beside its field, as tall as it",
                      btn["y"] == f["y"] and btn["height"] == f["height"] and btn["x"] > f["x"], (f, btn))
            else:
                check("signup: on a phone, Upload apworld gets a full-width line under its field",
                      btn["y"] > f["y"] + f["height"] - 1 and btn["width"] == f["width"] and btn["height"] >= 40, (f, btn))
            ctx.close()

        # --- signing out and in: the next page never shows the old identity
        ctx = web.context(b, srv, root)
        pg = ctx.new_page()
        pg.goto(srv.base + "/admin/seasons")
        pg.wait_for_timeout(500)
        held = web.HeldMe(pg)
        pg.click("#signout")
        pg.wait_for_load_state("domcontentloaded")
        pg.wait_for_timeout(300)
        check("after signing out, the first paint doesn't show the old name",
              "root" not in (pg.text_content("#who") or ""), pg.text_content("#who"))
        held.release()
        check("…and then offers to sign in", (pg.text_content("#who") or "").strip() == "sign in")
        pg.goto(srv.base + "/login")
        held.release()
        pg.fill("input[name=username]", "root")
        pg.fill("input[name=password]", "correct horse battery")
        pg.click("form[action='/login'] button[type=submit]")
        pg.wait_for_load_state("domcontentloaded")
        pg.wait_for_timeout(300)
        check("after signing in, the first paint doesn't offer to sign in",
              "sign in" not in (pg.text_content("#who") or ""), pg.text_content("#who"))
        held.release()
        check("…and then shows who's signed in", "signed in as root" in (pg.text_content("#who") or ""))
        held.stop()
        ctx.close()

    check("no script errors on any page", not web.ERRORS, web.ERRORS)
finally:
    srv.stop()

sys.exit(summary())
