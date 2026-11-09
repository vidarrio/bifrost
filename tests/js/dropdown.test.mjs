// Keyboard and mouse behaviour of the shared dropdown (src/render/dropdown.js),
// with each preset its page uses. Run with `node --test tests/js/*.test.mjs`.
//
// A browser isn't needed: the dropdown uses only a few DOM calls, which the
// fake elements below provide.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const SRC = readFileSync(new URL('../../src/render/dropdown.js', import.meta.url), 'utf8');

class El {
  constructor() {
    this.children = [];
    this.className = '';
    this.text = '';
    this.value = '';
    this.listeners = {};
    this.focused = 0;
  }
  addEventListener(type, f) {
    (this.listeners[type] ||= []).push(f);
  }
  fire(type, props = {}) {
    const ev = { type, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...props };
    for (const f of this.listeners[type] || []) f(ev);
    return ev;
  }
  set textContent(v) {
    this.children = [];
    this.text = v;
  }
  get textContent() {
    return this.text;
  }
  appendChild(c) {
    this.children.push(c);
  }
  querySelector(q) {
    assert.equal(q, '.sel');
    return this.children.find((c) => c.className.split(' ').includes('sel')) || null;
  }
  scrollIntoView() {}
  focus() {
    this.focused++;
  }
}

// The dropdown.js globals in a fresh context, with timers run by hand.
function load() {
  const timers = [];
  const ctx = {
    document: { createElement: () => new El() },
    setTimeout: (f) => timers.push(f),
  };
  vm.createContext(ctx);
  vm.runInContext(SRC, ctx);
  ctx.runTimers = () => timers.splice(0).forEach((f) => f());
  return ctx;
}

// A field and its box wired up with one of the presets.
function setup(preset, names) {
  const ctx = load();
  const inp = new El();
  const box = new El();
  const pool = { names };
  const handle = ctx.bifDropdown(inp, box, () => pool.names, ctx[preset]);
  const shown = () => box.children.map((c) => c.textContent);
  const selected = () => box.children.findIndex((c) => c.className.split(' ').includes('sel'));
  const type = (v) => {
    inp.value = v;
    inp.fire('input');
  };
  const key = (k) => inp.fire('keydown', { key: k });
  return { ctx, inp, box, pool, handle, shown, selected, type, key };
}

const PLAYERS = ['Ann', 'Bob', 'Dana', 'Eve', 'Fay', 'Gus', 'Hal', 'Ida', 'Jo', 'Kai'];

test('player list: focus opens the first names, nothing highlighted', () => {
  const t = setup('BIF_PLAYER_LIST', PLAYERS);
  t.inp.fire('focus');
  assert.deepEqual(t.shown(), PLAYERS.slice(0, 8));
  assert.equal(t.selected(), -1);
  assert.ok(t.box.children.every((c) => c.className === 'aci'));
});

test('player list: matches the text anywhere in a name, ignoring case', () => {
  const t = setup('BIF_PLAYER_LIST', PLAYERS);
  t.type(' AN ');
  assert.deepEqual(t.shown(), ['Ann', 'Dana']);
  t.type('a n');
  assert.deepEqual(t.shown(), [], 'the whole text has to match, not each word');
});

test('player list: arrows stop at the ends, Enter takes the highlighted name', () => {
  const t = setup('BIF_PLAYER_LIST', PLAYERS);
  t.type('a');
  assert.deepEqual(t.shown(), ['Ann', 'Dana', 'Fay', 'Hal', 'Ida', 'Kai']);
  assert.ok(!t.key('Enter').defaultPrevented, 'with nothing highlighted, Enter submits');
  assert.ok(t.key('ArrowUp').defaultPrevented);
  assert.equal(t.selected(), 0, '↑ from nothing highlights the first');
  t.key('ArrowUp');
  assert.equal(t.selected(), 0);
  for (let i = 0; i < 9; i++) t.key('ArrowDown');
  assert.equal(t.selected(), 5, '↓ stops at the last');
  assert.ok(!t.key('Tab').defaultPrevented, 'Tab moves focus on as usual');
  assert.equal(t.selected(), 5);
  assert.ok(t.key('Enter').defaultPrevented, "Enter doesn't also submit");
  assert.equal(t.inp.value, 'Kai');
  assert.deepEqual(t.shown(), []);
});

