//! Access-rule evaluation, as PopTracker's `resolveRules` does it.

pub(crate) mod locations;
pub(crate) mod providers;
pub(crate) mod settle;

use crate::rules::locations::reference_level;
use crate::rules::providers::{count_custom_call, location_present, provider_count_cached};
use crate::sandbox::{is_memory_error, note_error};
use crate::session::engine::E;
use mlua::{Function, Lua, MultiValue, Value};

// ---- access-rule evaluation --------------------------------------------------

/// PopTracker's accessibility levels, with the numbers packs see as
/// `AccessibilityLevel`: a `^$` rule's function returns one.
pub(crate) type Level = i64;
pub(crate) const LEVEL_NONE: Level = 0;
pub(crate) const LEVEL_PARTIAL: Level = 1;
pub(crate) const LEVEL_INSPECT: Level = 3;
pub(crate) const LEVEL_SEQUENCE_BREAK: Level = 5;
pub(crate) const LEVEL_NORMAL: Level = 6;
pub(crate) const LEVEL_CLEARED: Level = 7;

pub(crate) fn level_name(l: Level) -> &'static str {
    match l {
        LEVEL_NONE => "none",
        LEVEL_INSPECT => "inspect",
        LEVEL_SEQUENCE_BREAK => "glitch",
        _ => "reachable",
    }
}

/// How many times the rules are gone through for references between locations
/// to settle. PopTracker goes on until they do; a pack can make them flip forever.
pub(crate) const MAX_RULE_PASSES: usize = 256;

/// How many alternatives one location's rules may join up to with its parents':
/// the count multiplies at each level, and memory for it is outside Lua's cap.
pub(crate) const MAX_ALTERNATIVES: usize = 8192;

/// A rule list: any of its alternatives (OR), each all of its codes (AND).
pub(crate) type Rules = Vec<Vec<String>>;

/// `atoi`: the leading number, or 0.
pub(crate) fn leading_int(s: &str) -> i64 {
    let s = s.trim_start();
    let (sign, digits) = match s.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, s.strip_prefix('+').unwrap_or(s)),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    digits[..end].parse::<i64>().map_or(0, |n| sign * n)
}

/// Run a rule's `$name|arg|…` function, as PopTracker reads its result: a number
/// (or a string that is one) truncated, `true` as 1, anything else or an error 0.
pub(crate) fn run_rule_function(lua: &Lua, eng: &E, code: &str) -> i64 {
    let mut parts = code.strip_prefix('$').unwrap_or(code).split('|');
    let name = parts.next().unwrap_or("");
    let Ok(func) = lua.globals().get::<Function>(name) else {
        return 0;
    };
    // A host function as a rule's runs no Lua instructions, so count the calls too.
    if !count_custom_call(eng) {
        return 0;
    }
    let args =
        MultiValue::from_iter(parts.filter_map(|a| lua.create_string(a).ok().map(Value::String)));
    match func.call::<Value>(args) {
        Ok(Value::Boolean(b)) => b as i64,
        Ok(v) => lua.coerce_number(v).ok().flatten().map_or(0, |n| n as i64),
        // Out of memory, the pack can't go on: stop settling, and say so.
        Err(e) if is_memory_error(&e) => {
            note_error(eng, &e.to_string());
            eng.borrow().budget.set(0);
            0
        }
        Err(_) => 0,
    }
}

/// PopTracker's `resolveRules`, kept line for line in its logic: packs are written
/// against its behaviour, quirks included.
pub(crate) fn resolve_rules(
    lua: &Lua,
    eng: &E,
    rules: &Rules,
    visibility: bool,
    scoutable_glitched: bool,
) -> Level {
    let (mut glitched, mut inspectable) = (false, false);
    if rules.is_empty() {
        return LEVEL_NORMAL;
    }
    for ruleset in rules {
        if ruleset.is_empty() {
            return LEVEL_NORMAL;
        }
        let mut reachable = LEVEL_NORMAL;
        let mut inspect_only = false;
        for rule in ruleset {
            if rule.is_empty() {
                continue;
            }
            let mut s = rule.as_str();
            // `{…}` needs this to look, not to get.
            if let Some(rest) = s.strip_prefix('{') {
                inspect_only = true;
                s = rest;
            }
            if inspect_only && let Some(rest) = s.strip_suffix('}') {
                s = rest;
            }
            // `[…]` is optional: without it, a sequence break.
            let mut optional = false;
            if s.len() > 1 && s.starts_with('[') && s.ends_with(']') {
                optional = true;
                s = &s[1..s.len() - 1];
            }
            if inspect_only && s.is_empty() {
                continue;
            }
            // `^$f` gives a level; `@…` refers to a location; `code:n` wants n.
            let is_level = s.starts_with('^');
            let is_reference = s.starts_with('@');
            let mut count = 1;
            if !is_level
                && !is_reference
                && let Some((code, n)) = s.split_once(':')
            {
                count = leading_int(n);
                s = code;
            }
            if is_level {
                if s.len() < 3 || !s[1..].starts_with('$') {
                    reachable = LEVEL_NONE;
                    break;
                }
                s = &s[1..];
            }
            if let Some(id) = s.strip_prefix('@') {
                if let Some(mut sub) = reference_level(eng, id, visibility) {
                    if !inspect_only && sub == LEVEL_INSPECT {
                        sub = LEVEL_NONE;
                    } else if optional && sub == LEVEL_NONE {
                        sub = LEVEL_SEQUENCE_BREAK;
                    } else if sub == LEVEL_NONE {
                        reachable = LEVEL_NONE;
                    }
                    if sub == LEVEL_SEQUENCE_BREAK && reachable != LEVEL_NONE {
                        reachable = LEVEL_SEQUENCE_BREAK;
                    }
                }
                if reachable == LEVEL_NONE {
                    break;
                }
            } else {
                let n = provider_count_cached(lua, eng, s);
                if is_level {
                    let mut sub = n;
                    if !inspect_only && sub == LEVEL_INSPECT {
                        inspect_only = true;
                    } else if optional && sub == LEVEL_NONE {
                        sub = LEVEL_SEQUENCE_BREAK;
                    } else if sub == LEVEL_NONE {
                        reachable = LEVEL_NONE;
                    }
                    if sub == LEVEL_SEQUENCE_BREAK && reachable != LEVEL_NONE {
                        reachable = LEVEL_SEQUENCE_BREAK;
                    }
                    if reachable == LEVEL_NONE {
                        break;
                    }
                    continue;
                }
                if n >= count || location_present(eng, s) {
                    continue;
                }
                if optional {
                    reachable = LEVEL_SEQUENCE_BREAK;
                } else {
                    reachable = LEVEL_NONE;
                    break;
                }
            }
        }
        if reachable == LEVEL_NORMAL && !inspect_only {
            return LEVEL_NORMAL;
        }
        if reachable != LEVEL_NONE && inspect_only {
            inspectable = true;
        }
        if reachable == LEVEL_SEQUENCE_BREAK {
            glitched = true;
        }
    }
    if glitched && inspectable && !scoutable_glitched {
        LEVEL_INSPECT
    } else if glitched {
        LEVEL_SEQUENCE_BREAK
    } else if inspectable {
        LEVEL_INSPECT
    } else {
        LEVEL_NONE
    }
}
