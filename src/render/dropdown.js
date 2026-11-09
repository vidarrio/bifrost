// A filtered list of names under a text field, to pick one from. The page
// header includes this file, and tests/js/dropdown.test.mjs tests it, with
// the two pages' wiring at the end.
//
// bifDropdown(inp, box, list, o): inp is the field, box the element the matches
// go in, and list() returns the names to match. It's read on every keystroke,
// since the pool can change. o sets the behaviour:
//   words   every word typed must appear in a name (else the whole text must)
//   max     at most this many matches
//   cls     the class each match's element gets (plus `sel` when highlighted)
//   empty   an empty field lists the first names (else it lists nothing)
//   focus   open when the field gets focus, close when it loses it
//   first   typing highlights the first match (else nothing until ↓)
//   wrap    ↑/↓ wrap round the ends (else they stop there)
//   pick    the key that takes the highlighted match: 'Enter' or 'Tab'
// A click on a match takes it, and Escape closes the list. Without `focus`
// the list stays open when the field loses focus, so a click on it puts focus
// back in the field. With it, the click finds the field focused already (its
// mousedown is cancelled), and focusing it again would reopen the list. The list is the
// box's children, so CSS can hide an empty box with :empty.
// Returns { close }, for a form that clears the field itself.
//
// The highlighted match is scrolled into view.
//
// The two places that use it behave differently on purpose.

// The activity log's sender and receiver fields (bifAutocomplete, below): a short
// list of player names, so it opens when the field is focused, even empty, and
// nothing is highlighted until ↓. Enter takes the highlighted name, and
// without one it submits the filter.
var BIF_PLAYER_LIST = { max: 8, cls: 'aci', empty: true, focus: true, pick: 'Enter' };

// The game page's hint search (bifHintSearch, below): long item lists, so it matches
// every word typed, highlights the first match and wraps ↑/↓ round the ends.
// Tab takes the highlighted item, so Enter always sends the hint.
var BIF_ITEM_LIST = { words: true, max: 15, cls: 'match', first: true, wrap: true, pick: 'Tab' };

function bifDropdown(inp, box, list, o) {
 var hits = [], sel = -1;
 function close() { hits = []; sel = -1; box.textContent = ''; }
 function take(n) { inp.value = n; close(); if (!o.focus) inp.focus(); }
 function render() {
  box.textContent = '';
  hits.forEach(function (n, i) {
   var d = document.createElement('div');
   d.className = o.cls + (i === sel ? ' sel' : '');
   d.textContent = n;
   d.addEventListener('mousedown', function (e) { e.preventDefault(); take(n); });
   box.appendChild(d);
  });
  var s = box.querySelector('.sel');
  if (s) s.scrollIntoView({ block: 'nearest' });
 }
 function open() {
  var v = inp.value.trim().toLowerCase();
  var w = o.words ? v.split(/\s+/) : [v];
  hits = !v && !o.empty ? [] : list().filter(function (n) {
   var l = n.toLowerCase();
   return w.every(function (x) { return l.indexOf(x) >= 0; });
  }).slice(0, o.max);
  sel = o.first && hits.length ? 0 : -1;
  render();
 }
 inp.addEventListener('input', open);
 if (o.focus) {
  inp.addEventListener('focus', open);
  // Late enough that a click on a match lands first.
  inp.addEventListener('blur', function () { setTimeout(close, 150); });
 }
 inp.addEventListener('keydown', function (e) {
  var n = hits.length;
  if (!n) return;
  if (e.key === 'ArrowDown') {
   e.preventDefault();
   sel = o.wrap ? (sel + 1) % n : Math.min(sel + 1, n - 1);
   render();
  } else if (e.key === 'ArrowUp') {
   e.preventDefault();
   sel = o.wrap ? (sel - 1 + n) % n : Math.max(sel - 1, 0);
   render();
  } else if (e.key === o.pick && sel >= 0) {
   e.preventDefault();
   take(hits[sel]);
  } else if (e.key === 'Escape') {
   close();
  }
 });
 return { close: close };
}

// The activity log's filter: every `input[data-ac]` with an `.acbox` beside
// it lists the players in window.bifPlayers. The header runs it once the page
// has loaded.
function bifAutocomplete() {
 document.querySelectorAll('input[data-ac]').forEach(function (inp) {
  var box = inp.parentNode.querySelector('.acbox');
  if (!box) return;
  bifDropdown(inp, box, function () { return window.bifPlayers || []; }, BIF_PLAYER_LIST);
 });
}

// The game page's hint search (`HINT_JS`): #hintq lists the names in the
// page's ITEMS, which the page replaces after each hint, in #hintmatches.
function bifHintSearch() {
 return bifDropdown(document.getElementById('hintq'), document.getElementById('hintmatches'),
  function () { return ITEMS; }, BIF_ITEM_LIST);
}
