//! A session's reports: the tracker state and the pack summary.

use crate::layout::resolve_layout;
use crate::rules::locations::{all_held, real_section};
use crate::rules::settle::{settle_accessibility, settle_visibility};
use crate::rules::{LEVEL_NONE, LEVEL_NORMAL, Rules, level_name, resolve_rules};
use crate::sandbox::{lua_int, note_error};
use crate::session::engine::CODE_WATCHES;
use crate::session::{Session, registered};
use crate::state::{
    ItemState, LocationState, MapInfo, Marker, SectionState, Summary, TrackerState,
};
use mlua::Table;

impl Session {
    /// Snapshot the current tracker state (items + location accessibility).
    pub fn state(&self) -> TrackerState {
        self.charge();
        if let Err(e) = self.run_frame_handlers() {
            note_error(&self.eng, &e.to_string());
            self.charge();
        }
        // Frame handlers may have changed items, and a "*" watch hears once more that
        // anything may have changed: a pack may keep state outside items.
        if let Err(e) = self.run_code_watches() {
            note_error(&self.eng, &e.to_string());
        }
        for w in registered(&self.lua, CODE_WATCHES, |e| {
            e.get::<mlua::LuaString>("code")
                .is_ok_and(|c| c.as_bytes().as_ref() == b"*")
        }) {
            self.guard_noting(w.call::<()>("*"));
        }
        if let Err(e) = self.run_code_watches() {
            note_error(&self.eng, &e.to_string());
        }
        self.charge();
        let mut out = TrackerState::default();
        for (index, it) in self.eng.borrow().items.iter().enumerate() {
            // A consumable shows its count, first; an item is "lit" if active or
            // held.
            let overlay = (it.kind == "consumable" && it.count > 0)
                .then(|| it.count.to_string())
                .or_else(|| (!it.overlay.is_empty()).then(|| it.overlay.clone()));
            let (img, filters) = it.image();
            out.items.push(ItemState {
                key: format!("__item{index}"),
                kind: it.kind.clone(),
                active: it.state() || it.count > 0,
                count: it.count,
                stage: it.stage,
                codes: it.all_codes(),
                name: it.name.clone(),
                img,
                filters,
                overlay,
            });
        }
        out.items.sort_by(|a, b| a.key.cmp(&b.key));

        // maps
        let maps = self.eng.borrow().maps.clone();
        for m in maps {
            out.maps.push(MapInfo {
                name: m.name,
                img: m.img,
                location_size: m.location_size,
                location_shape: m.location_shape,
                location_orientation: m.location_orientation,
                location_border: m.location_border,
            });
        }

        // locations + markers (from the node model)
        // Only enforce `visibility_rules` once the slot's valid-location set is
        // known; otherwise every marker shows (fail-safe, old behavior).
        let enforce_vis = self.eng.borrow().valid_locations.is_some();
        // Handlers may have changed anything, so settle here, once.
        self.charge();
        settle_accessibility(&self.lua, &self.eng);
        if enforce_vis {
            settle_visibility(&self.lua, &self.eng);
        }
        // Marker rules below keep function results for this pass, as settling does.
        self.eng.borrow_mut().updating_cache = true;
        // After settling: a rule function may have added locations.
        let nodes = self.eng.borrow().nodes.clone();
        let level_of = |id: &str| {
            let e = self.eng.borrow();
            e.accessibility.get(id).copied().unwrap_or(LEVEL_NONE)
        };
        let visible = |id: &str, rules: &Rules| {
            !enforce_vis
                || rules.is_empty()
                || self
                    .eng
                    .borrow()
                    .visibility
                    .get(id)
                    .copied()
                    .unwrap_or(false)
        };
        for (index, n) in nodes.iter().enumerate().filter(|(_, n)| n.drawn()) {
            // Hide a location whose visibility rules fail (e.g. options the slot
            // didn't generate) — dropped from both markers and the locations list,
            // just as native PopTracker omits them.
            if !visible(&n.name, &n.vis) {
                continue;
            }
            let loc_access = level_of(&n.name) >= LEVEL_NORMAL;
            let locobjs: Option<Table> = self.lua.globals().get("__LOCOBJS").ok();
            // The chests still available in a section's chest object (if the pack
            // touched it via `onLocation`). `None` => the pack never tracked it.
            let section_avail = |key: &str| -> Option<i64> {
                let objs = locobjs.as_ref()?;
                objs.raw_get::<Table>(key)
                    .ok()
                    .and_then(|t| lua_int(&t, "AvailableChestCount"))
            };
            let sections: Vec<SectionState> = n
                .sections
                .iter()
                .enumerate()
                // A section can carry its own `visibility_rules` (Stardew uses them
                // at section level, e.g. per Desert Festival stall); drop the ones
                // the slot didn't generate, same as the node-level filter above.
                .filter(|(j, _)| match real_section(&self.eng, index, *j) {
                    Some((a, b)) => {
                        let target = &nodes[a].sections[b];
                        visible(&format!("{}/{}", nodes[a].name, target.name), &target.vis)
                    }
                    // A `ref` to nothing is a blank section, with no rules to fail.
                    None => true,
                })
                .map(|(j, s)| {
                    // A `ref` shows the section it points to, count and level.
                    let (key, item_count, hosted) = match real_section(&self.eng, index, j) {
                        Some((a, b)) => (
                            format!("{}/{}", nodes[a].name, nodes[a].sections[b].name),
                            nodes[a].sections[b].item_count,
                            nodes[a].sections[b].hosted.as_slice(),
                        ),
                        None => (String::new(), s.item_count, &[][..]),
                    };
                    let avail = section_avail(&key);
                    let lvl = level_of(&key);
                    SectionState {
                        name: s.name.clone(),
                        item_count,
                        cleared: avail.map_or(0, |a| item_count.saturating_sub(a).max(0)),
                        accessible: lvl >= LEVEL_NORMAL,
                        level: level_name(lvl).to_string(),
                        highlight: locobjs
                            .as_ref()
                            .and_then(|o| o.raw_get::<Table>(key.as_str()).ok())
                            .and_then(|t| t.raw_get::<Option<i64>>("Highlight").ok().flatten())
                            .unwrap_or(0),
                        // Nothing left to collect, its hosted items included.
                        checked: avail.map_or(item_count <= 0, |a| a <= 0)
                            && all_held(&self.lua, &self.eng, hosted),
                    }
                })
                .collect();
            // A node that declares sections but has none left after visibility
            // filtering isn't a real check for this slot — omit it entirely.
            if enforce_vis && !n.sections.is_empty() && sections.is_empty() {
                continue;
            }
            // Location is "Done" (grey) when it has sections and every one is checked.
            let loc_checked = !sections.is_empty() && sections.iter().all(|s| s.checked);
            for ml in &n.map_locations {
                // A map location's own rules, as PopTracker's `isVisible` has them.
                if enforce_vis && self.budget.get() != 0 {
                    let met = |rules: &Rules| {
                        resolve_rules(&self.lua, &self.eng, rules, true, false) != LEVEL_NONE
                    };
                    if (!ml.force_hidden.is_empty() && met(&ml.force_hidden)) || !met(&ml.restrict)
                    {
                        continue;
                    }
                }
                out.markers.push(Marker {
                    map: ml.map.clone(),
                    x: ml.x,
                    y: ml.y,
                    size: ml.size,
                    shape: ml.shape.clone(),
                    orientation: ml.orientation.clone(),
                    border: ml.border,
                    location: n.display_name.clone(),
                    accessible: loc_access,
                    checked: loc_checked,
                    sections: sections.clone(),
                });
            }
            out.locations.push(LocationState {
                name: n.display_name.clone(),
                accessible: loc_access,
                checked: loc_checked,
                sections,
            });
        }

        // resolved layout tree (tracker_default + fallbacks), `layout` refs inlined
        let layouts = self.eng.borrow().layouts.clone();
        for root in ["tracker_default", "tracker_horizontal", "tracker_vertical"] {
            if let Some(n) = layouts.get(root) {
                out.layout = Some(resolve_layout(&layouts, n, 0));
                break;
            }
        }
        {
            let mut e = self.eng.borrow_mut();
            e.updating_cache = false;
            e.indirect_codes.clear();
        }
        if self.budget.get() == 0 {
            note_error(
                &self.eng,
                "the pack's rules ran out of instruction budget, so some may read as unmet",
            );
        }
        // Last: evaluating the rules above runs pack code too.
        {
            let e = self.eng.borrow();
            out.pack_errors = e.pack_errors.clone();
            out.pack_error_count = e.pack_error_count;
        }
        out
    }

    /// Whether the pack registered any Archipelago autotracking handlers — i.e.
    /// it can be driven from AP server state. Memory-only (emulator) packs
    /// register none; the upload path should reject them with a clear message.
    pub fn has_archipelago_autotracking(&self) -> bool {
        let e = self.eng.borrow();
        !(e.clear_h.is_empty()
            && e.item_h.is_empty()
            && e.loc_h.is_empty()
            && e.retr_h.is_empty()
            && e.setr_h.is_empty())
    }

    /// A summary for validating an uploaded pack (counts + AP-autotracking flag).
    pub fn summary(&self) -> Summary {
        let e = self.eng.borrow();
        Summary {
            items: e.items.len(),
            locations: e.nodes.iter().filter(|n| n.drawn()).count(),
            maps: e.maps.len(),
            variant: e.variant.clone(),
            archipelago_autotracking: !(e.clear_h.is_empty()
                && e.item_h.is_empty()
                && e.loc_h.is_empty()
                && e.retr_h.is_empty()
                && e.setr_h.is_empty()),
        }
    }
}
