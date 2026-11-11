// The tab bar script (src/render/tabs.js): switching tabs, keeping the open
// tab in the URL hash, and carrying it on forms and on a panel's links back to
// the same page, so a log's pager and filter land back on the log's tab.
// Run with `node --test tests/js/*.test.mjs`.
//
// A browser isn't needed: the script uses only a few DOM calls, which the
// fake page below provides.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const SRC = readFileSync(new URL('../../src/render/tabs.js', import.meta.url), 'utf8');
const PAGE = 'https://bifrost.example/s/abc/season';

function classList(names) {
  const set = new Set(names);
  return {
    contains: (c) => set.has(c),
    toggle: (c, on) => (on ? set.add(c) : set.delete(c)),
    has: (c) => set.has(c),
  };
}

// A page with a tab bar over `panels` ({id: [link hrefs]}), the first one
// open, and one GET form in the last panel. `hash` is the URL's fragment.
function load(panels, hash = '') {
  const ids = Object.keys(panels);
  const listeners = [];
  const buttons = ids.map((id, i) => ({
    tab: id,
    classList: classList(i === 0 ? ['gtab', 'on'] : ['gtab']),
    getAttribute: (a) => (a === 'data-tab' ? id : null),
    closest: function (q) {
      return q === '.gtab' ? this : null;
    },
  }));
  const bar = {
    querySelector: (q) => buttons.find((b) => q === `.gtab[data-tab="${b.tab}"]`) || null,
    querySelectorAll: (q) => (assert.equal(q, '.gtab'), buttons),
    addEventListener: (type, f) => (assert.equal(type, 'click'), listeners.push(f)),
  };
  const panelEls = ids.map((id, i) => ({ id, hidden: i !== 0, classList: classList(['gtabpanel']) }));
  const links = [];
  panelEls.forEach((p) => {
    for (const href of panels[p.id]) {
      const link = {
        raw: href,
        href: new URL(href, PAGE).href,
        getAttribute: (a) => (a === 'href' ? href : null),
        closest: (q) => (q === '.gtabpanel' ? p : null),
      };
      links.push(link);
    }
  });
  let action = 'season';
  const form = {
    getAttribute: (a) => (a === 'action' ? action : null),
    setAttribute: (a, v) => {
      assert.equal(a, 'action');
      action = v;
    },
  };
  const url = new URL(PAGE);
  url.hash = hash;
  const page = {
    buttons,
    panels: panelEls,
    links,
    action: () => action,
    replaced: [],
    click: (id) => listeners.forEach((f) => f({ target: buttons.find((b) => b.tab === id) })),
    open: () => panelEls.filter((p) => !p.hidden).map((p) => p.id),
    on: () => buttons.filter((b) => b.classList.has('on')).map((b) => b.tab),
  };
  const document = {
    querySelector: (q) => (q === '.gtabs' ? bar : null),
    querySelectorAll: (q) =>
      ({ '.gtabpanel': panelEls, '.gtabpanel a[href]': links, form: [form] })[q] ??
      assert.fail(`unexpected selector ${q}`),
    getElementById: (id) => panelEls.find((p) => p.id === id) || null,
  };
  const ctx = {
    document,
    location: { origin: url.origin, pathname: url.pathname, hash: url.hash },
    history: { replaceState: (_s, _t, u) => page.replaced.push(u) },
    URL,
  };
  vm.createContext(ctx);
  vm.runInContext(SRC, ctx);
  return page;
}

const SEASON = {
  'tab-standings': [],
  'tab-mine': [],
  'tab-log': ['season?page=2&rpp=50', 'season?sender=Ann&page=3&rpp=50', 'season', '#top', 'game/3', '/'],
};

test('the first tab is open until another is clicked', () => {
  const p = load(SEASON);
  assert.deepEqual(p.open(), ['tab-standings']);
  p.click('tab-log');
  assert.deepEqual(p.open(), ['tab-log']);
  assert.deepEqual(p.on(), ['tab-log']);
  assert.deepEqual(p.replaced, ['#tab-log']);
});

test('the hash opens its tab without adding to the history', () => {
  const p = load(SEASON, '#tab-log');
  assert.deepEqual(p.open(), ['tab-log']);
  assert.deepEqual(p.on(), ['tab-log']);
  assert.deepEqual(p.replaced, []);
});

test("a hash that isn't a panel leaves the first tab open", () => {
  for (const hash of ['#nope', '#tab-hints', '']) {
    assert.deepEqual(load(SEASON, hash).open(), ['tab-standings'], hash);
  }
});

test('forms carry the open tab', () => {
  const p = load(SEASON);
  assert.equal(p.action(), 'season');
  p.click('tab-log');
  assert.equal(p.action(), 'season#tab-log');
  p.click('tab-mine');
  assert.equal(p.action(), 'season#tab-mine');
});

test("a panel's links back to this page carry its tab; others are left alone", () => {
  const p = load(SEASON);
  assert.deepEqual(
    p.links.map((l) => l.href),
    [
      `${PAGE}?page=2&rpp=50#tab-log`,
      `${PAGE}?sender=Ann&page=3&rpp=50#tab-log`,
      `${PAGE}#tab-log`,
      `${PAGE}#top`,
      'https://bifrost.example/s/abc/game/3',
      'https://bifrost.example/',
    ],
  );
});

test('a link that already names a tab keeps it', () => {
  const p = load({ 'tab-a': ['season#tab-b'], 'tab-b': [] });
  assert.equal(p.links[0].href, `${PAGE}#tab-b`);
});

test('a page without a tab bar is left alone', () => {
  const ctx = {
    document: { querySelector: () => null, querySelectorAll: () => assert.fail('looked further') },
  };
  vm.createContext(ctx);
  vm.runInContext(SRC, ctx);
});
