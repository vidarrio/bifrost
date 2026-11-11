//! A live engine session for one pack and one slot.

mod report;

use crate::engine::{
    CODE_WATCHES, DIRTY_CUSTOM, E, Eng, FRAME_HANDLERS, MAX_STORAGE_ROUNDS, MAX_WATCH_ROUNDS,
    PLAYERS,
};
use crate::host::install_host;
use crate::json::{json_to_lua, parse_pack_json};
use crate::pack::{Pack, find_in_pack};
use crate::rules::providers::{count_custom_call, custom_can_provide};
use crate::sandbox::{is_memory_error, note_error, run_pack_source, sandboxed_lua};
use crate::{Error, Result};
use mlua::{Function, Lua, Table, Value};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

/// A live engine session for one pack + one slot.
///
/// Holds a Lua VM, so it is `!Send` — run it on a dedicated/blocking task.
pub struct Session {
    lua: Lua,
    eng: E,
    /// Remaining instruction-hook ticks for the current operation; the debug
    /// hook decrements it and aborts at zero so untrusted pack Lua can't hang
    /// the host. Reset (via [`Self::charge`]) at the start of each public call.
    budget: Rc<std::cell::Cell<u64>>,
}

/// Fire the debug hook every this many Lua VM instructions.
const HOOK_EVERY: u32 = 1000;
/// Max hook ticks per operation → `HOOK_EVERY * MAX_TICKS` instructions before
/// a call is aborted as a runaway (~tens of millions; finite packs never reach it).
const MAX_TICKS: u64 = 100_000;

impl Session {
    /// Create a session: set up the sandboxed host + instruction cap, then run
    /// the pack's `init.lua` (which loads its items/locations/scripts and
    /// registers its Archipelago handlers).
    pub fn new(pack: &Pack) -> Result<Session> {
        let lua = sandboxed_lua()?;
        let budget = Rc::new(std::cell::Cell::new(MAX_TICKS));
        {
            let b = budget.clone();
            // Global, not per-thread: a pack's coroutines get the cap too
            // (set_hook only covers the main thread).
            lua.set_global_hook(
                mlua::HookTriggers::new().every_nth_instruction(HOOK_EVERY),
                move |_lua, _dbg| {
                    let r = b.get();
                    if r == 0 {
                        return Err(mlua::Error::runtime(
                            "instruction limit exceeded (possible infinite loop in pack Lua)",
                        ));
                    }
                    b.set(r - 1);
                    Ok(mlua::VmState::Continue)
                },
            )?;
        }
        let eng: E = Rc::new(RefCell::new(Eng {
            dir: pack.dir.clone(),
            variant: pack.variant.clone(),
            scoutable_default: pack.scoutable_glitched,
            disabled_filter: "grey".to_string(),
            budget: budget.clone(),
            ..Default::default()
        }));
        lua.load("__LOCOBJS = {}").exec()?;
        lua.load("__LUAITEMS = {}").exec()?;
        // A variant's settings.json, or the pack's.
        if let Some(filter) = read_pack_file(&eng, "settings.json")
            .ok()
            .and_then(|t| parse_pack_json(&t).ok())
            .and_then(|j| j.get("disabled_image_filter")?.as_str().map(String::from))
        {
            eng.borrow_mut().disabled_filter = filter;
        }
        install_host(&lua, &eng)?;

        budget.set(MAX_TICKS);
        let init = read_pack_file(&eng, "scripts/init.lua")?;
        run_pack_source(&lua, &init, "init.lua")?;

        let s = Session { lua, eng, budget };
        s.run_frame_handlers()?;
        s.run_code_watches()?;
        Ok(s)
    }

