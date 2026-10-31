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
//!   (and the scripts it `LoadScript`s or `require`s, e.g. `autotracking.lua`;
//!   `require` only finds modules inside the pack).
//! - Implements the `Tracker` / `ScriptHost` / `Archipelago` host globals a pack
//!   touches, plus the toggle / consumable / progressive_toggle item model.
//! - Drives the pack via the AP handlers (`on_clear` / `on_item` / `on_location`
//!   / `on_retrieved`) exactly as the live server would.
//! - Evaluates access rules: item codes, `code:count`, OR-of-ANDs, `@location`
//!   references, and the `$luaFunction|arg|…` escape hatch.
//! - Custom items (`ScriptHost:CreateLuaItem`), as state `FindObjectForCode`
//!   can find again; frame handlers (run after init, after each clear and
//!   before each state) and "*" code watches (run before each state).
//! - Sandbox: only `string`/`table`/`math`/`coroutine`/`utf8` stdlib is loaded,
//!   plus `os.clock`/`date`/`difftime`/`time` (no `io`/`package`/`debug`, the rest
//!   of `os` or Lua's own `require`), plus a memory cap.
//!
//! ## Not done yet (the long tail — tracked for the real build-out)
//! - Custom items' clicks, icons and `ProvidesCodeFunc` (they don't count
//!   toward access rules), and watches on single codes.
//! - Richer rule DSL: `[optional]` groups, `^`/glitched levels, the full
//!   Inspect/SequenceBreak accessibility ladder (only Reachable/None today).
//! - The reactive recompute loop and renderer data
//!   (maps/coordinates/item layout) for the visual tab.
//! - Memory-autotracking packs (SNES/N64) are inherently unsupported server-side.

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
    /// The variant in use (see [`pick_variant`]); empty if the pack has none.
    variant: String,
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
        let variant = pick_variant(&dir);
        Ok(Pack {
            dir,
            variant,
            _tmp: tmp,
        })
    }

    /// The pack's root directory on disk.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Read a file from inside the pack (pack-relative), e.g. a map image.
    /// Guards against path traversal.
    pub fn asset(&self, rel: &str) -> Option<Vec<u8>> {
        std::fs::read(find_in_pack(&self.dir, &self.variant, rel)?).ok()
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

/// Find a pack file as PopTracker does: in the variant's folder, then the pack's
/// root (a path may start with `/`, meaning the same).
fn find_in_pack(dir: &Path, variant: &str, rel: &str) -> Option<PathBuf> {
    let rel = rel.trim_start_matches('/');
    if !variant.is_empty()
        && let Some(p) = pack_join(dir, &format!("{variant}/{rel}"))
        && p.is_file()
    {
        return Some(p);
    }
    pack_join(dir, rel)
}

/// The most of a manifest that's read. Packs load in the server too, to serve
/// their images, so this stays cheap whatever a pack ships.
const MAX_MANIFEST: u64 = 1 << 20;

/// Which of a pack's variants to run: there's no one here to choose, so an
/// Archipelago one with a map rather than items only, `standard` if that fits,
/// else the first by name. Empty if the manifest names none.
fn pick_variant(dir: &Path) -> String {
    let path = dir.join("manifest.json");
    let txt = match std::fs::metadata(&path) {
        Ok(m) if m.len() <= MAX_MANIFEST => std::fs::read_to_string(&path).unwrap_or_default(),
        _ => String::new(),
    };
    let Ok(manifest) = parse_pack_json(txt.trim_start_matches('\u{feff}')) else {
        return String::new();
    };
    let Some(variants) = manifest.get("variants").and_then(|v| v.as_object()) else {
        return String::new();
    };
    let items_only = |uid: &str, v: &serde_json::Value| {
        let name = v.get("display_name").and_then(|n| n.as_str()).unwrap_or("");
        let plain = |s: &str| s.to_lowercase().replace([' ', '_', '-'], "");
        plain(uid).contains("itemsonly") || plain(name).contains("itemsonly")
    };
    let ap = |v: &serde_json::Value| {
        v.get("flags")
            .and_then(|f| f.as_array())
            .is_some_and(|f| f.iter().any(|x| x.as_str() == Some("ap")))
    };
    let usable = |uid: &&String| pack_join(dir, uid).is_some();
    let score = |uid: &String, v: &serde_json::Value| {
        (ap(v), !items_only(uid, v), uid.as_str() == "standard")
    };
    let mut best: Option<(&String, (bool, bool, bool))> = None;
    // Names in order, so an earlier one wins a tie.
    for (uid, v) in variants.iter().filter(|(uid, _)| usable(uid)) {
        let sc = score(uid, v);
        if best.as_ref().is_none_or(|(_, b)| sc > *b) {
            best = Some((uid, sc));
        }
    }
    best.map(|(uid, _)| uid.clone()).unwrap_or_default()
}

/// A `require` module name as a pack-relative path without the extension
/// ("logic.base" or "logic/base" → "logic/base"). Names with empty parts, or
/// anything `pack_join` would refuse, aren't modules.
fn module_path(name: &str) -> Option<String> {
    let rel = name.replace('.', "/");
    if rel.split('/').any(|p| p.is_empty()) {
        return None;
    }
    pack_join(Path::new(""), &rel).map(|_| rel)
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

/// Cap on total bytes written when extracting a pack, so a small "zip bomb"
/// can't fill the host's disk. Real packs are a few MB; this is generous.
const MAX_EXTRACT_BYTES: u64 = 512 * 1024 * 1024;

/// Caps on a pack's files and directories together, and on how deep they go, for
/// the same reason (they also bound how long extracting can take).
const MAX_EXTRACT_FILES: usize = 100_000;
const MAX_EXTRACT_DEPTH: usize = 32;

/// Extract a pack zip into `dest` (with a zip-slip guard). Used both for the
/// temp-dir load path and to install an uploaded pack to permanent storage.
pub fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    extract_zip_capped(zip_path, dest, MAX_EXTRACT_BYTES)
}

fn extract_zip_capped(zip_path: &Path, dest: &Path, cap: u64) -> Result<()> {
    use std::io::Read;
    let file = std::fs::File::open(zip_path).map_err(|e| Error::Io(e.to_string()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| Error::Pack(format!("zip: {e}")))?;
    if zip.len() > MAX_EXTRACT_FILES {
        return Err(Error::Pack("pack has too many files".into()));
    }
    let mut budget = cap;
    let mut made = HashSet::new();
    for i in 0..zip.len() {
        let mut f = zip
            .by_index(i)
            .map_err(|e| Error::Pack(format!("zip entry {i}: {e}")))?;
        // enclosed_name() rejects absolute paths and `..` traversal.
        let Some(rel) = f.enclosed_name() else {
            continue;
        };
        if rel.components().count() > MAX_EXTRACT_DEPTH {
            return Err(Error::Pack("pack's folders are too deep".into()));
        }
        // The entry itself and every folder it brings: a zip needn't list them.
        for p in rel.ancestors().filter(|p| !p.as_os_str().is_empty()) {
            // A hash: a pack's long names shouldn't all be kept in memory.
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(p, &mut h);
            if !made.insert(std::hash::Hasher::finish(&h)) {
                break;
            }
            if made.len() > MAX_EXTRACT_FILES {
                return Err(Error::Pack("pack has too many files".into()));
            }
        }
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
    /// Errors from the pack's handlers: the first few, and how many in all.
    /// Tracking carried on past them, but may be incomplete.
    pub pack_errors: Vec<String>,
    pub pack_error_count: usize,
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
#[derive(Debug, Serialize, serde::Deserialize)]
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
    variant: String,
    /// The first few errors from the pack's handlers, and how many in all.
    pack_errors: Vec<String>,
    pack_error_count: usize,
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

/// Registry keys of the tables holding `ScriptHost:AddOnFrameHandler`
/// callbacks (name → function) and `ScriptHost:AddWatchForCode` ones (name →
/// `{code, fn}`). They live in Lua, so the memory cap counts them. There are
/// no frames here: frame handlers run once after `init.lua`, after each clear
/// and before each `state`. Watches on every code ("*") run before each
/// `state`, since anything may have changed by then.
const FRAME_HANDLERS: &str = "bifrost.frame_handlers";
const CODE_WATCHES: &str = "bifrost.code_watches";

/// Registry key of the players table (slot → `{name, game}`), filled by
/// `Session::set_multiworld`.
const PLAYERS: &str = "bifrost.players";

/// Registry key of the metatable JSON items share for their methods.
const ITEM_METHODS: &str = "bifrost.item_methods";

/// How deep `FindObjectForCode` may nest through custom items'
/// `CanProvideCodeFunc`s that look up other codes.
const MAX_LOOKUP_DEPTH: u32 = 16;

/// How many of a pack's handler errors are kept, and how much of each.
const MAX_PACK_ERRORS: usize = 5;
const MAX_PACK_ERROR_LEN: usize = 500;
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
            ..Default::default()
        }));
        lua.load("__ITEMS = {}").exec()?;
        lua.load("__LOCOBJS = {}").exec()?;
        lua.load("__LUAITEMS = {}").exec()?;
        install_host(&lua, &eng)?;

        budget.set(MAX_TICKS);
        let init = read_pack_file(&eng, "scripts/init.lua")?;
        run_pack_source(&lua, &init, "init.lua")?;

        let s = Session { lua, eng, budget };
        s.run_frame_handlers();
        Ok(s)
    }

    /// As in PopTracker, an error in one of the pack's handlers is noted and the
    /// tracker carries on, as other items and locations can still track. Only
    /// running out of budget or memory ends the operation: a pack can't go on.
    fn guard(&self, r: mlua::Result<()>) -> Result<()> {
        let Err(e) = r else { return Ok(()) };
        let text = e.to_string();
        if self.budget.get() == 0 || text.contains("not enough memory") {
            return Err(e.into());
        }
        let mut eng = self.eng.borrow_mut();
        eng.pack_error_count += 1;
        if eng.pack_errors.len() < MAX_PACK_ERRORS {
            let end = (0..=text.len().min(MAX_PACK_ERROR_LEN))
                .rev()
                .find(|i| text.is_char_boundary(*i))
                .unwrap_or(0);
            eng.pack_errors.push(text[..end].to_string());
        }
        Ok(())
    }

    /// Run the frame handlers once (see `FRAME_HANDLERS`), with no time
    /// elapsed. A handler may remove itself or add others; those added run next
    /// time. As in PopTracker, a handler's error doesn't stop the tracker.
    fn run_frame_handlers(&self) {
        for f in registered(&self.lua, FRAME_HANDLERS, |_| true) {
            let _ = self.guard(f.call::<()>(0.0));
        }
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
        self.run_frame_handlers();
        Ok(())
    }

    /// Fire the pack's Item handlers for one received item.
    pub fn on_item(&self, index: i64, item_id: i64, item_name: &str, player: i64) -> Result<()> {
        self.charge();
        let handlers = self.eng.borrow().item_h.clone();
        for h in handlers {
            self.guard(h.call::<()>((index, item_id, item_name, player)))?;
        }
        Ok(())
    }

    /// Fire the pack's Location handlers for one checked location.
    pub fn on_location(&self, location_id: i64, location_name: &str) -> Result<()> {
        self.charge();
        let handlers = self.eng.borrow().loc_h.clone();
        for h in handlers {
            self.guard(h.call::<()>((location_id, location_name)))?;
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
            self.guard(h.call::<()>((key, v.clone())))?;
        }
        Ok(())
    }

    /// Snapshot the current tracker state (items + location accessibility).
    pub fn state(&self) -> TrackerState {
        self.charge();
        self.run_frame_handlers();
        for w in registered(&self.lua, CODE_WATCHES, |e| {
            e.get::<mlua::LuaString>("code")
                .is_ok_and(|c| c.as_bytes().as_ref() == b"*")
        }) {
            let _ = self.guard(w.call::<()>("*"));
        }
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
                let count = lua_int(&t, "AcquiredCount").unwrap_or(0);
                let stage = lua_int(&t, "CurrentStage").unwrap_or(0);
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
                let overlay = t
                    .get::<Option<String>>("__overlay")
                    .unwrap_or(None)
                    .or_else(|| (kind == "consumable" && count > 0).then(|| count.to_string()));
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
        {
            let e = self.eng.borrow();
            out.pack_errors = e.pack_errors.clone();
            out.pack_error_count = e.pack_error_count;
        }

        // maps
        let maps = self.eng.borrow().maps.clone();
        for m in maps {
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
                        .and_then(|t| lua_int(&t, "AvailableChestCount"))
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
    let (dir, variant) = {
        let e = eng.borrow();
        (e.dir.clone(), e.variant.clone())
    };
    let p = find_in_pack(&dir, &variant, rel)
        .ok_or_else(|| Error::Pack(format!("unsafe pack path refused: {rel}")))?;
    // Bytes that aren't UTF-8 (a comment in another encoding) don't stop PopTracker
    // either. Bytecode gets no further: Lua refuses it in text mode.
    let bytes = std::fs::read(&p).map_err(|e| Error::Io(format!("{}: {e}", p.display())))?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.trim_start_matches('\u{feff}').to_string())
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
        "consumable" => lua_int(&it, "AcquiredCount").unwrap_or(0),
        "progressive_toggle" | "progressive" => {
            let active = it.get::<bool>("Active").unwrap_or(false);
            let stage = lua_int(&it, "CurrentStage").unwrap_or(0);
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
        // `$name|arg|…`: the function gets the arguments as strings.
        let mut parts = f.split('|');
        let name = parts.next().unwrap_or(f);
        let name = name.split(':').next().unwrap_or(name);
        let args: Vec<&str> = parts.collect();
        if let Ok(func) = lua.globals().get::<Function>(name)
            && let Ok(v) = func.call::<Value>(MultiValue::from_iter(
                args.iter()
                    .filter_map(|a| lua.create_string(a).ok().map(Value::String)),
            ))
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

    // ImageReference: images are only drawn from item and location JSON here,
    // so a reference is just its pack path.
    let img = lua.create_table()?;
    img.set(
        "FromPackRelativePath",
        lua.create_function(|_, (_t, path): (Table, String)| Ok(path))?,
    )?;
    img.set(
        "FromImageReference",
        lua.create_function(|_, (_t, r, _mods): (Table, Value, MultiValue)| Ok(r))?,
    )?;
    g.set("ImageReference", &img)?;

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
                    t.set_metatable(Some(lua.named_registry_value(ITEM_METHODS)?))?;
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
        let depth = Rc::new(std::cell::Cell::new(0u32));
        tracker.set(
            "FindObjectForCode",
            lua.create_function(move |lua, (_t, code): (Table, Option<String>)| {
                let Some(code) = code else {
                    return Ok(Value::Nil);
                };
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
                            t.set("FullID", path)?;
                            t.set("Owner", lua.create_table()?)?;
                            objs.set(path, &t)?;
                            Ok(Value::Table(t))
                        }
                        None => Ok(Value::Nil),
                    };
                }
                let key = eng2.borrow().code_to_key.get(&code.to_lowercase()).cloned();
                if let Some(k) = key {
                    let items: Table = lua.globals().get("__ITEMS")?;
                    return items.get::<Value>(k.as_str());
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
                        if let Ok(Value::Function(f)) = item.get::<Value>("CanProvideCodeFunc")
                            && f.call::<Value>((&item, code.as_str()))
                                .is_ok_and(|v| !matches!(v, Value::Nil | Value::Boolean(false)))
                        {
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
                Ok(code.map_or(0, |c| provider_count(lua, &eng2, &c)))
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

    // ScriptHost
    let sh = lua.create_table()?;
    {
        let eng2 = eng.clone();
        sh.set(
            "LoadScript",
            // Some packs load a script they can only name once connected.
            lua.create_function(move |lua, (_t, file): (Table, Option<String>)| {
                let Some(file) = file else { return Ok(false) };
                let txt = read_pack_file(&eng2, &file).map_err(to_lua)?;
                run_pack_source(lua, &txt, &file).map(|()| true)
            })?,
        )?;
    }
    for noop in [
        "AddMemoryWatch",
        "RemoveMemoryWatch",
        "AddVariableWatch",
        "RemoveVariableWatch",
        "AddOnLocationSectionChangedHandler",
        "RemoveOnLocationSectionChangedHandler",
        "RemoveOnLocationSectionHandler",
    ] {
        sh.set(noop, lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    }
    // Frame handlers and code watches, by name, in the order they were added
    // (see `FRAME_HANDLERS`). Each list holds `{name, fn, code}` entries; a
    // removed or replaced one is marked dead, so both stay O(1).
    let make: Function = lua
        .load(
            r##"
            return function(list)
              local by_name = {}
              local function remove(_, name)
                local e = by_name[name]
                if not e then return false end
                e.dead = true
                by_name[name] = nil
                return true
              end
              local function add(_, name, a, b)
                remove(nil, name)
                local e = b == nil and { name = name, fn = a }
                  or { name = name, code = a, fn = b }
                by_name[name] = e
                list[#list + 1] = e
                return name
              end
              return add, remove
            end
            "##,
        )
        .set_name("host")
        .set_mode(mlua::chunk::ChunkMode::Text)
        .eval()?;
    for (key, add, remove) in [
        (FRAME_HANDLERS, "AddOnFrameHandler", "RemoveOnFrameHandler"),
        (CODE_WATCHES, "AddWatchForCode", "RemoveWatchForCode"),
    ] {
        let list = lua.create_table()?;
        lua.set_named_registry_value(key, &list)?;
        let (a, r): (Function, Function) = make.call(list)?;
        sh.set(add, a)?;
        sh.set(remove, r)?;
    }
    // Scripts "in the background" run here and now, under the same limits; there
    // are no threads and no one to notify.
    {
        let eng2 = eng.clone();
        let finish = |lua: &Lua, src: String, name: String, arg: Value, done: Option<Function>| {
            let chunk = lua
                .load(src)
                .set_name(name)
                .set_mode(mlua::chunk::ChunkMode::Text);
            lua.globals().set("arg", arg)?;
            let result = chunk.eval::<Value>();
            lua.globals().set("arg", Value::Nil)?;
            if let (Ok(v), Some(done)) = (result, done) {
                done.call::<()>(v)?;
            }
            Ok(Value::Nil)
        };
        sh.set(
            "RunScriptAsync",
            lua.create_function(
                move |lua,
                      (_t, file, arg, done, _p): (
                    Table,
                    String,
                    Value,
                    Option<Function>,
                    Value,
                )| {
                    let txt = read_pack_file(&eng2, &file).map_err(to_lua)?;
                    finish(lua, txt, file, arg, done)
                },
            )?,
        )?;
        sh.set(
            "RunStringAsync",
            lua.create_function(
                move |lua, (_t, src, arg, done, _p): (Table, String, Value, Option<Function>, Value)| {
                    finish(lua, src, "script".to_string(), arg, done)
                },
            )?,
        )?;
    }
    for noop in ["AsyncProgress", "PushMarkdownNotification"] {
        sh.set(noop, lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    }
    g.set("ScriptHost", &sh)?;

    // Custom items: a table the pack fills in, which `FindObjectForCode` can
    // find again. `:Set`/`:Get` keep values in its `ItemState`; overlays,
    // clicks and icons aren't drawn here.
    // What JSON items share: the overlay text shows, the rest isn't drawn here.
    lua.load(
        r##"
        local function noop() end
        return { __index = {
          SetOverlay = function(self, text) rawset(self, "__overlay", text) end,
          SetOverlayBackground = noop,
          SetOverlayColor = noop,
          SetOverlayFontSize = noop,
          SetOverlayAlign = noop,
        } }
        "##,
    )
    .set_name("host")
    .set_mode(mlua::chunk::ChunkMode::Text)
    .eval::<Table>()
    .and_then(|m| lua.set_named_registry_value(ITEM_METHODS, m))?;
    lua.load(
        r##"
        local items = __LUAITEMS
        local function noop() end
        ScriptHost.CreateLuaItem = function()
          local item = { Type = "custom", ItemState = {} }
          item.Set = function(self, k, v)
            local old = self.ItemState[k]
            self.ItemState[k] = v
            if old ~= v and type(self.PropertyChangedFunc) == "function" then
              self:PropertyChangedFunc(k, v, old)
            end
          end
          item.Get = function(self, k) return self.ItemState[k] end
          item.SetOverlay = noop
          item.SetOverlayBackground = noop
          item.SetOverlayColor = noop
          item.SetOverlayFontSize = noop
          item.SetOverlayAlign = noop
          items[#items + 1] = item
          return item
        end
        "##,
    )
    .set_name("host")
    .set_mode(mlua::chunk::ChunkMode::Text)
    .exec()?;

    // `require`, as PopTracker has it (0.25.6+): modules come only from the pack.
    // "foo.baz" tries scripts/foo/baz.lua, scripts/foo/baz/init.lua, foo/baz.lua
    // and foo/baz/init.lua; each file runs once, with the module name as `...`,
    // and its result is cached.
    {
        let eng2 = eng.clone();
        let loaded = lua.create_table()?;
        let loading = lua.create_table()?;
        let std_libs = lua.create_table()?;
        for lib in ["os", "string", "table", "math", "coroutine", "utf8"] {
            std_libs.set(lib, g.get::<Value>(lib)?)?;
        }
        g.set(
            "require",
            lua.create_function(move |lua, name: String| {
                // The libraries packs have, as they are here: `os` is the small one.
                let lib: Value = std_libs.get(name.as_str())?;
                if !lib.is_nil() {
                    return Ok(lib);
                }
                let rel = module_path(&name).ok_or_else(|| {
                    mlua::Error::runtime(format!("require: bad module name '{name}'"))
                })?;
                let (dir, variant) = {
                    let e = eng2.borrow();
                    (e.dir.clone(), e.variant.clone())
                };
                // PopTracker tries `.LUA` before `.lua` for each.
                let file = [
                    format!("scripts/{rel}.LUA"),
                    format!("scripts/{rel}.lua"),
                    format!("scripts/{rel}/init.LUA"),
                    format!("scripts/{rel}/init.lua"),
                    format!("{rel}.LUA"),
                    format!("{rel}.lua"),
                    format!("{rel}/init.LUA"),
                    format!("{rel}/init.lua"),
                ]
                .into_iter()
                .find(|f| find_in_pack(&dir, &variant, f).is_some_and(|p| p.is_file()))
                .ok_or_else(|| {
                    mlua::Error::runtime(format!("require: module '{name}' not found in pack"))
                })?;
                // Cached by file, so "a.b" and "a/b" are one module.
                let cached: Value = loaded.get(file.as_str())?;
                if !cached.is_nil() {
                    return Ok(cached);
                }
                // A module that requires itself, directly or not, is an error.
                if loading.get::<bool>(file.as_str())? {
                    return Err(mlua::Error::runtime(format!(
                        "require: module '{name}' requires itself"
                    )));
                }
                let txt = read_pack_file(&eng2, &file).map_err(to_lua)?;
                loading.set(file.as_str(), true)?;
                let v = lua
                    .load(txt)
                    .set_name(file.as_str())
                    .set_mode(mlua::chunk::ChunkMode::Text)
                    .call::<Value>(name.as_str());
                loading.set(file.as_str(), Value::Nil)?;
                let v = v?;
                let v = if v.is_nil() { Value::Boolean(true) } else { v };
                loaded.set(file.as_str(), &v)?;
                Ok(v)
            })?,
        )?;
    }

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

/// A whole-number field a pack keeps as a Lua number, truncated toward zero
/// like PopTracker does (and mlua before 0.12, which now refuses `1.5` as an
/// integer). Integers (and integral values) are read exactly; other numbers
/// are truncated if they fit an i64. Missing, non-numeric, NaN, infinite or
/// out-of-range values are `None`.
fn lua_int(t: &Table, key: &str) -> Option<i64> {
    if let Ok(i) = t.get::<i64>(key) {
        return Some(i);
    }
    // -2^63 is exact as a float; 2^63 is the first value past i64::MAX.
    const MIN: f64 = i64::MIN as f64;
    const MAX: f64 = -(i64::MIN as f64);
    t.get::<f64>(key)
        .ok()
        .filter(|n| *n >= MIN && *n < MAX)
        .map(|n| n.trunc() as i64)
}

/// Run a pack's Lua file, as source only: a chunk that starts with Lua's
/// bytecode signature is refused (see `sandboxed_lua`).
fn run_pack_source(lua: &Lua, src: &str, name: &str) -> mlua::Result<()> {
    lua.load(src)
        .set_name(name)
        .set_mode(mlua::chunk::ChunkMode::Text)
        .exec()
}

/// Build the sandboxed Lua VM: only string/table/math/coroutine/utf8 stdlib (base
/// functions like `print`/`pairs` are always present) and PopTracker's four
/// harmless `os` functions, i.e. no `io`/`package`/`debug`, the rest of `os` or
/// Lua's own `require` (the host adds a pack-only one), with a memory cap. Pack Lua runs here, so this
/// is the boundary that keeps an uploaded pack from touching the host.
fn sandboxed_lua() -> Result<Lua> {
    let lua = Lua::new_with(
        StdLib::STRING
            | StdLib::TABLE
            | StdLib::MATH
            | StdLib::COROUTINE
            | StdLib::OS
            | StdLib::UTF8,
        LuaOptions::default(),
    )?;
    lua.set_memory_limit(LUA_MEMORY_LIMIT)?;
    // `dofile`/`loadfile` are BASE-library functions in Lua 5.4 (not `package`),
    // so excluding the package lib doesn't drop them — and they read+run files
    // off disk. Remove them explicitly; they're the obvious filesystem escape.
    {
        let g = lua.globals();
        for f in ["dofile", "loadfile"] {
            let _ = g.set(f, Value::Nil);
        }
    }
    // Source code only, never precompiled chunks: Lua 5.4 doesn't verify
    // bytecode, and crafted bytecode can break out of the sandbox. `load`
    // always gets mode "t" (its env argument is passed on only when given,
    // since an explicit nil would mean an empty environment), and
    // `string.dump`, the way to produce bytecode from inside, is removed.
    // Pack files are loaded as text by the host as well (ChunkMode::Text).
    lua.load(
        r##"
        local load, select = load, select
        _G.load = function(chunk, name, _mode, ...)
          if select("#", ...) > 0 then return load(chunk, name, "t", ...) end
          return load(chunk, name, "t")
        end
        string.dump = nil
        -- Only what can't touch the host: the rest of `os` runs programs and
        -- removes files, and is out of reach once this table replaces it.
        local os = os
        _G.os = { clock = os.clock, date = os.date, difftime = os.difftime, time = os.time }
        "##,
    )
    .set_name("sandbox")
    .set_mode(mlua::chunk::ChunkMode::Text)
    .exec()?;
    Ok(lua)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytecode for `x = 1`, compiled outside the sandbox.
    fn bytecode() -> Vec<u8> {
        Lua::new().load("x = 1").into_function().unwrap().dump(true)
    }

    #[test]
    fn sandbox_load_takes_source_only() {
        let lua = sandboxed_lua().unwrap();
        // Source still loads, with the globals as its environment by default...
        let n: i64 = lua
            .load(r#"return load("return math.floor(2.5)")()"#)
            .eval()
            .unwrap();
        assert_eq!(n, 2);
        // ...or an explicit one.
        let v: i64 = lua
            .load(r#"return load("return y", "c", "t", { y = 7 })()"#)
            .eval()
            .unwrap();
        assert_eq!(v, 7);
        // Binary chunks are refused, even when the caller asks for mode "b".
        lua.globals()
            .set("BC", lua.create_string(bytecode()).unwrap())
            .unwrap();
        for call in [
            r#"return load(BC, "bc", "b")"#,
            r#"return load(BC)"#,
            r#"local s = BC; return load(function() local r = s; s = nil; return r end)"#,
        ] {
            let (f, err): (Value, Option<String>) = lua.load(call).eval().unwrap();
            assert!(f.is_nil(), "a binary chunk must not load: {call}");
            let err = err.unwrap_or_default();
            assert!(err.contains("binary"), "{call}: {err}");
        }
        // And there's no way to make bytecode inside.
        let dump: Value = lua.load("return string.dump").eval().unwrap();
        assert!(dump.is_nil());
    }

    #[test]
    fn pack_scripts_must_be_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("scripts")).unwrap();
        std::fs::write(dir.path().join("scripts/init.lua"), bytecode()).unwrap();
        let pack = Pack::load(dir.path()).unwrap();
        // Real bytecode always contains bytes that aren't valid UTF-8, so
        // reading the file refuses it first.
        let err = Session::new(&pack)
            .err()
            .expect("a precompiled init.lua must be refused");
        assert!(err.to_string().contains("binary"), "{err}");
    }

    #[test]
    fn pack_files_run_as_text_only() {
        // The backstop behind the UTF-8 read: a chunk that starts with the
        // bytecode signature is refused by the text-only mode itself.
        let lua = sandboxed_lua().unwrap();
        let err = run_pack_source(&lua, "\x1bLua x = 1", "fake.lua").unwrap_err();
        // Refused for being binary at all, not merely for being malformed
        // ("bad binary format"), which is what happens without the mode.
        assert!(err.to_string().contains("mode is 't'"), "{err}");
        run_pack_source(&lua, "x = 1", "ok.lua").unwrap();
    }

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
        for banned in ["io", "package", "require", "dofile", "loadfile", "debug"] {
            assert!(
                matches!(g.get::<Value>(banned).unwrap(), Value::Nil),
                "{banned} must be unavailable to pack Lua"
            );
        }
        let os: Table = g.get("os").unwrap();
        let mut names: Vec<String> = os
            .pairs::<String, Value>()
            .map(|kv| kv.unwrap().0)
            .collect();
        names.sort();
        assert_eq!(names, ["clock", "date", "difftime", "time"]);
        for allowed in ["string", "table", "math", "print", "pairs", "type"] {
            assert!(
                !matches!(g.get::<Value>(allowed).unwrap(), Value::Nil),
                "{allowed} should be present"
            );
        }
    }
}
