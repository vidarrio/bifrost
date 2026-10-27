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
use std::collections::{HashMap, HashSet};
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

/// A loaded pack. Either a directory on disk or a `.zip` extracted to a temp dir
/// (kept alive by `_tmp` for the pack's lifetime).
pub struct Pack {
    dir: PathBuf,
    _tmp: Option<tempfile::TempDir>,
}
impl Pack {
    /// Load a pack from a directory **or** a `.zip` file (auto-detected).
    pub fn load(path: impl AsRef<Path>) -> Result<Pack> {
        let path = path.as_ref();
        if path.is_dir() {
            Self::from_dir(path.to_path_buf(), None)
        } else {
            Self::load_zip(path)
        }
    }

    /// Extract an untrusted pack zip to a temp dir (with a zip-slip guard) and
    /// locate the pack root (the dir containing `scripts/init.lua`, at the zip
    /// root or one level down — packs are often zipped inside a top folder).
    pub fn load_zip(path: &Path) -> Result<Pack> {
        let tmp = tempfile::TempDir::new().map_err(|e| Error::Io(e.to_string()))?;
        extract_zip(path, tmp.path())?;
        let root = find_pack_root(tmp.path())?;
        Self::from_dir(root, Some(tmp))
    }

    fn from_dir(dir: PathBuf, tmp: Option<tempfile::TempDir>) -> Result<Pack> {
        if !dir.join("scripts/init.lua").is_file() {
            return Err(Error::Pack("missing scripts/init.lua".into()));
        }
        Ok(Pack { dir, _tmp: tmp })
    }

    /// The pack's root directory on disk.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Read a file from inside the pack (pack-relative), e.g. a map image.
    /// Guards against path traversal.
    pub fn asset(&self, rel: &str) -> Option<Vec<u8>> {
        std::fs::read(pack_join(&self.dir, rel)?).ok()
    }
}

/// Resolve a pack-relative path, rejecting traversal (`..`), absolute paths, and
/// Windows separators/roots. This is the guard that keeps every pack-controlled
/// filename — asset requests AND the pack's own Lua (`ScriptHost:LoadScript`,
/// `Tracker:AddItems`/`AddLocations`/`AddMaps`/`AddLayouts`) — inside the pack
/// directory, so a malicious pack can't read arbitrary host files.
fn pack_join(dir: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty()
        || rel.contains("..")
        || rel.starts_with('/')
        || rel.starts_with('\\')
        || rel.contains(':')
    {
        return None;
    }
    Some(dir.join(rel))
}

/// Cap on total bytes written when extracting a pack, so a small "zip bomb"
/// can't fill the host's disk. Real packs are a few MB; this is generous.
const MAX_EXTRACT_BYTES: u64 = 512 * 1024 * 1024;

/// Extract a pack zip into `dest` (with a zip-slip guard). Used both for the
/// temp-dir load path and to install an uploaded pack to permanent storage.
pub fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    extract_zip_capped(zip_path, dest, MAX_EXTRACT_BYTES)
}

fn extract_zip_capped(zip_path: &Path, dest: &Path, cap: u64) -> Result<()> {
    use std::io::Read;
    let file = std::fs::File::open(zip_path).map_err(|e| Error::Io(e.to_string()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| Error::Pack(format!("zip: {e}")))?;
    let mut budget = cap;
    for i in 0..zip.len() {
        let mut f = zip
            .by_index(i)
            .map_err(|e| Error::Pack(format!("zip entry {i}: {e}")))?;
        // enclosed_name() rejects absolute paths and `..` traversal.
        let Some(rel) = f.enclosed_name() else {
            continue;
        };
        let out = dest.join(rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| Error::Io(e.to_string()))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io(e.to_string()))?;
        }
        let mut w = std::fs::File::create(&out).map_err(|e| Error::Io(e.to_string()))?;
        // Copy at most `budget`+1 bytes: the declared size is attacker-controlled,
        // so enforce the real decompressed byte count, not the header's claim.
        let mut limited = (&mut f).take(budget + 1);
        let n = std::io::copy(&mut limited, &mut w).map_err(|e| Error::Io(e.to_string()))?;
        if n > budget {
            return Err(Error::Pack(
                "pack is too large when extracted (possible zip bomb)".into(),
            ));
        }
        budget -= n;
    }
    Ok(())
}

/// Find the pack root under `base`: `base` itself if it has `scripts/init.lua`,
/// else the first immediate subdirectory that does. Public so a caller that
/// extracted a pack with [`extract_zip`] can locate the root to load.
pub fn pack_root(base: &Path) -> Result<PathBuf> {
    find_pack_root(base)
}

/// Find the pack root under `base`: `base` itself if it has `scripts/init.lua`,
/// else the first immediate subdirectory that does.
fn find_pack_root(base: &Path) -> Result<PathBuf> {
    if base.join("scripts/init.lua").is_file() {
        return Ok(base.to_path_buf());
    }
    if let Ok(rd) = std::fs::read_dir(base) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && p.join("scripts/init.lua").is_file() {
                return Ok(p);
            }
        }
    }
    Err(Error::Pack("no scripts/init.lua found in zip".into()))
}

