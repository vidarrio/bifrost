"""Real-browser helpers for end-to-end suites: headless Chromium through
Playwright, driving a Bifrost that lib.Server started.

Page scripts and layout break in ways a fake DOM or plain HTTP can't see (how a
browser resolves URLs against <base>, what paints before a fetch answers,
where flexbox puts things), so these check the pages as a browser shows them.

Needs `pip install -r tests/e2e/requirements.txt` and a Chromium for
Playwright: `python -m playwright install chromium-headless-shell` (CI does
both). An already-downloaded headless shell under ~/.cache/ms-playwright is
used when Playwright's own build of it is missing; BIFROST_E2E_CHROMIUM names
an executable outright.
"""
import contextlib
import glob
import os
import re
from urllib.parse import urlsplit

try:
    from playwright.sync_api import sync_playwright
except ImportError:  # pragma: no cover - a clear message beats a traceback
    raise SystemExit(
        "this suite needs Playwright: pip install -r tests/e2e/requirements.txt, "
        "then python -m playwright install chromium-headless-shell"
    )


def _launch(p):
    exe = os.environ.get("BIFROST_E2E_CHROMIUM")
    if exe:
        return p.chromium.launch(executable_path=exe)
    try:
        return p.chromium.launch()
    except Exception as e:
        # The newest revision Playwright has downloaded before (the folder is
        # chromium_headless_shell-<revision>).
        def revision(path):
            m = re.search(r"chromium_headless_shell-(\d+)", path)
            return int(m.group(1)) if m else -1
        found = sorted(glob.glob(os.path.expanduser(
            "~/.cache/ms-playwright/chromium_headless_shell-*/*/chrome-headless-shell")), key=revision)
        if not found:
            raise SystemExit("no Chromium for Playwright; run: python -m playwright "
                             f"install chromium-headless-shell\n(launching its own failed: {e})")
        return p.chromium.launch(executable_path=found[-1])


@contextlib.contextmanager
def browser():
    """A headless Chromium, closed afterwards."""
    with sync_playwright() as p:
        b = _launch(p)
        try:
            yield b
        finally:
            b.close()


# Problems on any page of any context, as "<url>: <what>": uncaught script
# errors, console errors (a blocked or failing script logs one), and error
# answers to the page's own requests, other than those in ALLOWED.
ERRORS = []

# Error answers that are expected: (method, path, status). A signed-out
# header's /api/me gets 401 by design.
ALLOWED = {("GET", "/api/me", 401)}


def context(b, srv, session=None, width=1100, height=800):
    """A browser context on `srv`, signed in as `session` (a lib session)
    when given. Uncaught script errors on its pages land in `ERRORS`."""
    ctx = b.new_context(viewport={"width": width, "height": height})
    if session is not None:
        ctx.add_cookies([{"name": c.name, "value": c.value, "url": srv.base}
                         for c in session.cookies])
    ctx.on("page", lambda pg: _watch(pg, srv.base))
    return ctx


def _watch(pg, base):
    pg.on("pageerror", lambda e: ERRORS.append(f"{pg.url}: {e}"))
    # The browser logs a failed request as a console error too; the response
    # handler below judges those, with ALLOWED, so they're left to it.
    pg.on("console", lambda m: m.type == "error" and not m.text.startswith("Failed to load resource")
          and ERRORS.append(f"{pg.url}: console: {m.text}"))

    def answered(r):
        u = urlsplit(r.url)
        if (r.url.startswith(base) and r.status >= 400
                and (r.request.method, u.path, r.status) not in ALLOWED):
            ERRORS.append(f"{pg.url}: {r.request.method} {u.path} -> {r.status}")
    pg.on("response", answered)


class HeldMe:
    """Holds back the page's /api/me requests, so a check can see the header
    as it first paints (from the cached identity) before the answer arrives."""

    def __init__(self, page):
        self.page, self.held = page, []
        self._hold = lambda route: self.held.append(route)
        page.route("**/api/me", self._hold)

    def wait_held(self, timeout_ms=5000):
        """Wait until the page has asked for /api/me (and been held)."""
        waited = 0
        while not self.held:
            if waited >= timeout_ms:
                raise AssertionError(f"{self.page.url} never asked for /api/me")
            self.page.wait_for_timeout(50)
            waited += 50

    def release(self):
        """Let the held requests through, and wait until the page has its
        answer and has drawn it. Fails if nothing was held: a check that
        compares the first paint with the header after /api/me answers would
        otherwise compare the first paint with itself."""
        self.wait_held()
        with self.page.expect_response("**/api/me"):
            for r in self.held:
                r.continue_()
        self.held.clear()
        # The answer's handler draws the header; let it run.
        self.page.evaluate("() => new Promise(r => setTimeout(r, 0))")

    def stop(self):
        if self.held:
            self.release()
        self.page.unroute("**/api/me", self._hold)


# What the header shows: each item's text, left edge and vertical centre.
HEADER = """() => [...document.querySelectorAll('header .hwrap *')]
  .filter(e => e.matches('.brand, .hbrand, .who, .pill'))
  .map(e => { const r = e.getBoundingClientRect();
    return {cls: e.className, text: e.textContent.trim(),
            left: Math.round(r.left), mid: (r.top + r.height / 2)}; })"""


def header(page):
    return page.evaluate(HEADER)


def overflow_x(page):
    """How far the page is wider than the viewport, in CSS pixels."""
    return page.evaluate("document.documentElement.scrollWidth - innerWidth")


def box(page, selector):
    """The element's box, rounded: {x, y, width, height}, or None when it
    isn't on the page or isn't shown."""
    loc = page.locator(selector)
    if loc.count() == 0:
        return None
    b = loc.first.bounding_box()
    return None if b is None else {k: round(v) for k, v in b.items()}