    /// Tell code watches what's changed, as PopTracker does on each change, here at
    /// the end of each operation. A watch gets the code it watches, and a "*" one
    /// the item's codes. A watch may change items, which is told in turn, a few times.
    /// So a watch also hears of what changed earlier in the operation that added it,
    /// and one added by a watch hears from the next round. All of this counts as
    /// work against the operation's limit: it runs no Lua instructions.
    fn run_code_watches(&self) -> Result<()> {
        for _ in 0..MAX_WATCH_ROUNDS {
            let (items, custom_changed): (Vec<(String, Vec<String>)>, bool) = {
                let mut e = self.eng.borrow_mut();
                let dirty = std::mem::take(&mut e.dirty_items);
                let custom = std::mem::take(&mut e.dirty_custom);
                let items = dirty
                    .into_iter()
                    .map(|i| (e.items[i].codes_text.clone(), e.items[i].all_codes()))
                    .collect();
                (items, custom)
            };
            if items.is_empty() && !custom_changed {
                return Ok(());
            }
            let dirty_codes: HashSet<&str> = items
                .iter()
                .flat_map(|(_, codes)| codes.iter().map(String::as_str))
                .collect();
            // Which custom items changed, in the order they were made.
            let dirty_custom: Vec<Table> = if custom_changed {
                let dirty: Table = self.lua.named_registry_value(DIRTY_CUSTOM)?;
                let all: Table = self.lua.globals().get("__LUAITEMS")?;
                let changed = all
                    .sequence_values::<Table>()
                    .flatten()
                    .filter(|i| dirty.raw_get::<bool>(i).unwrap_or(false))
                    .collect();
                self.lua
                    .set_named_registry_value(DIRTY_CUSTOM, self.lua.create_table()?)?;
                changed
            } else {
                Vec::new()
            };
            let Ok(list) = self.lua.named_registry_value::<Table>(CODE_WATCHES) else {
                return Ok(());
            };
            let watches: Vec<Table> = list.sequence_values::<Table>().flatten().collect();
            for w in watches {
                if !count_custom_call(&self.eng) {
                    return Err(Error::Lua(
                        "instruction limit exceeded (too many code watches)".into(),
                    ));
                }
                if w.get::<bool>("dead").unwrap_or(true) {
                    continue;
                }
                let (Ok(code), Ok(f)) = (w.get::<String>("code"), w.get::<Function>("fn")) else {
                    continue;
                };
                if code == "*" {
                    for (text, _) in &items {
                        self.guard(f.call::<()>(text.as_str()))?;
                    }
                    for item in &dirty_custom {
                        let name = item.raw_get::<Option<String>>("Name").ok().flatten();
                        self.guard(f.call::<()>(name.unwrap_or_default()))?;
                    }
                    continue;
                }
                let hit = dirty_codes.contains(code.to_lowercase().as_str())
                    || dirty_custom
                        .iter()
                        .any(|i| custom_can_provide(&self.eng, i, &code));
                if hit {
                    self.guard(f.call::<()>(code.as_str()))?;
                }
            }
        }
        // Still changing after that: the next operation picks it up.
        Ok(())
    }

    /// As in PopTracker, an error in one of the pack's handlers is noted and the
    /// tracker carries on, as other items and locations can still track. Only
    /// running out of budget or memory ends the operation: a pack can't go on.
    fn guard(&self, r: mlua::Result<()>) -> Result<()> {
        let Err(e) = r else { return Ok(()) };
        if self.budget.get() == 0 || is_memory_error(&e) {
            return Err(e.into());
        }
        note_error(&self.eng, &e.to_string());
        Ok(())
    }

    /// `guard` where nothing can be returned: a pack stopped for good here shows
    /// as one more error.
    fn guard_noting(&self, r: mlua::Result<()>) {
        if let Err(e) = self.guard(r) {
            note_error(&self.eng, &e.to_string());
        }
    }

    /// Run the frame handlers once (see `FRAME_HANDLERS`), with no time
    /// elapsed. A handler may remove itself or add others; those added run next
    /// time. As in PopTracker, a handler's error doesn't stop the tracker.
    fn run_frame_handlers(&self) -> Result<()> {
        for f in registered(&self.lua, FRAME_HANDLERS, |_| true) {
            self.guard(f.call::<()>(0.0))?;
        }
        Ok(())
    }

    /// Reset the per-operation instruction budget. Call at the start of each
    /// public method that runs pack Lua.
    fn charge(&self) {
        self.budget.set(MAX_TICKS);
        let mut e = self.eng.borrow_mut();
        e.accessibility_stale = true;
        e.visibility_stale = true;
        e.custom_calls = 0;
    }