/// The computed tracker state — the crate's entire output. Serializable so the
/// caller can ship it to a front-end that draws the map and item panel.
#[derive(Debug, Default, Serialize)]
pub struct TrackerState {
    pub items: Vec<ItemState>,
    /// Map definitions (background image + how to draw location markers).
    pub maps: Vec<MapInfo>,
    /// Checkable locations with their current accessibility + sections.
    pub locations: Vec<LocationState>,
    /// Flat list of map placements to draw: one per (location × map_location).
    pub markers: Vec<Marker>,
    /// The pack's resolved layout tree (`tracker_default` with `layout` refs
    /// inlined), so the UI can reproduce the pack's tab hierarchy. `null` if the
    /// pack defines no layout (the UI then just lists the maps).
    pub layout: Option<serde_json::Value>,
}
#[derive(Debug, Serialize)]
pub struct ItemState {
    pub key: String,
    pub kind: String,
    pub active: bool,
    pub count: i64,
    pub stage: i64,
    /// All codes this item answers to (so an item-grid cell can look it up).
    pub codes: Vec<String>,
    pub name: String,
    /// Current image to show (stage image for staged items), pack-relative.
    pub img: Option<String>,
    /// Small overlay text (e.g. a consumable's count), if any.
    pub overlay: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct MapInfo {
    pub name: String,
    /// Pack-relative path to the map background image.
    pub img: String,
    /// Default marker size in map pixels (per-marker `size` overrides).
    pub location_size: Option<f64>,
    /// Default marker shape (rect/diamond/…); per-marker `shape` overrides.
    pub location_shape: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct LocationState {
    pub name: String,
    /// Reachable right now (today: reachable/none; richer levels are TODO).
    pub accessible: bool,
    /// Every section's checks have been collected (all sections `checked`), so
    /// PopTracker would render this location as "Done" (grey).
    pub checked: bool,
    pub sections: Vec<SectionState>,
}
#[derive(Debug, Serialize, Clone)]
pub struct SectionState {
    pub name: String,
    pub item_count: i64,
    /// Chests collected so far (item_count - AvailableChestCount, clamped ≥0).
    /// Lets the UI show PopTracker's "cleared/total" counts in the tooltip.
    pub cleared: i64,
    /// True iff `level == "reachable"` (fully in logic). Kept for the season-wide
    /// "in logic" tallies that predate the richer `level`.
    pub accessible: bool,
    /// PopTracker accessibility level: `reachable` (green), `glitch` (yellow,
    /// sequence-break-only via `[..]` rules), `inspect` (blue, viewable-only via
    /// `{..}` rules), or `none` (red, blocked). Bracket-free rules only ever
    /// yield `reachable`/`none`, matching the old boolean behavior.
    pub level: String,
    /// All of this section's chests have been checked (AvailableChestCount <= 0).
    pub checked: bool,
}
/// A single marker to draw on a map image.
#[derive(Debug, Serialize)]
pub struct Marker {
    pub map: String,
    pub x: f64,
    pub y: f64,
    pub size: Option<f64>,
    pub shape: Option<String>,
    pub location: String,
    pub accessible: bool,
    /// The location's checks are all collected → render grey ("Done").
    pub checked: bool,
    /// Per-section state, so the map marker can render PopTracker-style split
    /// colors (mixed in-logic/out-of-logic/collected) and a per-check tooltip.
    pub sections: Vec<SectionState>,
}

/// A quick summary for validating an uploaded pack before accepting it.
#[derive(Debug, Serialize)]
pub struct Summary {
    pub items: usize,
    pub locations: usize,
    pub maps: usize,
    /// True if the pack registered any Archipelago autotracking handlers, i.e.
    /// it can be driven from AP server state. Packs that only autotrack from
    /// emulator memory register none and are unusable server-side.
    pub archipelago_autotracking: bool,
}

#[derive(Clone)]
struct MapDef {
    name: String,
    img: String,
    location_size: Option<f64>,
    location_shape: Option<String>,
}
#[derive(Clone)]
struct MapLoc {
    map: String,
    x: f64,
    y: f64,
    size: Option<f64>,
    shape: Option<String>,
}
#[derive(Clone)]
struct SectionDef {
    name: String,
    item_count: i64,
    rules: Vec<String>, // section-level access_rules (own OR-group)
    vis: Vec<String>,   // section-level visibility_rules (own OR-group); empty = always visible
}
#[derive(Clone)]
struct LocNode {
    name: String,                 // full path, e.g. "Area/Sub/Spot"
    groups: Vec<Vec<String>>, // effective location-level access: parent chain + own; ALL groups must pass
    vis_groups: Vec<Vec<String>>, // effective `visibility_rules`: parent chain + own; ALL must pass for the marker to show
    map_locations: Vec<MapLoc>,
    sections: Vec<SectionDef>,
}

#[derive(Default)]
struct Eng {
    dir: PathBuf,
    code_to_key: HashMap<String, String>,
    kind: HashMap<String, String>,
    /// For a staged item: the codes at each stage (index = stage). A code is
    /// "provided" once the current stage reaches the first stage it appears in,
    /// so a stage-N code isn't granted while the item sits below stage N.
    stage_codes: HashMap<String, Vec<Vec<String>>>,
    /// key -> display name.
    names: HashMap<String, String>,
    /// key -> all codes (for item-grid cell lookup).
    item_codes: HashMap<String, Vec<String>>,
    /// key -> images: stage images for staged items, else a single base image.
    imgs: HashMap<String, Vec<String>>,
    maps: Vec<MapDef>,
    nodes: Vec<LocNode>,
    /// `@`-location path (without the leading `@`) -> total chest count, for
    /// resolving `Tracker:FindObjectForCode("@Area/Sub/Section")` to a section
    /// object the pack's `onLocation` can decrement. Keyed as
    /// `"<node full path>/<section name>"` (section name may be empty → trailing
    /// slash), matching how packs' `LOCATION_MAPPING` address sections.
    loc_counts: HashMap<String, i64>,
    /// Named layout widgets (`layouts/*.json`), e.g. `tracker_default`.
    layouts: HashMap<String, serde_json::Value>,
    /// The slot's valid AP location ids (checked ∪ missing). `None` until the
    /// host feeds them; while `None`, `visibility_rules` are not applied (every
    /// marker shows). Packs gate markers on `visibility_rules` that reference a
    /// location id as a bare code — present iff the id is in this set — so this
    /// is what lets us hide checks a slot's options didn't generate (e.g. the
    /// "Shipped" squares when shipsanity is off), matching native PopTracker.
    valid_locations: Option<HashSet<i64>>,
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
    /// Remaining instruction-hook ticks for the current operation; the debug
    /// hook decrements it and aborts at zero so untrusted pack Lua can't hang
    /// the host. Reset (via [`charge`]) at the start of each public call.
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
            lua.set_hook(
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
            );
        }
        let eng: E = Rc::new(RefCell::new(Eng {
            dir: pack.dir.clone(),
            ..Default::default()
        }));
        lua.load("__ITEMS = {}").exec()?;
        lua.load("__LOCOBJS = {}").exec()?;
        install_host(&lua, &eng)?;

        budget.set(MAX_TICKS);
        let init = read_pack_file(&eng, "scripts/init.lua")?;
        lua.load(&init).set_name("init.lua").exec()?;

        Ok(Session { lua, eng, budget })
    }

