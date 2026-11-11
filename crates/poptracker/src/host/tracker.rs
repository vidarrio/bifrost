//! The `Tracker` global.

use crate::Result;
use crate::defs::MapDef;
use crate::engine::{
    E, ITEM_INDEX, ITEM_METHODS, ITEM_OBJECTS, LOCATION_METHODS, MAX_LOOKUP_DEPTH,
};
use crate::item_json::read_item;
use crate::json::parse_pack_json;
use crate::layout::{flatten_locations, num};
use crate::rules::locations::{location_object, mark_indirect};
use crate::rules::providers::{custom_can_provide, provider_count_cached};
use crate::session::read_pack_file;
use mlua::{Lua, MultiValue, Table, Value};
use std::rc::Rc;

/// The `Tracker` global: loading items, locations, maps and layouts, and finding objects.
pub(super) fn install(lua: &Lua, eng: &E, g: &Table) -> Result<()> {
    // Tracker
    let tracker = lua.create_table()?;
    {
        let eng2 = eng.clone();
        tracker.set(
            "AddItems",
            lua.create_function(move |lua, (_t, file): (Table, String)| {
                // PopTracker's Add* return a bool and tolerate a missing/broken
                // file; mirror that so one bad path can't reject the whole pack.
                let Ok(txt) = read_pack_file(&eng2, &file) else {
                    return Ok(false);
                };
                let Ok(json) = parse_pack_json(&txt) else {
                    return Ok(false);
                };
                let objects: Table = lua.named_registry_value(ITEM_OBJECTS)?;
                let methods: Table = lua.named_registry_value(ITEM_METHODS)?;
                for item in json.as_array().map(Vec::as_slice).unwrap_or_default() {
                    let disabled = eng2.borrow().disabled_filter.clone();
                    let item = read_item(item, &disabled);
                    let index = {
                        let mut e = eng2.borrow_mut();
                        let index = e.items.len();
                        for code in item.all_codes() {
                            let list = e.code_items.entry(code).or_default();
                            if list.last() != Some(&index) {
                                list.push(index);
                            }
                        }
                        if item.kind == "composite_toggle" {
                            e.composites.push(index);
                        }
                        e.items.push(item);
                        e.accessibility_stale = true;
                        e.visibility_stale = true;
                        e.provider_cache.clear();
                        index
                    };
                    // A composite starts from its parts.
                    {
                        let mut e = eng2.borrow_mut();
                        let parts = e.items[index].parts.clone();
                        let mut stage = e.items[index].stage;
                        for (bit, code) in parts.iter().enumerate() {
                            if let Some(part) = e.code_items.get(code).and_then(|l| l.first())
                                && *part != index
                                && e.items[*part].state()
                            {
                                stage |= 1 << bit;
                            }
                        }
                        e.items[index].stage = stage;
                    }
                    let t = lua.create_table()?;
                    t.raw_set(ITEM_INDEX, index as i64)?;
                    t.set_metatable(Some(methods.clone()))?;
                    objects.raw_set(index as i64 + 1, t)?;
                }
                Ok(true)
            })?,
        )?;
    }
    {
        let eng2 = eng.clone();
        tracker.set(
            "AddLocations",
            lua.create_function(move |_lua, (_t, file): (Table, String)| {
                let Ok(txt) = read_pack_file(&eng2, &file) else {
                    return Ok(false);
                };
                let Ok(json) = parse_pack_json(&txt) else {
                    return Ok(false);
                };
                let scoutable = {
                    let mut e = eng2.borrow_mut();
                    e.locations_before = e.nodes.len();
                    e.scoutable_default
                };
                flatten_locations(&eng2, &json, "", &Vec::new(), &Vec::new(), scoutable);
                Ok(true)
            })?,
        )?;
    }
    {
        let eng2 = eng.clone();
        tracker.set(
            "AddMaps",
            lua.create_function(move |_lua, (_t, file): (Table, String)| {
                let Ok(txt) = read_pack_file(&eng2, &file) else {
                    return Ok(false);
                };
                let Ok(json) = parse_pack_json(&txt) else {
                    return Ok(false);
                };
                let mut e = eng2.borrow_mut();
                for m in json.as_array().cloned().unwrap_or_default() {
                    let map = MapDef {
                        name: m
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        img: m
                            .get("img")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        location_size: m.get("location_size").and_then(num),
                        location_orientation: m
                            .get("location_orientation")
                            .and_then(|v| v.as_str().map(String::from)),
                        location_border: m.get("location_border_thickness").and_then(num),
                        location_shape: m
                            .get("location_shape")
                            .and_then(|v| v.as_str().map(String::from)),
                    };
                    // A map added again replaces the one of that name, as in PopTracker:
                    // packs switch a map's image this way.
                    match e.maps.iter_mut().find(|old| old.name == map.name) {
                        Some(old) => *old = map,
                        None => e.maps.push(map),
                    }
                }
                Ok(true)
            })?,
        )?;
    }
    {
        let eng2 = eng.clone();
        let depth = Rc::new(std::cell::Cell::new(0u32));
        tracker.set(
            "FindObjectForCode",
            lua.create_function(move |lua, (_t, code): (Table, Option<String>)| {
                let Some(code) = code else {
                    return Ok(Value::Nil);
                };
                // `@`-codes address a section or location, found as in rules. A
                // section's object (made on first use) has the mutable
                // `AvailableChestCount` a pack's `onLocation` decrements, which is
                // how checks show here. A location with one section means that
                // section; ids are case-sensitive, unlike item codes.
                if let Some(path) = code.strip_prefix('@') {
                    mark_indirect(&eng2);
                    let Some((id, count)) = location_object(&eng2, path) else {
                        return Ok(Value::Nil);
                    };
                    let objs: Table = lua.globals().get("__LOCOBJS")?;
                    if let Ok(t) = objs.raw_get::<Table>(id.as_str()) {
                        return Ok(Value::Table(t));
                    }
                    let t = lua.create_table()?;
                    if let Some(c) = count {
                        t.set("AvailableChestCount", c)?;
                        t.set("ChestCount", c)?;
                        t.set("Highlight", 0)?;
                    }
                    t.set("Active", false)?;
                    t.set("FullID", id.as_str())?;
                    t.set("Owner", lua.create_table()?)?;
                    t.set_metatable(Some(lua.named_registry_value(LOCATION_METHODS)?))?;
                    objs.raw_set(id.as_str(), &t)?;
                    return Ok(Value::Table(t));
                }
                let first = (eng2.borrow().code_items.get(&code.to_lowercase()))
                    .and_then(|l| l.first())
                    .copied();
                if let Some(index) = first {
                    let objects: Table = lua.named_registry_value(ITEM_OBJECTS)?;
                    return objects.raw_get(index as i64 + 1);
                }
                // A custom item (`ScriptHost:CreateLuaItem`) answers for the codes
                // its `CanProvideCodeFunc` accepts (a failing one accepts none).
                if depth.get() >= MAX_LOOKUP_DEPTH {
                    return Ok(Value::Nil);
                }
                depth.set(depth.get() + 1);
                let found = (|| {
                    let custom: Table = lua.globals().get("__LUAITEMS")?;
                    for item in custom.sequence_values::<Table>() {
                        let item = item?;
                        if eng2.borrow().budget.get() == 0 {
                            break;
                        }
                        if custom_can_provide(&eng2, &item, &code) {
                            return Ok(Value::Table(item));
                        }
                    }
                    Ok(Value::Nil)
                })();
                depth.set(depth.get() - 1);
                found
            })?,
        )?;
    }
    {
        let eng2 = eng.clone();
        tracker.set(
            "ProviderCountForCode",
            lua.create_function(move |lua, (_t, code): (Table, Option<String>)| {
                Ok(code.map_or(0, |c| provider_count_cached(lua, &eng2, &c)))
            })?,
        )?;
    }
    {
        let eng2 = eng.clone();
        tracker.set(
            "AddLayouts",
            lua.create_function(move |_lua, (_t, file): (Table, String)| {
                let Ok(txt) = read_pack_file(&eng2, &file) else {
                    return Ok(false);
                };
                let Ok(json) = parse_pack_json(&txt) else {
                    return Ok(false);
                };
                if let Some(obj) = json.as_object() {
                    let mut e = eng2.borrow_mut();
                    for (k, v) in obj {
                        e.layouts.insert(k.clone(), v.clone());
                    }
                }
                Ok(true)
            })?,
        )?;
    }
    for noop in ["AddClasses", "UiHint", "OpenLink"] {
        tracker.set(noop, lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    }
    tracker.set("ActiveVariantUID", eng.borrow().variant.as_str())?;
    g.set("Tracker", &tracker)?;

    Ok(())
}