    /// Set the slot's AP player number (`Archipelago.PlayerNumber`), which some
    /// packs read to tell local from remote items. Call before feeding state.
    pub fn set_player(&self, n: i64) {
        if let Ok(ap) = self.lua.globals().get::<Table>("Archipelago") {
            let _ = ap.set("PlayerNumber", n);
        }
    }

    /// Set the multiworld's players, as `(slot, name, game)`, for
    /// `Archipelago:GetPlayerAlias` / `GetPlayerGame`, and its seed name
    /// (`Archipelago.Seed`). Call before feeding state.
    pub fn set_multiworld(&self, seed: Option<&str>, players: &[(i64, &str, &str)]) -> Result<()> {
        let ap: Table = self.lua.globals().get("Archipelago")?;
        ap.set("Seed", seed)?;
        let t: Table = self.lua.named_registry_value(PLAYERS)?;
        for (slot, name, game) in players {
            let p = self.lua.create_table()?;
            p.set("name", *name)?;
            p.set("game", *game)?;
            t.set(*slot, p)?;
        }
        Ok(())
    }

    /// Feed the slot's valid AP location ids (checked ∪ missing). This switches
    /// on `visibility_rules` enforcement: a bare numeric visibility code resolves
    /// to "present" iff its id is in this set, so markers for locations the
    /// slot's options never generated are hidden (as in native PopTracker).
    /// Also populates `Archipelago.MissingLocations`/`CheckedLocations` so a
    /// pack's own `onClear` can set the same toggles if it reads them.
    pub fn set_valid_locations(&self, ids: &[i64]) {
        self.eng.borrow_mut().valid_locations = Some(ids.iter().copied().collect());
        if let Ok(ap) = self.lua.globals().get::<Table>("Archipelago")
            && let Ok(t) = self.lua.create_table()
        {
            for (i, id) in ids.iter().enumerate() {
                let _ = t.set(i as i64 + 1, *id);
            }
            let _ = ap.set("MissingLocations", t);
        }
    }

    /// Fire the pack's Clear handlers (called on connect) with `slot_data`.
    pub fn on_clear(&self, slot_data: &serde_json::Value) -> Result<()> {
        self.charge();
        let sd = json_to_lua(&self.lua, slot_data)?;
        // Copy the list out first: a handler may register more handlers, which
        // needs the engine borrowed mutably.
        let handlers = self.eng.borrow().clear_h.clone();
        for h in handlers {
            self.guard(h.call::<()>(sd.clone()))?;
        }
        self.run_frame_handlers()?;
        self.run_code_watches()
    }

    /// Fire the pack's Item handlers for one received item.
    pub fn on_item(&self, index: i64, item_id: i64, item_name: &str, player: i64) -> Result<()> {
        self.charge();
        let handlers = self.eng.borrow().item_h.clone();
        for h in handlers {
            self.guard(h.call::<()>((index, item_id, item_name, player)))?;
        }
        self.run_code_watches()?;
        Ok(())
    }

    /// Fire the pack's Location handlers for one checked location.
    pub fn on_location(&self, location_id: i64, location_name: &str) -> Result<()> {
        self.charge();
        let handlers = self.eng.borrow().loc_h.clone();
        for h in handlers {
            self.guard(h.call::<()>((location_id, location_name)))?;
        }
        self.run_code_watches()?;
        Ok(())
    }

    /// Fire the pack's Retrieved/SetReply handlers for a data-storage value.
    pub fn on_retrieved(&self, key: &str, val: &serde_json::Value) -> Result<()> {
        self.charge();
        let v = json_to_lua(&self.lua, val)?;
        let handlers: Vec<Function> = {
            let e = self.eng.borrow();
            e.retr_h.iter().chain(e.setr_h.iter()).cloned().collect()
        };
        for h in handlers {
            self.guard(h.call::<()>((key, v.clone())))?;
        }
        self.run_code_watches()?;
        Ok(())
    }

