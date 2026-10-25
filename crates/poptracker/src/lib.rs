//! PopTracker-pack engine (logic only).
//!
//! Runs a [PopTracker](https://github.com/black-sliver/PopTracker) pack's Lua
//! headlessly against Archipelago state and reports the resulting tracker state:
//! item states and per-location accessibility. There is deliberately **no**
//! rendering, no web, and no network here — a caller (Bifrost) feeds AP state in
//! and turns the returned [`TrackerState`] into whatever UI it wants, and serves
//! the pack's own images. This keeps the crate reusable and unit-testable against
//! a corpus of real packs.
//!
//! ## What works today
//! - Loads a pack's declarative item defs + locations and runs its `init.lua`
//!   (and the scripts it `LoadScript`s, e.g. `autotracking.lua`).
//! - Implements the `Tracker` / `ScriptHost` / `Archipelago` host globals a pack
//!   touches, plus the toggle / consumable / progressive_toggle item model.
//! - Drives the pack via the AP handlers (`on_clear` / `on_item` / `on_location`
//!   / `on_retrieved`) exactly as the live server would.
//! - Evaluates access rules: item codes, `code:count`, OR-of-ANDs, `@location`
//!   references, and the `$luaFunction` escape hatch.
//! - Sandbox: only `string`/`table`/`math`/`coroutine` stdlib is loaded (no
//!   `os`/`io`/`package`/`require`/`debug`), plus a memory cap.
//!
//! ## Not done yet (the long tail — tracked for the real build-out)
//! - Custom item classes (`ScriptHost:CreateLuaItem`) — currently a no-op.
//! - Richer rule DSL: `[optional]` groups, `^`/glitched levels, the full
//!   Inspect/SequenceBreak accessibility ladder (only Reachable/None today).
//! - The reactive recompute loop (`AddWatchForCode`) and renderer data
//!   (maps/coordinates/item layout) for the visual tab.
//! - Memory-autotracking packs (SNES/N64) are inherently unsupported server-side.
//! - CPU/instruction-count limit on untrusted Lua (memory cap only, for now).

use mlua::{Function, Lua, LuaOptions, MultiValue, StdLib, Table, Value};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Memory ceiling for the sandboxed Lua VM (guards runaway allocation).
const LUA_MEMORY_LIMIT: usize = 256 * 1024 * 1024;

