//! Locations and sections as rules and objects refer to them.

use crate::defs::LocNode;
use crate::engine::E;
use crate::rules::providers::provider_count;
use crate::rules::{LEVEL_NONE, LEVEL_NORMAL, Level};
use mlua::Lua;

/// A function running for a rule looked at a location (see `provider_count_cached`).
pub(crate) fn mark_indirect(eng: &E) {
    let mut e = eng.borrow_mut();
    if e.updating_cache {
        let codes = e.lua_codes.clone();
        e.indirect_codes.extend(codes);
    }
}

/// PopTracker's `getLocation`: by id, else by name, or by the end of an id.
fn find_location(nodes: &[LocNode], id: &str) -> Option<usize> {
    if let Some(i) = nodes.iter().position(|n| n.name == id) {
        return Some(i);
    }
    if !id.contains('/') {
        return nodes.iter().position(|n| n.short_name == id);
    }
    let search = format!("/{id}");
    nodes.iter().position(|n| n.name.ends_with(&search))
}

/// Find a location for a reference, remembering the answer: packs refer to the
/// same ones over and over, and each search goes through every location.
fn find_location_cached(eng: &E, id: &str) -> Option<usize> {
    if let Some(hit) = eng.borrow().location_lookup.get(id) {
        return *hit;
    }
    let nodes = eng.borrow().nodes.clone();
    let hit = find_location(&nodes, id);
    eng.borrow_mut().location_lookup.insert(id.to_string(), hit);
    hit
}

pub(crate) fn all_held(lua: &Lua, eng: &E, codes: &[String]) -> bool {
    codes.iter().all(|code| provider_count(lua, eng, code) >= 1)
}

/// Are the items a section, by its exact id, hosts all held?
pub(crate) fn hosted_collected(lua: &Lua, eng: &E, id: &str) -> bool {
    let hosted = {
        let e = eng.borrow();
        e.nodes
            .iter()
            .find_map(|n| {
                let section = id.strip_prefix(n.name.as_str())?.strip_prefix('/')?;
                n.sections.iter().find(|s| s.name == section)
            })
            .map(|s| s.hosted.clone())
    };
    all_held(lua, eng, &hosted.unwrap_or_default())
}

/// The object `Tracker:FindObjectForCode("@…")` means: its id, and a chest count
/// if it's a section.
pub(crate) fn location_object(eng: &E, path: &str) -> Option<(String, Option<i64>)> {
    if let Some((loc, section)) = path.rsplit_once('/')
        && let Some(i) = find_location_cached(eng, loc)
    {
        let e = eng.borrow();
        let n = &e.nodes[i];
        if let Some(s) = n.sections.iter().find(|s| s.name == section) {
            return Some((format!("{}/{}", n.name, s.name), Some(s.item_count)));
        }
    }
    let i = find_location_cached(eng, path)?;
    let e = eng.borrow();
    let n = &e.nodes[i];
    Some(match n.sections.as_slice() {
        [s] => (format!("{}/{}", n.name, s.name), Some(s.item_count)),
        _ => (n.name.clone(), None),
    })
}

/// Which section a section's `ref` means, as `location/section`: rules and the
/// tracker see the section it points to.
pub(crate) fn real_section(eng: &E, loc: usize, section: usize) -> Option<(usize, usize)> {
    let reference = {
        let e = eng.borrow();
        let s = e.nodes.get(loc)?.sections.get(section)?;
        match &s.reference {
            Some(r) => r.clone(),
            None => return Some((loc, section)),
        }
    };
    let (target_loc, target_section) = reference.rsplit_once('/')?;
    let i = find_location_cached(eng, target_loc)?;
    let e = eng.borrow();
    let j = e.nodes[i]
        .sections
        .iter()
        .position(|s| s.name == target_section)?;
    Some((i, j))
}

/// A reference's level, from what's settled so far. As in PopTracker, a target with
/// no visibility rules is visible, and one not settled yet isn't.
pub(super) fn reference_level(eng: &E, id: &str, visibility: bool) -> Option<Level> {
    let read = |key: &str, vis_empty: bool| -> Level {
        let e = eng.borrow();
        if visibility {
            let seen = vis_empty || e.visibility.get(key).copied().unwrap_or(false);
            if seen { LEVEL_NORMAL } else { LEVEL_NONE }
        } else {
            e.accessibility.get(key).copied().unwrap_or(LEVEL_NONE)
        }
    };
    if let Some(i) = find_location_cached(eng, id) {
        let (key, vis_empty) = {
            let e = eng.borrow();
            (e.nodes[i].name.clone(), e.nodes[i].vis.is_empty())
        };
        return Some(read(&key, vis_empty));
    }
    let (loc, section) = id.rsplit_once('/')?;
    let i = find_location_cached(eng, loc)?;
    let j = eng.borrow().nodes[i]
        .sections
        .iter()
        .position(|s| s.name == section)?;
    let Some((i, j)) = real_section(eng, i, j) else {
        // A `ref` to nothing is a blank section: never reachable, always visible.
        return Some(if visibility { LEVEL_NORMAL } else { LEVEL_NONE });
    };
    let (key, vis_empty) = {
        let e = eng.borrow();
        let n = &e.nodes[i];
        (
            format!("{}/{}", n.name, n.sections[j].name),
            n.sections[j].vis.is_empty(),
        )
    };
    Some(read(&key, vis_empty))
}