    /// Reset the per-operation instruction budget. Call at the start of each
    /// public method that runs pack Lua.
    fn charge(&self) {
        self.budget.set(MAX_TICKS);
    }

    /// Set the slot's AP player number (`Archipelago.PlayerNumber`), which some
    /// packs read to tell local from remote items. Call before feeding state.
    pub fn set_player(&self, n: i64) {
        if let Ok(ap) = self.lua.globals().get::<Table>("Archipelago") {
            let _ = ap.set("PlayerNumber", n);
        }
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
            h.call::<()>(sd.clone())?;
        }
        Ok(())
    }

    /// Fire the pack's Item handlers for one received item.
    pub fn on_item(&self, index: i64, item_id: i64, item_name: &str, player: i64) -> Result<()> {
        self.charge();
        let handlers = self.eng.borrow().item_h.clone();
        for h in handlers {
            h.call::<()>((index, item_id, item_name, player))?;
        }
        Ok(())
    }

    /// Fire the pack's Location handlers for one checked location.
    pub fn on_location(&self, location_id: i64, location_name: &str) -> Result<()> {
        self.charge();
        let handlers = self.eng.borrow().loc_h.clone();
        for h in handlers {
            h.call::<()>((location_id, location_name))?;
        }
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
            h.call::<()>((key, v.clone()))?;
        }
        Ok(())
    }

    /// Snapshot the current tracker state (items + location accessibility).
    pub fn state(&self) -> TrackerState {
        self.charge();
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
                let active = t.get::<bool>("Active").unwrap_or(false);
                let count = t.get::<i64>("AcquiredCount").unwrap_or(0);
                let stage = t.get::<i64>("CurrentStage").unwrap_or(0);
                let e = self.eng.borrow();
                let staged =
                    kind == "progressive" || kind == "progressive_toggle" || kind == "static";
                let img = e.imgs.get(&key).and_then(|v| {
                    let pick = if staged {
                        let i = (stage.max(0) as usize).min(v.len().saturating_sub(1));
                        v.get(i)
                    } else {
                        v.first()
                    }
                    .filter(|s| !s.is_empty());
                    // Fall back to the first non-empty image (e.g. a progressive
                    // item whose stage-0 image is blank) so the cell isn't empty.
                    pick.or_else(|| v.iter().find(|s| !s.is_empty())).cloned()
                });
                // A consumable shows its count; an item is "lit" if active or held.
                let overlay = if kind == "consumable" && count > 0 {
                    Some(count.to_string())
                } else {
                    None
                };
                out.items.push(ItemState {
                    key: key.clone(),
                    kind,
                    active: active || count > 0,
                    count,
                    stage,
                    codes: e.item_codes.get(&key).cloned().unwrap_or_default(),
                    name: e.names.get(&key).cloned().unwrap_or_default(),
                    img,
                    overlay,
                });
            }
        }
        out.items.sort_by(|a, b| a.key.cmp(&b.key));

        // maps
        for m in self.eng.borrow().maps.clone() {
            out.maps.push(MapInfo {
                name: m.name,
                img: m.img,
                location_size: m.location_size,
                location_shape: m.location_shape,
            });
        }

        // locations + markers (from the node model)
        let nodes = self.eng.borrow().nodes.clone();
        // Only enforce `visibility_rules` once the slot's valid-location set is
        // known; otherwise every marker shows (fail-safe, old behavior).
        let enforce_vis = self.eng.borrow().valid_locations.is_some();
        for n in nodes {
            // Hide a location whose visibility rules fail (e.g. options the slot
            // didn't generate) — dropped from both markers and the locations list,
            // just as native PopTracker omits them.
            if enforce_vis
                && !n.vis_groups.is_empty()
                && !eval_groups(&self.lua, &self.eng, &n.vis_groups, 0)
            {
                continue;
            }
            let loc_access = eval_groups(&self.lua, &self.eng, &n.groups, 0);
            let locobjs: Option<Table> = self.lua.globals().get("__LOCOBJS").ok();
            let single = n.sections.len() == 1;
            // The chests still available in a section's chest object (if the pack
            // touched it via `onLocation`). Look under both the `full/section`
            // and bare `full` address forms. `None` => the pack never tracked it.
            let section_avail = |secname: &str| -> Option<i64> {
                let objs = locobjs.as_ref()?;
                let mut keys = vec![format!("{}/{}", n.name, secname)];
                if single || secname.is_empty() {
                    keys.push(n.name.clone());
                }
                keys.iter().find_map(|k| {
                    objs.get::<Table>(k.as_str())
                        .ok()
                        .and_then(|t| t.get::<i64>("AvailableChestCount").ok())
                })
            };
            let sections: Vec<SectionState> = n
                .sections
                .iter()
                // A section can carry its own `visibility_rules` (Stardew uses them
                // at section level, e.g. per Desert Festival stall); drop the ones
                // the slot didn't generate, same as the node-level filter above.
                .filter(|s| {
                    !enforce_vis || s.vis.is_empty() || eval_rules(&self.lua, &self.eng, &s.vis, 0)
                })
                .map(|s| {
                    // section accessible = location groups AND the section's own rules
                    let mut g = n.groups.clone();
                    if !s.rules.is_empty() {
                        g.push(s.rules.clone());
                    }
                    let avail = section_avail(&s.name);
                    let lvl = eval_groups_level(&self.lua, &self.eng, &g, 0);
                    SectionState {
                        name: s.name.clone(),
                        item_count: s.item_count,
                        cleared: avail.map_or(0, |a| (s.item_count - a).max(0)),
                        accessible: lvl == Level::Reachable,
                        level: lvl.as_str().to_string(),
                        checked: avail.is_some_and(|a| a <= 0),
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
                out.markers.push(Marker {
                    map: ml.map.clone(),
                    x: ml.x,
                    y: ml.y,
                    size: ml.size,
                    shape: ml.shape.clone(),
                    location: n.name.clone(),
                    accessible: loc_access,
                    checked: loc_checked,
                    sections: sections.clone(),
                });
            }
            out.locations.push(LocationState {
                name: n.name,
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
            items: e.kind.len(),
            locations: e.nodes.len(),
            maps: e.maps.len(),
            archipelago_autotracking: !(e.clear_h.is_empty()
                && e.item_h.is_empty()
                && e.loc_h.is_empty()
                && e.retr_h.is_empty()
                && e.setr_h.is_empty()),
        }
    }
}

fn read_pack_file(eng: &E, rel: &str) -> Result<String> {
    let dir = eng.borrow().dir.clone();
    let p = pack_join(&dir, rel)
        .ok_or_else(|| Error::Pack(format!("unsafe pack path refused: {rel}")))?;
    std::fs::read_to_string(&p).map_err(|e| Error::Io(format!("{}: {e}", p.display())))
}

// ---- access-rule evaluation --------------------------------------------------

fn provider_count(lua: &Lua, eng: &E, code: &str) -> i64 {
    let code = code.trim().to_lowercase(); // codes are case-insensitive
    let (key, kind, stages) = {
        let e = eng.borrow();
        match e.code_to_key.get(&code) {
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
            // The earliest stage this code appears at; a base (non-stage) code
            // not listed in any stage is treated as stage 0 (always provided
            // while active).
            let idx = stages
                .iter()
                .position(|cs| cs.iter().any(|c| c == &code))
                .unwrap_or(0) as i64;
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
        if let Ok(func) = lua.globals().get::<Function>(name)
            && let Ok(v) = func.call::<Value>(())
        {
            return match v {
                Value::Integer(i) => i >= 1,
                Value::Number(n) => n >= 1.0,
                Value::Boolean(b) => b,
                _ => false,
            };
        }
        return false;
    }
    if let Some(at) = tok.strip_prefix('@') {
        if depth > 32 {
            return false; // crude cycle guard
        }
        // @Location reference: accessible iff that location's groups all pass.
        // Strip a trailing "/Section" if present; we key on the location node.
        let groups = eng
            .borrow()
            .nodes
            .iter()
            .find(|n| n.name == at || n.name.ends_with(at))
            .map(|n| n.groups.clone());
        return match groups {
            Some(g) => eval_groups(lua, eng, &g, depth + 1),
            None => false,
        };
    }
    if let Some((code, cnt)) = tok.split_once(':') {
        let need: i64 = cnt.trim().parse().unwrap_or(1);
        let code = code.trim();
        provider_count(lua, eng, code) >= need || location_present(eng, code)
    } else {
        provider_count(lua, eng, tok) >= 1 || location_present(eng, tok)
    }
}

/// A bare numeric code in a rule (chiefly `visibility_rules`) is an AP location
/// id: the pack treats it as "present" exactly when that location exists for the
/// slot (its `onClear` sets a toggle of that code from `MissingLocations ∪
/// CheckedLocations`). We resolve it directly against the fed valid-location set.
/// Returns false when no set was fed (so visibility stays disabled) or the code
/// isn't a plain integer.
fn location_present(eng: &E, code: &str) -> bool {
    let e = eng.borrow();
    let Some(valid) = e.valid_locations.as_ref() else {
        return false;
    };
    code.parse::<i64>().is_ok_and(|id| valid.contains(&id))
}

fn eval_rule(lua: &Lua, eng: &E, rule: &str, depth: u32) -> bool {
    rule.split(',').all(|t| eval_token(lua, eng, t, depth))
}

/// An access rule list is an OR of rule strings (empty => always true).
fn eval_rules(lua: &Lua, eng: &E, rules: &[String], depth: u32) -> bool {
    if rules.is_empty() {
        return true;
    }
    rules.iter().any(|r| eval_rule(lua, eng, r, depth))
}

/// Effective accessibility = every OR-group passes (parent chain + own + section).
fn eval_groups(lua: &Lua, eng: &E, groups: &[Vec<String>], depth: u32) -> bool {
    groups.iter().all(|g| eval_rules(lua, eng, g, depth))
}

/// PopTracker accessibility levels, worst→best. A rule can downgrade from
/// `Reachable` via two bracket forms: `[token]` is a glitch/sequence-break
/// requirement (missing it → `Glitch`), `{token}` is an inspect requirement
/// (missing it → `Inspect`, i.e. you can see the check but not reach it).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    None,
    Inspect,
    Glitch,
    Reachable,
}
impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::None => "none",
            Level::Inspect => "inspect",
            Level::Glitch => "glitch",
            Level::Reachable => "reachable",
        }
    }
}