#[derive(Debug)]
pub enum Error {
    Io(String),
    Lua(String),
    Json(String),
    Pack(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(s) => write!(f, "io: {s}"),
            Error::Lua(s) => write!(f, "lua: {s}"),
            Error::Json(s) => write!(f, "json: {s}"),
            Error::Pack(s) => write!(f, "pack: {s}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<mlua::Error> for Error {
    fn from(e: mlua::Error) -> Self {
        Error::Lua(e.to_string())
    }
}
pub type Result<T> = std::result::Result<T, Error>;

/// A loaded pack on disk. (Zip support can wrap this by extracting first.)
pub struct Pack {
    dir: PathBuf,
}
impl Pack {
    /// Load a pack from a directory that contains `scripts/init.lua`.
    pub fn load(dir: impl AsRef<Path>) -> Result<Pack> {
        let dir = dir.as_ref().to_path_buf();
        if !dir.join("scripts/init.lua").is_file() {
            return Err(Error::Pack("missing scripts/init.lua".into()));
        }
        Ok(Pack { dir })
    }
}

/// The computed tracker state — the crate's entire output. Serializable so the
/// caller can ship it to a front-end.
#[derive(Debug, Default, Serialize)]
pub struct TrackerState {
    pub items: Vec<ItemState>,
    pub locations: Vec<LocationState>,
}
#[derive(Debug, Serialize)]
pub struct ItemState {
    pub key: String,
    pub kind: String,
    pub active: bool,
    pub count: i64,
    pub stage: i64,
}
#[derive(Debug, Serialize)]
pub struct LocationState {
    pub name: String,
    /// Reachable right now (today: a simple reachable/none; richer levels TODO).
    pub accessible: bool,
    pub rules: Vec<String>,
}

#[derive(Default)]
struct Eng {
    dir: PathBuf,
    code_to_key: HashMap<String, String>,
    kind: HashMap<String, String>,
    stage_codes: HashMap<String, Vec<String>>,
    locations: Vec<(String, Vec<String>)>,
    clear_h: Vec<Function>,
    item_h: Vec<Function>,
    loc_h: Vec<Function>,
    retr_h: Vec<Function>,
    setr_h: Vec<Function>,
}
type E = Rc<RefCell<Eng>>;

/// A live engine session for one pack + one slot.
///
/// Holds a Lua VM, so it is `!Send` — run it on a dedicated/blocking task.
pub struct Session {
    lua: Lua,
    eng: E,
}

impl Session {
    /// Create a session: set up the sandboxed host, then run the pack's
    /// `init.lua` (which loads its items/locations/scripts and registers its
    /// Archipelago handlers).
    pub fn new(pack: &Pack) -> Result<Session> {
        let lua = sandboxed_lua()?;
        let eng: E = Rc::new(RefCell::new(Eng {
            dir: pack.dir.clone(),
            ..Default::default()
        }));
        lua.load("__ITEMS = {}").exec()?;
        install_host(&lua, &eng)?;

        let init = read_pack_file(&eng, "scripts/init.lua")?;
        lua.load(&init).set_name("init.lua").exec()?;

        Ok(Session { lua, eng })
    }

    /// Fire the pack's Clear handlers (called on connect) with `slot_data`.
    pub fn on_clear(&self, slot_data: &serde_json::Value) -> Result<()> {
        let sd = json_to_lua(&self.lua, slot_data)?;
        for h in self.eng.borrow().clear_h.clone() {
            h.call::<()>(sd.clone())?;
        }
        Ok(())
    }

    /// Fire the pack's Item handlers for one received item.
    pub fn on_item(&self, index: i64, item_id: i64, item_name: &str, player: i64) -> Result<()> {
        for h in self.eng.borrow().item_h.clone() {
            h.call::<()>((index, item_id, item_name, player))?;
        }
        Ok(())
    }

    /// Fire the pack's Location handlers for one checked location.
    pub fn on_location(&self, location_id: i64, location_name: &str) -> Result<()> {
        for h in self.eng.borrow().loc_h.clone() {
            h.call::<()>((location_id, location_name))?;
        }
        Ok(())
    }

    /// Fire the pack's Retrieved/SetReply handlers for a data-storage value.
    pub fn on_retrieved(&self, key: &str, val: &serde_json::Value) -> Result<()> {
        let v = json_to_lua(&self.lua, val)?;
        for h in self.eng.borrow().retr_h.iter().chain(self.eng.borrow().setr_h.iter()).cloned() {
            h.call::<()>((key, v.clone()))?;
        }
        Ok(())
    }

    /// Snapshot the current tracker state (items + location accessibility).
    pub fn state(&self) -> TrackerState {
        let mut out = TrackerState::default();
        let items: Table = match self.lua.globals().get("__ITEMS") {
            Ok(t) => t,
            Err(_) => return out,
        };
        let keys: Vec<(String, String)> = self
            .eng
            .borrow()
            .kind
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (key, kind) in keys {
            if let Ok(t) = items.get::<Table>(key.as_str()) {
                out.items.push(ItemState {
                    key: key.clone(),
                    kind,
                    active: t.get::<bool>("Active").unwrap_or(false),
                    count: t.get::<i64>("AcquiredCount").unwrap_or(0),
                    stage: t.get::<i64>("CurrentStage").unwrap_or(0),
                });
            }
        }
        out.items.sort_by(|a, b| a.key.cmp(&b.key));
        let locs = self.eng.borrow().locations.clone();
        for (name, rules) in locs {
            let accessible = eval_rules(&self.lua, &self.eng, &rules, 0);
            out.locations.push(LocationState { name, accessible, rules });
        }
        out
    }
}

fn read_pack_file(eng: &E, rel: &str) -> Result<String> {
    let p = eng.borrow().dir.join(rel);
    std::fs::read_to_string(&p).map_err(|e| Error::Io(format!("{}: {e}", p.display())))
}

// ---- access-rule evaluation --------------------------------------------------

fn provider_count(lua: &Lua, eng: &E, code: &str) -> i64 {
    let (key, kind, stages) = {
        let e = eng.borrow();
        match e.code_to_key.get(code) {
            Some(k) => (
                k.clone(),
                e.kind.get(k).cloned().unwrap_or_default(),
                e.stage_codes.get(k).cloned().unwrap_or_default(),
            ),
            None => return 0,
        }
    };
    let items: Table = match lua.globals().get("__ITEMS") {
        Ok(t) => t,
        Err(_) => return 0,
    };
    let it: Table = match items.get(key.as_str()) {
        Ok(t) => t,
        Err(_) => return 0,
    };
    match kind.as_str() {
        "toggle" => it.get::<bool>("Active").unwrap_or(false) as i64,
        "consumable" => it.get::<i64>("AcquiredCount").unwrap_or(0),
        "progressive_toggle" | "progressive" => {
            let active = it.get::<bool>("Active").unwrap_or(false);
            let stage = it.get::<i64>("CurrentStage").unwrap_or(0);
            let idx = stages.iter().position(|c| c == code).unwrap_or(0) as i64;
            (active && stage >= idx) as i64
        }
        _ => 0,
    }
}

fn eval_token(lua: &Lua, eng: &E, tok: &str, depth: u32) -> bool {
    let tok = tok.trim();
    if tok.is_empty() {
        return true;
    }
    if let Some(f) = tok.strip_prefix('$') {
        let name = f.split(':').next().unwrap_or(f);
        if let Ok(func) = lua.globals().get::<Function>(name) {
            if let Ok(v) = func.call::<Value>(()) {
                return match v {
                    Value::Integer(i) => i >= 1,
                    Value::Number(n) => n >= 1.0,
                    Value::Boolean(b) => b,
                    _ => false,
                };
            }
        }
        return false;
    }
    if let Some(at) = tok.strip_prefix('@') {
        if depth > 32 {
            return false; // crude cycle guard
        }
        let rules = eng
            .borrow()
            .locations
            .iter()
            .find(|(n, _)| n == at || n.ends_with(at))
            .map(|(_, r)| r.clone());
        return match rules {
            Some(r) => eval_rules(lua, eng, &r, depth + 1),
            None => false,
        };
    }
    if let Some((code, cnt)) = tok.split_once(':') {
        let need: i64 = cnt.trim().parse().unwrap_or(1);
        provider_count(lua, eng, code.trim()) >= need
    } else {
        provider_count(lua, eng, tok) >= 1
    }
}

fn eval_rule(lua: &Lua, eng: &E, rule: &str, depth: u32) -> bool {
    rule.split(',').all(|t| eval_token(lua, eng, t, depth))
}

fn eval_rules(lua: &Lua, eng: &E, rules: &[String], depth: u32) -> bool {
    if rules.is_empty() {
        return true;
    }
    rules.iter().any(|r| eval_rule(lua, eng, r, depth))
}

fn flatten_locations(eng: &E, node: &serde_json::Value, prefix: &str) {
    if let Some(arr) = node.as_array() {
        for n in arr {
            flatten_locations(eng, n, prefix);
        }
        return;
    }
    let name = node.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let full = if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    };
    if node.get("sections").is_some() {
        let rules: Vec<String> = node
            .get("access_rules")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        eng.borrow_mut().locations.push((full.clone(), rules));
    }
    if let Some(children) = node.get("children") {
        flatten_locations(eng, children, &full);
    }
}

// ---- serde_json -> Lua bridge (slot_data / data-storage values) --------------

fn json_to_lua(lua: &Lua, v: &serde_json::Value) -> Result<Value> {
    Ok(match v {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else {
                Value::Number(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::String(lua.create_string(s)?),
        serde_json::Value::Array(a) => {
            let t = lua.create_table()?;
            for (i, e) in a.iter().enumerate() {
                t.set(i as i64 + 1, json_to_lua(lua, e)?)?;
            }
            Value::Table(t)
        }
        serde_json::Value::Object(o) => {
            let t = lua.create_table()?;
            for (k, e) in o {
                t.set(k.as_str(), json_to_lua(lua, e)?)?;
            }
            Value::Table(t)
        }
    })
}

// ---- host API ----------------------------------------------------------------

fn install_host(lua: &Lua, eng: &E) -> Result<()> {
    let g = lua.globals();

    // Tracker
    let tracker = lua.create_table()?;
    {
        let eng2 = eng.clone();
        tracker.set(
            "AddItems",
            lua.create_function(move |lua, (_t, file): (Table, String)| {
                let txt = read_pack_file(&eng2, &file).map_err(to_lua)?;
                let json: serde_json::Value = serde_json::from_str(&txt).map_err(|e| mlua::Error::runtime(format!("AddItems json: {e}")))?;
                let items: Table = lua.globals().get("__ITEMS")?;
                for item in json.as_array().cloned().unwrap_or_default() {
                    let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("toggle").to_string();
                    let mut all_codes: Vec<String> = Vec::new();
                    let mut stage_codes: Vec<String> = Vec::new();
                    if let Some(c) = item.get("codes").and_then(|v| v.as_str()) {
                        all_codes.extend(c.split(',').map(|s| s.trim().to_string()));
                    }
                    if let Some(stages) = item.get("stages").and_then(|v| v.as_array()) {
                        for st in stages {
                            if let Some(c) = st.get("codes").and_then(|v| v.as_str()) {
                                let first = c.split(',').next().unwrap_or("").trim().to_string();
                                stage_codes.push(first);
                                all_codes.extend(c.split(',').map(|s| s.trim().to_string()));
                            }
                        }
                    }
                    let key = all_codes.first().cloned().unwrap_or_else(|| {
                        item.get("name").and_then(|v| v.as_str()).unwrap_or("?").to_string()
                    });
                    let t = lua.create_table()?;
                    t.set("Active", false)?;
                    t.set("AcquiredCount", 0)?;
                    t.set("CurrentStage", 0)?;
                    items.set(key.as_str(), &t)?;
                    let mut e = eng2.borrow_mut();
                    for c in &all_codes {
                        e.code_to_key.insert(c.clone(), key.clone());
                    }
                    e.kind.insert(key.clone(), kind);
                    if !stage_codes.is_empty() {
                        e.stage_codes.insert(key, stage_codes);
                    }
                }
                Ok(())
            })?,
        )?;
    }
    {
        let eng2 = eng.clone();
        tracker.set(
            "AddLocations",
            lua.create_function(move |_lua, (_t, file): (Table, String)| {
                let txt = read_pack_file(&eng2, &file).map_err(to_lua)?;
                let json: serde_json::Value = serde_json::from_str(&txt).map_err(|e| mlua::Error::runtime(format!("AddLocations json: {e}")))?;
                flatten_locations(&eng2, &json, "");
                Ok(())
            })?,
        )?;
    }
    {
        let eng2 = eng.clone();
        tracker.set(
            "FindObjectForCode",
            lua.create_function(move |lua, (_t, code): (Table, String)| {
                let key = eng2.borrow().code_to_key.get(&code).cloned();
                match key {
                    Some(k) => {
                        let items: Table = lua.globals().get("__ITEMS")?;
                        items.get::<Value>(k.as_str())
                    }
                    None => Ok(Value::Nil),
                }
            })?,
        )?;
    }
    {
        let eng2 = eng.clone();
        tracker.set(
            "ProviderCountForCode",
            lua.create_function(move |lua, (_t, code): (Table, String)| {
                Ok(provider_count(lua, &eng2, &code))
            })?,
        )?;
    }
    for noop in ["AddLayouts", "AddMaps", "AddClasses", "UiHint", "OpenLink"] {
        tracker.set(noop, lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    }
    g.set("Tracker", &tracker)?;

    // ScriptHost
    let sh = lua.create_table()?;
    {
        let eng2 = eng.clone();
        sh.set(
            "LoadScript",
            lua.create_function(move |lua, (_t, file): (Table, String)| {
                let txt = read_pack_file(&eng2, &file).map_err(to_lua)?;
                lua.load(&txt).set_name(file).exec()
            })?,
        )?;
    }
    for noop in [
        "AddMemoryWatch", "RemoveMemoryWatch", "AddVariableWatch", "RemoveVariableWatch",
        "AddWatchForCode", "RemoveWatchForCode", "AddOnFrameHandler", "AddOnLocationSectionChangedHandler",
        "RemoveOnLocationSectionChangedHandler", "CreateLuaItem",
    ] {
        sh.set(noop, lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    }
    g.set("ScriptHost", &sh)?;

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
    for noop in [
        "SetNotify", "Get", "LocationChecks", "LocationScouts", "HintItem", "HintLocation",
        "StatusUpdate", "AddScoutHandler", "AddBouncedHandler",
    ] {
        ap.set(noop, lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    }
    g.set("Archipelago", &ap)?;

    // AccessibilityLevel enum
    let al = lua.create_table()?;
    for (k, v) in [("None", 0), ("Inspect", 1), ("SequenceBreak", 2), ("Reachable", 3), ("Cleared", 4)] {
        al.set(k, v)?;
    }
    g.set("AccessibilityLevel", &al)?;

    Ok(())
}

fn to_lua(e: Error) -> mlua::Error {
    mlua::Error::runtime(e.to_string())
}

/// Build the sandboxed Lua VM: only string/table/math/coroutine stdlib (base
/// functions like `print`/`pairs` are always present), i.e. no `os`/`io`/
/// `package`/`require`/`debug`, with a memory cap. Pack Lua runs here, so this
/// is the boundary that keeps an uploaded pack from touching the host.
fn sandboxed_lua() -> Result<Lua> {
    let lua = Lua::new_with(
        StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::COROUTINE,
        LuaOptions::default(),
    )?;
    let _ = lua.set_memory_limit(LUA_MEMORY_LIMIT);
    // `dofile`/`loadfile` are BASE-library functions in Lua 5.4 (not `package`),
    // so excluding the package lib doesn't drop them — and they read+run files
    // off disk. Remove them explicitly; they're the obvious filesystem escape.
    {
        let g = lua.globals();
        for f in ["dofile", "loadfile"] {
            let _ = g.set(f, Value::Nil);
        }
    }
    Ok(lua)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_excludes_dangerous_stdlib() {
        let lua = sandboxed_lua().unwrap();
        let g = lua.globals();
        for banned in ["os", "io", "package", "require", "dofile", "loadfile", "debug"] {
            assert!(
                matches!(g.get::<Value>(banned).unwrap(), Value::Nil),
                "{banned} must be unavailable to pack Lua"
            );
        }
        for allowed in ["string", "table", "math", "print", "pairs", "type"] {
            assert!(
                !matches!(g.get::<Value>(allowed).unwrap(), Value::Nil),
                "{allowed} should be present"
            );
        }
    }
}