    /// Answer what the pack has asked of the data storage from `read`, as the server
    /// would after a connect: `Get` through its Retrieved handlers, and `SetNotify`
    /// through its SetReply ones, with the value as it stands. Handlers may ask for
    /// more; that's answered too, a few times over.
    pub fn replay_storage(&self, read: impl Fn(&str) -> Option<serde_json::Value>) -> Result<()> {
        for _ in 0..MAX_STORAGE_ROUNDS {
            let asked = std::mem::take(&mut self.eng.borrow_mut().storage_asked);
            // A `Get` answers a key for good. A `SetNotify` alone is told only of a
            // value that exists: the server sends nothing at connect, but a pack
            // here can't see the changes that led to it.
            let got: HashSet<String> = (asked.iter())
                .filter(|(_, notify)| !notify)
                .map(|(k, _)| k.clone())
                .collect();
            let mut fresh: Vec<(String, bool)> = Vec::new();
            {
                let mut e = self.eng.borrow_mut();
                for (key, notify) in asked {
                    let told =
                        got.contains(&key) || e.storage_answered.contains(&(key.clone(), false));
                    if notify && told {
                        continue;
                    }
                    if e.storage_answered.insert((key.clone(), notify)) {
                        fresh.push((key, notify));
                    }
                }
            }
            if fresh.is_empty() {
                break;
            }
            self.charge();
            for (key, notify) in fresh {
                let value = read(&key);
                if notify && value.is_none() {
                    continue;
                }
                let value = match value {
                    Some(v) => json_to_lua(&self.lua, &v)?,
                    None => Value::Nil,
                };
                let handlers = if notify {
                    self.eng.borrow().setr_h.clone()
                } else {
                    self.eng.borrow().retr_h.clone()
                };
                for h in handlers {
                    let args = (key.as_str(), value.clone(), Value::Nil);
                    self.guard(h.call::<()>(args))?;
                }
            }
            self.run_code_watches()?;
        }
        Ok(())
    }

    /// The locations this slot has checked, as `Archipelago.CheckedLocations`, with
    /// the rest of the valid ones as `MissingLocations`. Call before feeding state.
    pub fn set_checked_locations(&self, checked: &[i64]) {
        let done: HashSet<i64> = checked.iter().copied().collect();
        let mut missing: Vec<i64> = {
            let e = self.eng.borrow();
            e.valid_locations
                .iter()
                .flatten()
                .filter(|id| !done.contains(id))
                .copied()
                .collect()
        };
        missing.sort_unstable();
        if let Ok(ap) = self.lua.globals().get::<Table>("Archipelago") {
            for (field, list) in [
                ("CheckedLocations", checked),
                ("MissingLocations", &missing),
            ] {
                if let Ok(t) = self.lua.create_sequence_from(list.iter().copied()) {
                    let _ = ap.set(field, t);
                }
            }
        }
    }
}

pub(crate) fn read_pack_file(eng: &E, rel: &str) -> Result<String> {
    let (dir, variant) = {
        let e = eng.borrow();
        (e.dir.clone(), e.variant.clone())
    };
    let p = find_in_pack(&dir, &variant, rel)
        .ok_or_else(|| Error::Pack(format!("unsafe pack path refused: {rel}")))?;
    // Bytes that aren't UTF-8 (a comment in another encoding) don't stop PopTracker
    // either. Bytecode gets no further: Lua refuses it in text mode.
    let bytes = std::fs::read(&p).map_err(|e| Error::Io(format!("{rel}: {}", e.kind())))?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.trim_start_matches('\u{feff}').to_string())
}

/// The live callbacks in a registry list (see `FRAME_HANDLERS`) that `pick`
/// accepts, in the order they were added, copied out first so a callback may
/// change the list.
fn registered(lua: &Lua, key: &str, pick: impl Fn(&Table) -> bool) -> Vec<Function> {
    let Ok(list) = lua.named_registry_value::<Table>(key) else {
        return Vec::new();
    };
    list.sequence_values::<Table>()
        .filter_map(|e| e.ok())
        .filter(|e| !e.get::<bool>("dead").unwrap_or(true) && pick(e))
        .filter_map(|e| e.get::<Function>("fn").ok())
        .collect()
}