test('player list: typing clears the highlight', () => {
  const t = setup('BIF_PLAYER_LIST', PLAYERS);
  t.type('a');
  t.key('ArrowDown');
  t.key('ArrowDown');
  t.type('an');
  assert.equal(t.selected(), -1);
});

test('player list: closes a moment after the field loses focus', () => {
  const t = setup('BIF_PLAYER_LIST', PLAYERS);
  t.inp.fire('focus');
  t.inp.fire('blur');
  assert.equal(t.shown().length, 8, 'still open, so a click on a name lands');
  t.ctx.runTimers();
  assert.deepEqual(t.shown(), []);
  assert.ok(!t.key('Enter').defaultPrevented, 'keys pass through once closed');
});

test('item list: an empty field lists nothing, and focus opens nothing', () => {
  const t = setup('BIF_ITEM_LIST', ['Sword', 'Shield']);
  t.inp.fire('focus');
  assert.deepEqual(t.shown(), []);
  t.type('   ');
  assert.deepEqual(t.shown(), []);
  t.inp.fire('blur');
  t.type('s');
  t.ctx.runTimers();
  assert.deepEqual(t.shown(), ['Sword', 'Shield'], "losing focus doesn't close it");
});

test('item list: every word has to appear, in any order', () => {
  const t = setup('BIF_ITEM_LIST', ['Iron Sword', 'Iron Shield', 'Sword of Fire', 'Fire Rod']);
  t.type('sw IRON');
  assert.deepEqual(t.shown(), ['Iron Sword']);
  t.type('fire');
  assert.deepEqual(t.shown(), ['Sword of Fire', 'Fire Rod']);
  assert.ok(t.box.children.every((c) => c.className.split(' ')[0] === 'match'));
});

test('item list: highlights the first match, arrows wrap, Tab takes it', () => {
  const t = setup('BIF_ITEM_LIST', ['Bow', 'Bomb', 'Boots']);
  t.type('bo');
  assert.equal(t.selected(), 0);
  assert.ok(t.key('ArrowUp').defaultPrevented);
  assert.equal(t.selected(), 2, '↑ from the first wraps to the last');
  t.key('ArrowDown');
  assert.equal(t.selected(), 0, '↓ from the last wraps to the first');
  t.key('ArrowDown');
  assert.ok(!t.key('Enter').defaultPrevented, 'Enter submits the hint form');
  assert.ok(t.key('Tab').defaultPrevented);
  assert.equal(t.inp.value, 'Bomb');
  assert.deepEqual(t.shown(), []);
  assert.ok(!t.key('Tab').defaultPrevented, 'once closed, Tab moves focus on');
});

test('item list: at most 15 matches, from the pool as it is now', () => {
  const many = Array.from({ length: 20 }, (_, i) => `Key ${i}`);
  const t = setup('BIF_ITEM_LIST', many);
  t.type('key');
  assert.deepEqual(t.shown(), many.slice(0, 15));
  t.pool.names = ['Key A'];
  t.type('key');
  assert.deepEqual(t.shown(), ['Key A']);
});

for (const preset of ['BIF_PLAYER_LIST', 'BIF_ITEM_LIST']) {
  test(`${preset}: a click takes a match and keeps focus in the field`, () => {
    const t = setup(preset, ['Alpha', 'Alpine']);
    t.type('alp');
    const ev = t.box.children[1].fire('mousedown');
    assert.ok(ev.defaultPrevented, "the field doesn't lose focus");
    assert.equal(t.inp.value, 'Alpine');
    assert.deepEqual(t.shown(), []);
    assert.equal(t.inp.focused, 1);
  });

  test(`${preset}: Escape closes, close() too`, () => {
    const t = setup(preset, ['Alpha', 'Alpine']);
    t.type('alp');
    t.key('Escape');
    assert.deepEqual(t.shown(), []);
    t.type('alp');
    t.handle.close();
    assert.deepEqual(t.shown(), []);
    assert.ok(!t.key('ArrowDown').defaultPrevented);
  });
}