/// Split a rule on top-level commas, keeping commas inside `[..]`/`{..}` groups
/// attached to their token.
fn split_rule_tokens(rule: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in rule.chars() {
        match c {
            '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ']' | '}' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Accessibility level of one AND-rule: normal tokens are hard requirements
/// (any failure → `None`); `[..]`/`{..}` tokens downgrade a met rule.
fn eval_rule_level(lua: &Lua, eng: &E, rule: &str, depth: u32) -> Level {
    let (mut normal, mut glitch, mut inspect) = (true, true, true);
    for t in split_rule_tokens(rule) {
        if let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if !eval_token(lua, eng, inner, depth) {
                glitch = false;
            }
        } else if let Some(inner) = t.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            if !eval_token(lua, eng, inner, depth) {
                inspect = false;
            }
        } else if !eval_token(lua, eng, &t, depth) {
            normal = false;
        }
    }
    if !normal {
        return Level::None;
    }
    match (glitch, inspect) {
        (true, true) => Level::Reachable,
        (false, _) => Level::Glitch, // missing a glitch req → sequence-break only
        (true, false) => Level::Inspect, // missing an inspect req → viewable only
    }
}

/// A rule list is an OR: the best level any alternative achieves (empty => Reachable).
fn eval_rules_level(lua: &Lua, eng: &E, rules: &[String], depth: u32) -> Level {
    if rules.is_empty() {
        return Level::Reachable;
    }
    rules
        .iter()
        .map(|r| eval_rule_level(lua, eng, r, depth))
        .max()
        .unwrap_or(Level::None)
}

