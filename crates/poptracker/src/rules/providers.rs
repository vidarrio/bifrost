//! Counting the items (and custom items) that provide a code.

use crate::engine::{E, MAX_CUSTOM_CALLS, MAX_LOOKUP_DEPTH};
use crate::rules::run_rule_function;
use crate::sandbox::{is_memory_error, note_error};
use mlua::{Lua, Table, Value};

pub(crate) fn provider_count(lua: &Lua, eng: &E, code: &str) -> i64 {
    // JSON items' codes are case-insensitive; custom items get the code as written.
    let lower = code.to_lowercase();
    let mut total: i64 = {
        let e = eng.borrow();
        e.code_items.get(&lower).map_or(0, |list| {
            list.iter().map(|i| e.items[*i].provides(&lower)).sum()
        })
    };
    // Custom items count too, as their own functions say. They may ask in turn.
    let Ok(custom) = lua.globals().get::<Table>("__LUAITEMS") else {
        return total;
    };
    let budget = eng.borrow().budget.clone();
    if custom.raw_len() == 0 || eng.borrow().provider_depth >= MAX_LOOKUP_DEPTH {
        return total;
    }
    eng.borrow_mut().provider_depth += 1;
    for item in custom.sequence_values::<Table>().flatten() {
        // Out of budget, a pack can't go on, and its functions could fan out.
        if budget.get() == 0 {
            break;
        }
        if !custom_can_provide(eng, &item, code) {
            continue;
        }
        let Ok(Value::Function(gives)) = item.raw_get::<Value>("ProvidesCodeFunc") else {
            continue;
        };
        if !count_custom_call(eng) {
            break;
        }
        match gives.call::<Value>((&item, code)) {
            Ok(Value::Boolean(b)) => total += b as i64,
            Ok(n) => {
                let n = lua.coerce_number(n).ok().flatten().unwrap_or(0.0);
                total = total.saturating_add(n as i32 as i64);
            }
            Err(e) if is_memory_error(&e) => {
                note_error(eng, &e.to_string());
                budget.set(0);
            }
            Err(_) => {}
        }
    }
    eng.borrow_mut().provider_depth -= 1;
    total
}

/// Count a call to a custom item's function, false once there have been too many
/// or the budget's gone. A function may be a host function, which runs no Lua
/// instructions, so the budget alone wouldn't notice them.
pub(crate) fn count_custom_call(eng: &E) -> bool {
    let mut e = eng.borrow_mut();
    e.custom_calls += 1;
    if e.custom_calls > MAX_CUSTOM_CALLS {
        e.budget.set(0);
    }
    e.budget.get() != 0
}

/// PopTracker's `LuaItem::canProvideCode`: its `PotentialCodes` if it has them, else
/// its `CanProvideCodeFunc`. One that fails accepts nothing, and running out of
/// memory stops the operation's rules, as it does a rule function.
pub(crate) fn custom_can_provide(eng: &E, item: &Table, code: &str) -> bool {
    if let Ok(Value::Table(list)) = item.raw_get::<Value>("PotentialCodes") {
        // Scanning it is work too, whoever asks.
        eng.borrow_mut().custom_calls += list.raw_len() as u64;
        if !count_custom_call(eng) {
            return false;
        }
        return list
            .sequence_values::<String>()
            .flatten()
            .any(|c| c == code);
    }
    let Ok(Value::Function(can)) = item.raw_get::<Value>("CanProvideCodeFunc") else {
        return false;
    };
    if !count_custom_call(eng) {
        return false;
    }
    match can.call::<Value>((item, code)) {
        Ok(v) => !matches!(v, Value::Nil | Value::Boolean(false)),
        Err(e) => {
            if is_memory_error(&e) {
                note_error(eng, &e.to_string());
                eng.borrow().budget.set(0);
            }
            false
        }
    }
}

/// A bare numeric code in a rule (chiefly `visibility_rules`) is an AP location
/// id: the pack treats it as "present" exactly when that location exists for the
/// slot (its `onClear` sets a toggle of that code from `MissingLocations ∪
/// CheckedLocations`). We resolve it directly against the fed valid-location set.
/// Returns false when no set was fed (so visibility stays disabled) or the code
/// isn't a plain integer.
pub(super) fn location_present(eng: &E, code: &str) -> bool {
    let e = eng.borrow();
    let Some(valid) = e.valid_locations.as_ref() else {
        return false;
    };
    code.parse::<i64>().is_ok_and(|id| valid.contains(&id))
}

/// PopTracker's `ProviderCountForCode`: `$` codes run Lua, others count items.
/// While settling, results are kept, except a function's that looked at a
/// location, which may change as the rules settle. Otherwise a pack may have
/// just changed what it's asking about.
pub(crate) fn provider_count_cached(lua: &Lua, eng: &E, code: &str) -> i64 {
    let settling = eng.borrow().updating_cache;
    if settling && let Some(n) = eng.borrow().provider_cache.get(code) {
        return *n;
    }
    let n = if code.starts_with('$') {
        eng.borrow_mut().lua_codes.push(code.to_string());
        let n = run_rule_function(lua, eng, code);
        eng.borrow_mut().lua_codes.pop();
        n
    } else {
        provider_count(lua, eng, code)
    };
    let mut e = eng.borrow_mut();
    if settling && !e.indirect_codes.contains(code) {
        e.provider_cache.insert(code.to_string(), n);
    }
    n
}
