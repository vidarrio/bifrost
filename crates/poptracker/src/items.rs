//! PopTracker's item model: what a pack reads and writes through an item's object.

use crate::item_json::parse_filters;
use crate::session::engine::E;
use mlua::{Lua, Value};

/// How far one item's change may pass on through composites' links.
const MAX_LINK_STEPS: usize = 256;

/// The most of a pack's overlay text or icon path an item keeps: it's held outside
/// Lua's memory cap, and nothing longer is ever shown.
const MAX_ITEM_TEXT: usize = 4096;

/// An item changed: rules must be looked at again, and a composite follows its
/// parts and they it (PopTracker's links, without its events).
fn item_changed(eng: &E, index: usize) {
    let mut e = eng.borrow_mut();
    e.accessibility_stale = true;
    e.visibility_stale = true;
    e.provider_cache.clear();
    e.dirty_items.insert(index);
    // A change may pass on, through a part shared by composites. Each step only
    // follows a real change, and links can't go on for long in any sane pack.
    let mut todo = vec![index];
    let mut steps = 0;
    while let Some(i) = todo.pop() {
        steps += 1;
        if steps > MAX_LINK_STEPS {
            break;
        }
        if e.items[i].kind == "composite_toggle" {
            let stage = e.items[i].stage;
            let parts = e.items[i].parts.clone();
            for (bit, code) in parts.iter().enumerate() {
                let part = e.code_items.get(code).and_then(|l| l.first()).copied();
                if let Some(part) = part.filter(|p| !is_linked_kind(&e.items[*p].kind)) {
                    let on = stage & (1 << bit) != 0;
                    let it = &mut e.items[part];
                    if it.active != on {
                        it.active = on;
                        it.icon = None;
                        e.dirty_items.insert(part);
                        todo.push(part);
                    }
                }
            }
        }
        for c in e.composites.clone() {
            for bit in 0..2 {
                let code = e.items[c].parts[bit].clone();
                if e.code_items.get(&code).and_then(|l| l.first()) == Some(&i) && c != i {
                    let on = e.items[i].state();
                    let it = &mut e.items[c];
                    let stage = if on {
                        it.stage | (1 << bit)
                    } else {
                        it.stage & !(1 << bit)
                    };
                    if it.stage != stage {
                        it.stage = stage;
                        it.icon = None;
                        e.dirty_items.insert(c);
                        todo.push(c);
                    }
                }
            }
        }
    }
}

/// Kinds a composite ignores as its parts.
fn is_linked_kind(kind: &str) -> bool {
    kind == "composite_toggle" || kind == "toggle_badged"
}

/// A number from a pack, as `luaL_checknumber` takes one.
fn lua_number(lua: &Lua, v: Value) -> mlua::Result<i64> {
    match lua.coerce_number(v)? {
        Some(n) => Ok(n as i32 as i64),
        None => Err(mlua::Error::runtime("number expected")),
    }
}

/// `item.field`, from PopTracker's `JsonItem::Lua_Index`.
pub(crate) fn item_get(lua: &Lua, eng: &E, index: usize, key: &str) -> mlua::Result<Value> {
    // Copied out first: making a Lua string can run a pack's `__gc`, which may write
    // to an item.
    let text = {
        let e = eng.borrow();
        let Some(it) = e.items.get(index) else {
            return Ok(Value::Nil);
        };
        match key {
            "AcquiredCount" => return Ok(Value::Integer(it.count)),
            "Active" => return Ok(Value::Boolean(it.active)),
            "CurrentStage" => {
                return Ok(Value::Integer(
                    if it.kind == "progressive" && it.allow_disabled {
                        if it.active { it.stage + 1 } else { 0 }
                    } else {
                        it.stage
                    },
                ));
            }
            "MinCount" => return Ok(Value::Integer(it.min)),
            "MaxCount" => return Ok(Value::Integer(it.max)),
            "Increment" => return Ok(Value::Integer(it.increment)),
            "Decrement" => return Ok(Value::Integer(it.decrement)),
            "IgnoreUserInput" => return Ok(Value::Boolean(it.ignore_user_input)),
            "Type" => it.kind.clone(),
            "Name" => it.name.clone(),
            "Icon" => match &it.icon_text {
                Some(icon) => icon.clone(),
                None => it.image().0.unwrap_or_default(),
            },
            "BadgeText" => it.overlay.clone(),
            "BadgeTextColor" => it.overlay_color.clone(),
            "Owner" => String::new(),
            _ => return Ok(Value::Nil),
        }
    };
    if key == "Owner" {
        return lua.create_table().map(Value::Table);
    }
    lua.create_string(&text).map(Value::String)
}

