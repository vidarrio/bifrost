//! The `Archipelago` global.

use crate::Result;
use crate::session::engine::{E, MAX_STORAGE_KEY, MAX_STORAGE_KEYS, PLAYERS};
use mlua::{Function, Lua, MultiValue, Table, Value};

/// The `Archipelago` global: handlers, data-storage reads and players.
pub(super) fn install(lua: &Lua, eng: &E, g: &Table) -> Result<()> {
    // Archipelago
    let ap = lua.create_table()?;
    ap.set("PlayerNumber", 1)?;
    ap.set("TeamNumber", 0)?;
    ap.set("CheckedLocations", lua.create_table()?)?;
    ap.set("MissingLocations", lua.create_table()?)?;
    macro_rules! handler {
        ($method:literal, $field:ident) => {{
            let eng2 = eng.clone();
            ap.set(
                $method,
                lua.create_function(move |_lua, (_t, _name, cb): (Table, String, Function)| {
                    eng2.borrow_mut().$field.push(cb);
                    Ok(())
                })?,
            )?;
        }};
    }
    handler!("AddClearHandler", clear_h);
    handler!("AddItemHandler", item_h);
    handler!("AddLocationHandler", loc_h);
    handler!("AddRetrievedHandler", retr_h);
    handler!("AddSetReplyHandler", setr_h);
    // What a pack asks of the data storage, to answer from a copy of it (see
    // `Session::replay_storage`). Nothing here can write to it.
    for (method, notify) in [("Get", false), ("SetNotify", true)] {
        let eng2 = eng.clone();
        ap.set(
            method,
            lua.create_function(move |_, (_t, keys): (Value, Value)| {
                let Value::Table(keys) = keys else {
                    return Ok(false);
                };
                let mut e = eng2.borrow_mut();
                for key in keys.sequence_values::<String>().flatten() {
                    if key.len() <= MAX_STORAGE_KEY && e.storage_asked.len() < MAX_STORAGE_KEYS {
                        e.storage_asked.insert((key, notify));
                    }
                }
                Ok(true)
            })?,
        )?;
    }
    for noop in [
        "LocationChecks",
        "LocationScouts",
        "HintItem",
        "HintLocation",
        "StatusUpdate",
        "AddScoutHandler",
        "AddBouncedHandler",
    ] {
        ap.set(noop, lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    }
    let status = lua.create_table()?;
    for (k, v) in [
        ("UNKNOWN", 0),
        ("CONNECTED", 5),
        ("READY", 10),
        ("PLAYING", 20),
        ("GOAL", 30),
    ] {
        status.set(k, v)?;
    }
    ap.set("ClientStatus", status)?;
    // Names by id aren't known here yet; AP's answer for an unknown one.
    for lookup in ["GetItemName", "GetLocationName"] {
        ap.set(
            lookup,
            lua.create_function(|_, _: MultiValue| Ok("Unknown"))?,
        )?;
    }
    // Players by slot, once the host has set them (`Session::set_multiworld`):
    // an unknown slot has no game and an empty alias.
    let players = lua.create_table()?;
    lua.set_named_registry_value(PLAYERS, &players)?;
    for (method, field, missing) in [
        ("GetPlayerGame", "game", Value::Nil),
        (
            "GetPlayerAlias",
            "name",
            Value::String(lua.create_string("")?),
        ),
    ] {
        let players = players.clone();
        ap.set(
            method,
            // `Archipelago:GetPlayerGame(slot)`; a dot-call or a missing slot
            // finds nobody rather than failing.
            lua.create_function(move |_, args: MultiValue| {
                let p = match args.into_iter().last() {
                    Some(slot @ (Value::Integer(_) | Value::Number(_))) => {
                        players.get::<Value>(slot)?
                    }
                    _ => Value::Nil,
                };
                Ok(match p {
                    Value::Table(p) => p.get::<Value>(field)?,
                    _ => missing.clone(),
                })
            })?,
        )?;
    }
    g.set("Archipelago", &ap)?;

    Ok(())
}
