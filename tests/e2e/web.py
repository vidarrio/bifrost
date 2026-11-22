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
        found = sorted(glob.glob(os.path.expanduser(
            "~/.cache/ms-playwright/chromium_headless_shell-*/*/chrome-headless-shell")))
        if not found:
            raise SystemExit(f"no Chromium for Playwright ({e}); run: "
                             "python -m playwright install chromium-headless-shell")
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


# Uncaught script errors on any page of any context, as "<url>: <error>".
ERRORS = []


def context(b, srv, session=None, width=1100, height=800):
    """A browser context on `srv`, signed in as `session` (a lib session)
    when given. Uncaught script errors on its pages land in `ERRORS`."""
    ctx = b.new_context(viewport={"width": width, "height": height})
    if session is not None:
        ctx.add_cookies([{"name": c.name, "value": c.value, "url": srv.base}
                         for c in session.cookies])
    ctx.on("page", _watch)
    return ctx


def _watch(pg):
    pg.on("pageerror", lambda e: ERRORS.append(f"{pg.url}: {e}"))


class HeldMe:
    """Holds back the page's /api/me requests, so a check can see the header
    as it first paints (from the cached identity) before the answer arrives."""

    def __init__(self, page):
        self.page, self.held = page, []
        self._hold = lambda route: self.held.append(route)
        page.route("**/api/me", self._hold)

    def release(self, settle_ms=400):
        for r in self.held:
            r.continue_()
        self.held.clear()
        self.page.wait_for_timeout(settle_ms)

    def stop(self):
        self.release(0)
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
    """The element's box, rounded: {x, y, width, height}, or None."""
    b = page.locator(selector).first.bounding_box()
    return None if b is None else {k: round(v) for k, v in b.items()}