/// `item.field = value`, from PopTracker's `JsonItem::Lua_NewIndex`; false if the
/// field isn't one of its own.
pub(crate) fn item_set(
    lua: &Lua,
    eng: &E,
    index: usize,
    key: &str,
    v: Value,
) -> mlua::Result<bool> {
    if eng.borrow().items.get(index).is_none() {
        return Ok(false);
    }
    let truthy = |v: &Value| match v {
        Value::Integer(i) => *i > 0,
        Value::Nil | Value::Boolean(false) => false,
        _ => true,
    };
    // As `luaL_checkstring`: a string or a number, and no more than is ever shown.
    let text = |v: Value| -> mlua::Result<String> {
        let s = lua
            .coerce_string(v)?
            .ok_or_else(|| mlua::Error::runtime("string expected"))?;
        let bytes = s.as_bytes();
        let bytes = &bytes[..bytes.len().min(MAX_ITEM_TEXT)];
        Ok(String::from_utf8_lossy(bytes).into_owned())
    };
    let mut changed = false;
    match key {
        "AcquiredCount" => {
            let n = lua_number(lua, v)?;
            let mut e = eng.borrow_mut();
            let it = &mut e.items[index];
            let mut val = n;
            if it.max >= it.min && val > it.max {
                val = it.max;
            }
            val = val.max(it.min);
            if val != it.count {
                it.count = val;
                if it.kind == "consumable" {
                    it.active = val > 0;
                }
                changed = true;
            }
        }
        "Active" => {
            let mut val = truthy(&v);
            let mut e = eng.borrow_mut();
            let it = &mut e.items[index];
            if it.kind == "progressive" && it.allow_disabled && !val {
                it.stage = 0;
            } else if !it.allow_disabled {
                val = true;
            }
            if it.active != val {
                it.active = val;
                it.icon = None;
                it.icon_text = None;
                changed = true;
            }
        }
        "CurrentStage" => {
            let mut val = lua_number(lua, v)?.max(0);
            let mut e = eng.borrow_mut();
            let it = &mut e.items[index];
            let mut en = it.active;
            if it.kind == "progressive" && it.allow_disabled {
                if val == 0 {
                    en = false;
                } else {
                    en = true;
                    val -= 1;
                }
            }
            if !it.stages.is_empty() {
                val = val.min(it.stages.len() as i64 - 1);
            }
            if it.stage != val || it.active != en {
                it.active = en;
                it.stage = val;
                it.icon = None;
                it.icon_text = None;
                changed = true;
            }
        }
        "MinCount" => {
            let n = lua_number(lua, v)?;
            let mut e = eng.borrow_mut();
            let it = &mut e.items[index];
            it.min = n;
            if it.min > it.count {
                it.count = it.min;
                changed = true;
            }
        }
        "MaxCount" => {
            let n = lua_number(lua, v)?;
            let mut e = eng.borrow_mut();
            let it = &mut e.items[index];
            it.max = n;
            if it.max >= it.min && it.count > it.max {
                it.count = it.max;
                changed = true;
            }
        }
        "Increment" => {
            let n = lua_number(lua, v)?;
            eng.borrow_mut().items[index].increment = n;
        }
        "Decrement" => {
            let n = lua_number(lua, v)?;
            eng.borrow_mut().items[index].decrement = n;
        }
        "Icon" => {
            // Nil clears it. An override is `path:mods`, unlike a plain `img`, whose
            // paths may have a colon in them.
            let written = if v.is_nil() { String::new() } else { text(v)? };
            let (path, mods) = written.split_once(':').unwrap_or((&written, ""));
            let mut e = eng.borrow_mut();
            let filters = parse_filters(mods, &e.disabled_filter);
            let it = &mut e.items[index];
            it.icon = Some(path.to_string());
            it.icon_mods = filters;
            it.icon_text = Some(written);
        }
        "IgnoreUserInput" => {
            let on = truthy(&v);
            eng.borrow_mut().items[index].ignore_user_input = on;
        }
        "BadgeText" => {
            let t = text(v)?;
            eng.borrow_mut().items[index].overlay = t;
        }
        "BadgeTextColor" => {
            let t = text(v)?;
            eng.borrow_mut().items[index].overlay_color = t;
        }
        _ => return Ok(false),
    }
    if changed {
        item_changed(eng, index);
    }
    Ok(true)
}