/// Effective level across every AND-group (parent chain + own + section): the
/// weakest group caps the result.
fn eval_groups_level(lua: &Lua, eng: &E, groups: &[Vec<String>], depth: u32) -> Level {
    groups
        .iter()
        .map(|g| eval_rules_level(lua, eng, g, depth))
        .min()
        .unwrap_or(Level::Reachable)
}

/// Resolve a layout node for the UI: inline `{type:"layout", key:K}` references
/// to the named layout `K`, recursing through `content`/`tabs`. Other fields
/// (type, header, title, maps, orientation, dock…) are kept so the front-end can
/// reproduce the pack's tab hierarchy.
fn resolve_layout(
    layouts: &HashMap<String, serde_json::Value>,
    node: &serde_json::Value,
    depth: u32,
) -> serde_json::Value {
    if depth > 64 {
        return serde_json::Value::Null;
    }
    match node {
        serde_json::Value::Object(o) => {
            if o.get("type").and_then(|v| v.as_str()) == Some("layout") {
                if let Some(target) = o
                    .get("key")
                    .and_then(|v| v.as_str())
                    .and_then(|k| layouts.get(k))
                {
                    return resolve_layout(layouts, target, depth + 1);
                }
                return serde_json::Value::Null;
            }
            let mut m = serde_json::Map::new();
            for (k, v) in o {
                let nv = if k == "content" || k == "tabs" {
                    resolve_layout(layouts, v, depth + 1)
                } else {
                    v.clone()
                };
                m.insert(k.clone(), nv);
            }
            serde_json::Value::Object(m)
        }
        serde_json::Value::Array(a) => serde_json::Value::Array(
            a.iter()
                .map(|x| resolve_layout(layouts, x, depth + 1))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Parse a pack JSON file tolerating JSONC/JSON5 (comments + trailing commas),
/// which PopTracker packs use freely; strict `serde_json` rejects those.
fn parse_pack_json(txt: &str) -> std::result::Result<serde_json::Value, String> {
    json5::from_str::<serde_json::Value>(txt).map_err(|e| e.to_string())
}

/// An item image spec may carry comma-separated image mods (e.g.
/// `images/x.png,@disabled`); the file path is the part before the first comma.
fn img_path(s: &str) -> String {
    s.split(',').next().unwrap_or("").trim().to_string()
}

/// A number that PopTracker may encode as a JSON number or a numeric string.
fn num(v: &serde_json::Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// Read a rules field (`access_rules`/`visibility_rules`). PopTracker allows it
/// as either a single string (one OR-alternative) or an array of strings, so we
/// accept both — a bare string was being dropped, which silently removed gates
/// like a parent's `"access_rules": "bus"`.
fn str_rules(node: &serde_json::Value, field: &str) -> Vec<String> {
    match node.get(field) {
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect(),
        _ => Vec::new(),
    }
}

/// Flatten the location tree into [`LocNode`]s, accumulating the parent chain's
/// access-rule groups so a node's effective accessibility includes its ancestors.
fn flatten_locations(
    eng: &E,
    node: &serde_json::Value,
    prefix: &str,
    parent_groups: &[Vec<String>],
    parent_vis: &[Vec<String>],
) {
    if let Some(arr) = node.as_array() {
        for n in arr {
            flatten_locations(eng, n, prefix, parent_groups, parent_vis);
        }
        return;
    }
    let name = node.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let full = if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    };

    // This node's effective groups = parent chain + its own access_rules.
    let mut groups = parent_groups.to_vec();
    let own = str_rules(node, "access_rules");
    if !own.is_empty() {
        groups.push(own);
    }
    // Visibility inherits the same way (a hidden parent hides its children).
    let mut vis_groups = parent_vis.to_vec();
    let own_vis = str_rules(node, "visibility_rules");
    if !own_vis.is_empty() {
        vis_groups.push(own_vis);
    }

    let map_locations: Vec<MapLoc> = node
        .get("map_locations")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    Some(MapLoc {
                        map: m.get("map")?.as_str()?.to_string(),
                        x: num(m.get("x")?)?,
                        y: num(m.get("y")?)?,
                        size: m.get("size").and_then(num),
                        shape: m.get("shape").and_then(|v| v.as_str().map(String::from)),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let sections: Vec<SectionDef> = node
        .get("sections")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .map(|s| SectionDef {
                    name: s
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    item_count: s.get("item_count").and_then(|v| v.as_i64()).unwrap_or(1),
                    rules: str_rules(s, "access_rules"),
                    vis: str_rules(s, "visibility_rules"),
                })
                .collect()
        })
        .unwrap_or_default();

    // Register each section's chest count under its `@`-path(s), so a checked
    // location can resolve to a decrementable object (see `loc_counts`).
    {
        let mut e = eng.borrow_mut();
        for s in &sections {
            e.loc_counts
                .insert(format!("{full}/{}", s.name), s.item_count);
            // A single-section (or unnamed-section) location is also addressable
            // by the bare location path.
            if sections.len() == 1 || s.name.is_empty() {
                e.loc_counts.insert(full.clone(), s.item_count);
            }
        }
    }

    // A node is a real, drawable/checkable location if it has sections or a map placement.
    if !sections.is_empty() || !map_locations.is_empty() {
        eng.borrow_mut().nodes.push(LocNode {
            name: full.clone(),
            groups: groups.clone(),
            vis_groups: vis_groups.clone(),
            map_locations,
            sections,
        });
    }
    if let Some(children) = node.get("children") {
        flatten_locations(eng, children, &full, &groups, &vis_groups);
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

    // Plain globals the API declares (packs gate on PopVersion; a nil breaks
    // version comparisons like `PopVersion < "x"`).
    g.set("PopVersion", "0.34.1")?;
    g.set("DEBUG", false)?;

    // AutoTracker (memory interface): present but inert — AP packs still probe
    // it (e.g. GetConnectionState). Reads return 0; connection state reads as 3
    // (connected), since from the pack's view we are effectively connected.
    let at = lua.create_table()?;
    for m in ["ReadU8", "ReadU16", "ReadU24", "ReadU32"] {
        at.set(m, lua.create_function(|_, _: MultiValue| Ok(0i64))?)?;
    }
    at.set(
        "ReadVariable",
        lua.create_function(|_, _: MultiValue| Ok(Value::Nil))?,
    )?;
    at.set(
        "GetConnectionState",
        lua.create_function(|_, _: MultiValue| Ok(3i64))?,
    )?;
    at.set("SelectedConnectorType", lua.create_table()?)?;
    g.set("AutoTracker", &at)?;

    // Highlight enum (display only).
    let hl = lua.create_table()?;
    for (k, v) in [
        ("None", 0),
        ("Avoid", 1),
        ("NoPriority", 2),
        ("Unspecified", 3),
        ("Priority", 4),
    ] {
        hl.set(k, v)?;
    }
    g.set("Highlight", &hl)?;

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
                let items: Table = lua.globals().get("__ITEMS")?;
                for item in json.as_array().cloned().unwrap_or_default() {
                    let kind = item
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("toggle")
                        .to_string();
                    let mut all_codes: Vec<String> = Vec::new();
                    let mut stage_codes: Vec<Vec<String>> = Vec::new();
                    let mut imgs: Vec<String> = Vec::new();
                    // Codes are matched case-insensitively in PopTracker (grids
                    // and logic may reference a differently-cased code than the
                    // item declares), so normalise to lowercase everywhere.
                    if let Some(c) = item.get("codes").and_then(|v| v.as_str()) {
                        all_codes.extend(c.split(',').map(|s| s.trim().to_lowercase()));
                    }
                    if let Some(stages) = item.get("stages").and_then(|v| v.as_array()) {
                        for st in stages {
                            if let Some(c) = st.get("codes").and_then(|v| v.as_str()) {
                                let scodes: Vec<String> =
                                    c.split(',').map(|s| s.trim().to_lowercase()).collect();
                                all_codes.extend(scodes.iter().cloned());
                                stage_codes.push(scodes);
                            } else {
                                // keep stage indices aligned even for an imageless/codeless stage
                                stage_codes.push(Vec::new());
                            }
                            // one image per stage (index-aligned), "" if none
                            imgs.push(
                                st.get("img")
                                    .and_then(|v| v.as_str())
                                    .map(img_path)
                                    .unwrap_or_default(),
                            );
                        }
                    }
                    if imgs.is_empty()
                        && let Some(im) = item.get("img").and_then(|v| v.as_str())
                    {
                        imgs.push(img_path(im));
                    }
                    // Unique per item: PopTracker items can share a leading code
                    // (e.g. all "Traveling Merchant" days share "tm"), so keying
                    // by first code would collapse distinct items into one. Use a
                    // monotonic id and map every code to it.
                    let key = format!("__item{}", eng2.borrow().kind.len());
                    // Build the item object the way PopTracker exposes it to Lua:
                    // the mutable state fields plus the declared properties packs
                    // read (Increment/MinCount/MaxCount for consumables, etc.).
                    let geti = |k: &str, d: i64| item.get(k).and_then(|v| v.as_i64()).unwrap_or(d);
                    let t = lua.create_table()?;
                    t.set(
                        "Active",
                        item.get("initial_active_state")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                    )?;
                    t.set("AcquiredCount", geti("initial_quantity", 0))?;
                    t.set("CurrentStage", geti("initial_stage_index", 0))?;
                    t.set("Increment", geti("increment", 1))?;
                    t.set("MinCount", geti("min_quantity", 0))?;
                    t.set("MaxCount", geti("max_quantity", 0))?;
                    t.set("Type", kind.as_str())?;
                    t.set(
                        "Name",
                        item.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                    )?;
                    items.set(key.as_str(), &t)?;
                    let name = item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let mut e = eng2.borrow_mut();
                    for c in &all_codes {
                        e.code_to_key.insert(c.clone(), key.clone());
                    }
                    e.names.insert(key.clone(), name);
                    e.item_codes.insert(key.clone(), all_codes.clone());
                    if !imgs.is_empty() {
                        e.imgs.insert(key.clone(), imgs);
                    }
                    e.kind.insert(key.clone(), kind);
                    if !stage_codes.is_empty() {
                        e.stage_codes.insert(key, stage_codes);
                    }
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
                flatten_locations(&eng2, &json, "", &[], &[]);
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
                    e.maps.push(MapDef {
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
                        location_shape: m
                            .get("location_shape")
                            .and_then(|v| v.as_str().map(String::from)),
                    });
                }
                Ok(true)
            })?,
        )?;
    }
    {
        let eng2 = eng.clone();
        tracker.set(
            "FindObjectForCode",
            lua.create_function(move |lua, (_t, code): (Table, String)| {
                // `@`-codes address a location section; return (creating on first
                // use) a small object with a mutable `AvailableChestCount` that
                // the pack's `onLocation` decrements, so checked locations become
                // observable server-side. Path is case-sensitive (unlike items).
                if let Some(path) = code.strip_prefix('@') {
                    let objs: Table = lua.globals().get("__LOCOBJS")?;
                    if let Ok(t) = objs.get::<Table>(path) {
                        return Ok(Value::Table(t));
                    }
                    let count = eng2.borrow().loc_counts.get(path).copied();
                    return match count {
                        Some(c) => {
                            let t = lua.create_table()?;
                            t.set("AvailableChestCount", c)?;
                            t.set("ChestCount", c)?;
                            t.set("Active", false)?;
                            objs.set(path, &t)?;
                            Ok(Value::Table(t))
                        }
                        None => Ok(Value::Nil),
                    };
                }
                let key = eng2.borrow().code_to_key.get(&code.to_lowercase()).cloned();
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
        "AddMemoryWatch",
        "RemoveMemoryWatch",
        "AddVariableWatch",
        "RemoveVariableWatch",
        "AddWatchForCode",
        "RemoveWatchForCode",
        "AddOnFrameHandler",
        "AddOnLocationSectionChangedHandler",
        "RemoveOnLocationSectionChangedHandler",
        "CreateLuaItem",
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
        "SetNotify",
        "Get",
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
    g.set("Archipelago", &ap)?;

    // AccessibilityLevel enum
    let al = lua.create_table()?;
    for (k, v) in [
        ("None", 0),
        ("Inspect", 1),
        ("SequenceBreak", 2),
        ("Reachable", 3),
        ("Cleared", 4),
    ] {
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
    fn layout_inlines_refs_inside_tabs() {
        let mut layouts = HashMap::new();
        layouts.insert(
            "inner".to_string(),
            serde_json::json!({"type":"map","maps":["m1"]}),
        );
        layouts.insert(
            "tracker_default".to_string(),
            serde_json::json!({"type":"tabbed","tabs":[{"title":"A","content":{"type":"layout","key":"inner"}}]}),
        );
        let r = resolve_layout(&layouts, layouts.get("tracker_default").unwrap(), 0);
        assert_eq!(r["tabs"][0]["content"]["type"], "map");
        assert_eq!(r["tabs"][0]["content"]["maps"][0], "m1");
    }

    #[test]
    fn parses_jsonc_with_comments_and_trailing_commas() {
        // PopTracker packs use JSONC; strict JSON would reject both of these.
        let txt = r#"[
            // a comment
            { "name": "A", "codes": "a", },  /* block */
            { "name": "B", "codes": "b" },
        ]"#;
        let v = parse_pack_json(txt).expect("JSONC should parse");
        assert_eq!(v.as_array().unwrap().len(), 2);
    }

    #[test]
    fn pack_join_blocks_traversal_and_absolute() {
        let d = Path::new("/packs/slot");
        // legitimate pack-relative paths resolve inside the dir
        assert_eq!(
            pack_join(d, "images/map.png"),
            Some(d.join("images/map.png"))
        );
        assert_eq!(
            pack_join(d, "scripts/logic.lua"),
            Some(d.join("scripts/logic.lua"))
        );
        // traversal / absolute / windowsy / empty are refused
        for bad in [
            "..",
            "../etc/passwd",
            "a/../../b",
            "/etc/passwd",
            "\\x",
            "C:/x",
            "",
        ] {
            assert!(pack_join(d, bad).is_none(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn extract_zip_enforces_byte_cap() {
        // A zip whose extracted contents exceed the cap is rejected (zip bomb).
        let tmp = tempfile::TempDir::new().unwrap();
        let zip_path = tmp.path().join("big.zip");
        {
            let mut zw = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
            zw.start_file("big.bin", zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut zw, &vec![0u8; 4096]).unwrap();
            zw.finish().unwrap();
        }
        let dest = tmp.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        // Cap below the 4 KiB payload → rejected; a generous cap → fine.
        assert!(extract_zip_capped(&zip_path, &dest, 1024).is_err());
        assert!(extract_zip_capped(&zip_path, &dest, 1 << 20).is_ok());
    }

    #[test]
    fn sandbox_excludes_dangerous_stdlib() {
        let lua = sandboxed_lua().unwrap();
        let g = lua.globals();
        for banned in [
            "os", "io", "package", "require", "dofile", "loadfile", "debug",
        ] {
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
