//! Settling every location's and section's level over repeated passes.

use crate::engine::E;
use crate::rules::{LEVEL_NONE, LEVEL_NORMAL, MAX_RULE_PASSES, Rules, resolve_rules};
use crate::sandbox::note_error;
use mlua::Lua;

/// Settle every location's and section's level, unless that's been done since the
/// last change. While settling, a look at a level gets what's known so far.
pub(crate) fn settle_accessibility(lua: &Lua, eng: &E) {
    {
        let mut e = eng.borrow_mut();
        if !e.accessibility_stale || e.updating_cache {
            return;
        }
        e.accessibility_stale = false;
        e.updating_cache = true;
        e.accessibility.clear();
        e.provider_cache.clear();
    }
    let nodes = eng.borrow().nodes.clone();
    let budget = eng.borrow().budget.clone();
    let mut settled = false;
    for _ in 0..MAX_RULE_PASSES {
        let mut changed = false;
        let mut update = |id: &str, rules: &Rules, glitched: bool| {
            let known = eng.borrow().accessibility.get(id).copied();
            if known == Some(LEVEL_NORMAL) {
                return;
            }
            let level = resolve_rules(lua, eng, rules, false, glitched);
            if known != Some(level) {
                eng.borrow_mut().accessibility.insert(id.to_string(), level);
                changed = true;
            }
        };
        for n in nodes.iter() {
            if budget.get() == 0 {
                break;
            }
            update(&n.name, &n.rules, n.scoutable_glitched);
            for s in &n.sections {
                update(
                    &format!("{}/{}", n.name, s.name),
                    &s.rules,
                    s.scoutable_glitched,
                );
            }
        }
        if !changed {
            settled = true;
            break;
        }
        if budget.get() == 0 {
            break;
        }
    }
    finish_settling(eng, settled, budget.get());
}

fn finish_settling(eng: &E, settled: bool, budget: u64) {
    {
        let mut e = eng.borrow_mut();
        e.updating_cache = false;
        e.indirect_codes.clear();
    }
    if !settled && budget > 0 {
        note_error(
            eng,
            "the pack's rules never settled, so some may read wrong",
        );
    }
}

/// As [`settle_accessibility`], for visibility. Empty rules mean visible, and
/// aren't kept.
pub(crate) fn settle_visibility(lua: &Lua, eng: &E) {
    {
        let mut e = eng.borrow_mut();
        if !e.visibility_stale || e.updating_cache {
            return;
        }
        e.visibility_stale = false;
        e.updating_cache = true;
        e.visibility.clear();
        e.provider_cache.clear();
    }
    let nodes = eng.borrow().nodes.clone();
    let budget = eng.borrow().budget.clone();
    let mut settled = false;
    for _ in 0..MAX_RULE_PASSES {
        let mut changed = false;
        let mut update = |id: &str, rules: &Rules| {
            if rules.is_empty() {
                return;
            }
            let known = eng.borrow().visibility.get(id).copied();
            if known == Some(true) {
                return;
            }
            let visible = resolve_rules(lua, eng, rules, true, false) != LEVEL_NONE;
            if known != Some(visible) {
                eng.borrow_mut().visibility.insert(id.to_string(), visible);
                changed = true;
            }
        };
        for n in nodes.iter() {
            if budget.get() == 0 {
                break;
            }
            update(&n.name, &n.vis);
            for s in &n.sections {
                update(&format!("{}/{}", n.name, s.name), &s.vis);
            }
        }
        if !changed {
            settled = true;
            break;
        }
        if budget.get() == 0 {
            break;
        }
    }
    finish_settling(eng, settled, budget.get());
}
