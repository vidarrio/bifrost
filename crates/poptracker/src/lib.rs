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
//! - Evaluates access rules as PopTracker's `resolveRules` does: item codes,
//!   `code:count`, `[optional]`, `{inspect}`, `@location/section` references,
//!   `$function|arg|…` and `^$function` levels, settled over repeated passes.
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

    /// The variant in use; empty if the pack has none.
    pub fn variant(&self) -> &str {
        &self.variant
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

/// Read a file of an unpacked pack without loading it, given its variant. Serving
/// images takes this path: it parses nothing a pack wrote, so it's safe in the
/// server's own process, unlike [`Pack::load`], which belongs in a worker.
pub fn read_asset(dir: &Path, variant: &str, rel: &str) -> Option<Vec<u8>> {
    std::fs::read(find_in_pack(dir, variant, rel)?).ok()
}

/// The most of a manifest that's read. Parsing one recurses, so a hostile pack
/// can overflow the stack: that's why only a worker may call [`Pack::load`].
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
    /// Reachable right now, at the normal level.
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
    /// The variant in use; empty if the pack has none.
    #[serde(default)]
    pub variant: String,
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
    rules: Rules, // effective access_rules: the location's joined to its own
    vis: Rules,   // effective visibility_rules; empty = always visible
    scoutable_glitched: bool,
}
#[derive(Clone)]
struct LocNode {
    name: String,       // full path, e.g. "Area/Sub/Spot"
    short_name: String, // its last part
    rules: Rules,       // effective access_rules: the parent chain's joined to its own
    vis: Rules,         // effective visibility_rules, likewise; empty = always visible
    scoutable_glitched: bool,
    map_locations: Vec<MapLoc>,
    sections: Vec<SectionDef>,
}

impl LocNode {
    /// A real, drawable/checkable location: it has sections or a map placement.
    fn drawn(&self) -> bool {
        !self.sections.is_empty() || !self.map_locations.is_empty()
    }
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
    nodes: Rc<Vec<LocNode>>,
    /// What's settled of the rules (see `settle_accessibility`), by location or
    /// `location/section` id, and whether a change has made it stale.
    accessibility: HashMap<String, Level>,
    visibility: HashMap<String, bool>,
    accessibility_stale: bool,
    visibility_stale: bool,
    updating_cache: bool,
    /// `provider_count_cached`'s results, the `$` codes it's running, and those
    /// that looked at a location and so aren't kept.
    provider_cache: HashMap<String, i64>,
    lua_codes: Vec<String>,
    indirect_codes: HashSet<String>,
    location_lookup: HashMap<String, Option<usize>>,
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

/// Registry key of the metatable location and section objects share, for the
/// level they read on demand.
const LOCATION_METHODS: &str = "bifrost.location_methods";

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
        s.run_frame_handlers()?;
        Ok(s)
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
        e.provider_cache.clear();
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
        self.run_frame_handlers()
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
        if let Err(e) = self.run_frame_handlers() {
            note_error(&self.eng, &e.to_string());
            self.charge();
        }
        for w in registered(&self.lua, CODE_WATCHES, |e| {
            e.get::<mlua::LuaString>("code")
                .is_ok_and(|c| c.as_bytes().as_ref() == b"*")
        }) {
            self.guard_noting(w.call::<()>("*"));
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
                // As PopTracker draws it: a consumable's count first, then the
                // pack's own text. Not through metatables, with the engine borrowed.
                let text = |field| {
                    t.raw_get::<Option<String>>(field)
                        .unwrap_or(None)
                        .filter(|s| !s.is_empty())
                };
                let overlay = (kind == "consumable" && count > 0)
                    .then(|| count.to_string())
                    .or_else(|| text("BadgeText"));
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
        // Handlers may have changed anything, so settle here, once.
        self.charge();
        settle_accessibility(&self.lua, &self.eng);
        if enforce_vis {
            settle_visibility(&self.lua, &self.eng);
        }
        if self.budget.get() == 0 {
            note_error(
                &self.eng,
                "the pack's rules ran out of instruction budget, so some may read as unmet",
            );
        }
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
        for n in nodes.iter().filter(|n| n.drawn()) {
            // Hide a location whose visibility rules fail (e.g. options the slot
            // didn't generate) — dropped from both markers and the locations list,
            // just as native PopTracker omits them.
            if !visible(&n.name, &n.vis) {
                continue;
            }
            let loc_access = level_of(&n.name) >= LEVEL_NORMAL;
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
                .filter(|s| visible(&format!("{}/{}", n.name, s.name), &s.vis))
                .map(|s| {
                    let avail = section_avail(&s.name);
                    let lvl = level_of(&format!("{}/{}", n.name, s.name));
                    SectionState {
                        name: s.name.clone(),
                        item_count: s.item_count,
                        cleared: avail.map_or(0, |a| (s.item_count - a).max(0)),
                        accessible: lvl >= LEVEL_NORMAL,
                        level: level_name(lvl).to_string(),
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
                name: n.name.clone(),
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
            items: e.kind.len(),
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

fn read_pack_file(eng: &E, rel: &str) -> Result<String> {
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

/// PopTracker's accessibility levels, with the numbers packs see as
/// `AccessibilityLevel`: a `^$` rule's function returns one.
type Level = i64;
const LEVEL_NONE: Level = 0;
const LEVEL_PARTIAL: Level = 1;
const LEVEL_INSPECT: Level = 3;
const LEVEL_SEQUENCE_BREAK: Level = 5;
const LEVEL_NORMAL: Level = 6;
const LEVEL_CLEARED: Level = 7;

fn level_name(l: Level) -> &'static str {
    match l {
        LEVEL_NONE => "none",
        LEVEL_INSPECT => "inspect",
        LEVEL_SEQUENCE_BREAK => "glitch",
        _ => "reachable",
    }
}

/// How many times the rules are gone through for references between locations
/// to settle. PopTracker goes on until they do; a pack can make them flip forever.
const MAX_RULE_PASSES: usize = 256;

/// A rule list: any of its alternatives (OR), each all of its codes (AND).
type Rules = Vec<Vec<String>>;

/// `atoi`: the leading number, or 0.
fn leading_int(s: &str) -> i64 {
    let s = s.trim_start();
    let (sign, digits) = match s.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, s.strip_prefix('+').unwrap_or(s)),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    digits[..end].parse::<i64>().map_or(0, |n| sign * n)
}

/// Run a rule's `$name|arg|…` function, as PopTracker reads its result: a number
/// (or a string that is one) truncated, `true` as 1, anything else or an error 0.
fn run_rule_function(lua: &Lua, code: &str) -> i64 {
    let mut parts = code.strip_prefix('$').unwrap_or(code).split('|');
    let name = parts.next().unwrap_or("");
    let Ok(func) = lua.globals().get::<Function>(name) else {
        return 0;
    };
    let args =
        MultiValue::from_iter(parts.filter_map(|a| lua.create_string(a).ok().map(Value::String)));
    match func.call::<Value>(args) {
        Ok(Value::Integer(i)) => i,
        Ok(Value::Number(n)) => n as i64,
        Ok(Value::Boolean(b)) => b as i64,
        Ok(Value::String(t)) => t
            .to_str()
            .ok()
            .and_then(|t| t.trim().parse::<f64>().ok())
            .map_or(0, |n| n as i64),
        _ => 0,
    }
}

/// PopTracker's `ProviderCountForCode`: `$` codes run Lua, others count items.
/// Results are kept for the operation, except a function's that looked at a
/// location, which may change as the rules settle.
fn provider_count_cached(lua: &Lua, eng: &E, code: &str) -> i64 {
    if let Some(n) = eng.borrow().provider_cache.get(code) {
        return *n;
    }
    let n = if code.starts_with('$') {
        eng.borrow_mut().lua_codes.push(code.to_string());
        let n = run_rule_function(lua, code);
        eng.borrow_mut().lua_codes.pop();
        n
    } else {
        provider_count(lua, eng, code)
    };
    let mut e = eng.borrow_mut();
    if !e.indirect_codes.contains(code) {
        e.provider_cache.insert(code.to_string(), n);
    }
    n
}

/// A function running for a rule looked at a location (see `provider_count_cached`).
fn mark_indirect(eng: &E) {
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

/// The object `Tracker:FindObjectForCode("@…")` means: its id, and a chest count
/// if it's a section.
fn location_object(eng: &E, path: &str) -> Option<(String, Option<i64>)> {
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

/// A reference's level, from what's settled so far.
fn reference_level(eng: &E, id: &str, visibility: bool) -> Option<Level> {
    let cached = |key: &str| -> Level {
        let e = eng.borrow();
        if visibility {
            e.visibility.get(key).copied().unwrap_or(true) as Level * LEVEL_NORMAL
        } else {
            e.accessibility.get(key).copied().unwrap_or(LEVEL_NONE)
        }
    };
    if let Some(i) = find_location_cached(eng, id) {
        let key = eng.borrow().nodes[i].name.clone();
        return Some(cached(&key));
    }
    let (loc, section) = id.rsplit_once('/')?;
    let i = find_location_cached(eng, loc)?;
    let e = eng.borrow();
    let n = &e.nodes[i];
    if !n.sections.iter().any(|s| s.name == section) {
        return None;
    }
    let key = format!("{}/{section}", n.name);
    drop(e);
    Some(cached(&key))
}

/// PopTracker's `resolveRules`, kept line for line in its logic: packs are written
/// against its behaviour, quirks included.
fn resolve_rules(
    lua: &Lua,
    eng: &E,
    rules: &Rules,
    visibility: bool,
    scoutable_glitched: bool,
) -> Level {
    let (mut glitched, mut inspectable) = (false, false);
    if rules.is_empty() {
        return LEVEL_NORMAL;
    }
    for ruleset in rules {
        if ruleset.is_empty() {
            return LEVEL_NORMAL;
        }
        let mut reachable = LEVEL_NORMAL;
        let mut inspect_only = false;
        for rule in ruleset {
            if rule.is_empty() {
                continue;
            }
            let mut s = rule.as_str();
            // `{…}` needs this to look, not to get.
            if let Some(rest) = s.strip_prefix('{') {
                inspect_only = true;
                s = rest;
            }
            if inspect_only && let Some(rest) = s.strip_suffix('}') {
                s = rest;
            }
            // `[…]` is optional: without it, a sequence break.
            let mut optional = false;
            if s.len() > 1 && s.starts_with('[') && s.ends_with(']') {
                optional = true;
                s = &s[1..s.len() - 1];
            }
            if inspect_only && s.is_empty() {
                continue;
            }
            // `^$f` gives a level; `@…` refers to a location; `code:n` wants n.
            let is_level = s.starts_with('^');
            let is_reference = s.starts_with('@');
            let mut count = 1;
            if !is_level
                && !is_reference
                && let Some((code, n)) = s.split_once(':')
            {
                count = leading_int(n);
                s = code;
            }
            if is_level {
                if s.len() < 3 || !s[1..].starts_with('$') {
                    reachable = LEVEL_NONE;
                    break;
                }
                s = &s[1..];
            }
            if let Some(id) = s.strip_prefix('@') {
                if let Some(mut sub) = reference_level(eng, id, visibility) {
                    if !inspect_only && sub == LEVEL_INSPECT {
                        sub = LEVEL_NONE;
                    } else if optional && sub == LEVEL_NONE {
                        sub = LEVEL_SEQUENCE_BREAK;
                    } else if sub == LEVEL_NONE {
                        reachable = LEVEL_NONE;
                    }
                    if sub == LEVEL_SEQUENCE_BREAK && reachable != LEVEL_NONE {
                        reachable = LEVEL_SEQUENCE_BREAK;
                    }
                }
                if reachable == LEVEL_NONE {
                    break;
                }
            } else {
                let n = provider_count_cached(lua, eng, s);
                if is_level {
                    let mut sub = n;
                    if !inspect_only && sub == LEVEL_INSPECT {
                        inspect_only = true;
                    } else if optional && sub == LEVEL_NONE {
                        sub = LEVEL_SEQUENCE_BREAK;
                    } else if sub == LEVEL_NONE {
                        reachable = LEVEL_NONE;
                    }
                    if sub == LEVEL_SEQUENCE_BREAK && reachable != LEVEL_NONE {
                        reachable = LEVEL_SEQUENCE_BREAK;
                    }
                    if reachable == LEVEL_NONE {
                        break;
                    }
                    continue;
                }
                if n >= count || location_present(eng, s) {
                    continue;
                }
                if optional {
                    reachable = LEVEL_SEQUENCE_BREAK;
                } else {
                    reachable = LEVEL_NONE;
                    break;
                }
            }
        }
        if reachable == LEVEL_NORMAL && !inspect_only {
            return LEVEL_NORMAL;
        }
        if reachable != LEVEL_NONE && inspect_only {
            inspectable = true;
        }
        if reachable == LEVEL_SEQUENCE_BREAK {
            glitched = true;
        }
    }
    if glitched && inspectable && !scoutable_glitched {
        LEVEL_INSPECT
    } else if glitched {
        LEVEL_SEQUENCE_BREAK
    } else if inspectable {
        LEVEL_INSPECT
    } else {
        LEVEL_NONE
    }
}

/// Settle every location's and section's level, unless that's been done since the
/// last change. While settling, a look at a level gets what's known so far.
fn settle_accessibility(lua: &Lua, eng: &E) {
    {
        let mut e = eng.borrow_mut();
        if !e.accessibility_stale || e.updating_cache {
            return;
        }
        e.accessibility_stale = false;
        e.updating_cache = true;
        e.accessibility.clear();
    }
    let nodes = eng.borrow().nodes.clone();
    for _ in 0..MAX_RULE_PASSES {
        let mut changed = false;
        let mut update = |id: &str, rules: &Rules, glitched: bool| {
            let known = eng.borrow().accessibility.get(id).copied();
            if known == Some(LEVEL_NORMAL) {
                return;
            }
            let level = resolve_rules(lua, eng, rules, false, glitched);
            if known != Some(level) {
                eng.borrow_mut().accessibility.insert(id.to_string(), level);
                changed = true;
            }
        };
        for n in nodes.iter() {
            update(&n.name, &n.rules, n.scoutable_glitched);
            for s in &n.sections {
                update(
                    &format!("{}/{}", n.name, s.name),
                    &s.rules,
                    s.scoutable_glitched,
                );
            }
        }
        if !changed {
            break;
        }
    }
    let mut e = eng.borrow_mut();
    e.updating_cache = false;
    e.indirect_codes.clear();
}

/// As [`settle_accessibility`], for visibility. Empty rules mean visible, and
/// aren't kept.
fn settle_visibility(lua: &Lua, eng: &E) {
    {
        let mut e = eng.borrow_mut();
        if !e.visibility_stale || e.updating_cache {
            return;
        }
        e.visibility_stale = false;
        e.updating_cache = true;
        e.visibility.clear();
    }
    let nodes = eng.borrow().nodes.clone();
    for _ in 0..MAX_RULE_PASSES {
        let mut changed = false;
        let mut update = |id: &str, rules: &Rules| {
            if rules.is_empty() {
                return;
            }
            let known = eng.borrow().visibility.get(id).copied();
            if known == Some(true) {
                return;
            }
            let visible = resolve_rules(lua, eng, rules, true, false) != LEVEL_NONE;
            if known != Some(visible) {
                eng.borrow_mut().visibility.insert(id.to_string(), visible);
                changed = true;
            }
        };
        for n in nodes.iter() {
            update(&n.name, &n.vis);
            for s in &n.sections {
                update(&format!("{}/{}", n.name, s.name), &s.vis);
            }
        }
        if !changed {
            break;
        }
    }
    let mut e = eng.borrow_mut();
    e.updating_cache = false;
    e.indirect_codes.clear();
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
/// A location's or section's rule list, each alternative joined to each of its
/// parent's (PopTracker's way of inheriting). A rule is a comma-separated string or
/// an array of codes; a whole list may also be one string.
fn parse_rules(node: &serde_json::Value, field: &str, parent: &Rules) -> Rules {
    let split = |v: &str| -> Vec<String> { v.split(',').map(|c| c.trim().to_string()).collect() };
    let own: Vec<Vec<String>> = match node.get(field) {
        Some(serde_json::Value::String(s)) if !s.is_empty() => vec![split(s)],
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .filter_map(|v| match v {
                serde_json::Value::String(s) => Some(split(s)),
                serde_json::Value::Array(parts) => Some(
                    parts
                        .iter()
                        .filter_map(|p| p.as_str().map(String::from))
                        .collect(),
                ),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    if own.is_empty() {
        return parent.clone();
    }
    if parent.is_empty() {
        return own;
    }
    let mut out = Vec::with_capacity(own.len() * parent.len());
    for new in &own {
        for old in parent {
            let mut rule = old.clone();
            rule.extend(new.iter().cloned());
            out.push(rule);
        }
    }
    out
}

/// Flatten the location tree into [`LocNode`]s, accumulating the parent chain's
/// access-rule groups so a node's effective accessibility includes its ancestors.
fn flatten_locations(
    eng: &E,
    node: &serde_json::Value,
    prefix: &str,
    parent_rules: &Rules,
    parent_vis: &Rules,
    parent_glitched: bool,
) {
    if let Some(arr) = node.as_array() {
        for n in arr {
            flatten_locations(eng, n, prefix, parent_rules, parent_vis, parent_glitched);
        }
        return;
    }
    let name = node.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let full = if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    };

    let rules = parse_rules(node, "access_rules", parent_rules);
    let vis = parse_rules(node, "visibility_rules", parent_vis);
    let scoutable_glitched = node
        .get("inspectable_sequence_break")
        .and_then(|v| v.as_bool())
        .unwrap_or(parent_glitched);

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
                    rules: parse_rules(s, "access_rules", &rules),
                    vis: parse_rules(s, "visibility_rules", &vis),
                    scoutable_glitched: s
                        .get("inspectable_sequence_break")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(scoutable_glitched),
                })
                .collect()
        })
        .unwrap_or_default();

    // Every location is kept, as rules may refer to one that isn't drawn.
    {
        let mut e = eng.borrow_mut();
        let nodes = Rc::make_mut(&mut e.nodes);
        nodes.push(LocNode {
            name: full.clone(),
            short_name: name.to_string(),
            rules: rules.clone(),
            vis: vis.clone(),
            scoutable_glitched,
            map_locations,
            sections,
        });
        e.location_lookup.clear();
        e.accessibility_stale = true;
        e.visibility_stale = true;
    }
    if let Some(children) = node.get("children") {
        flatten_locations(eng, children, &full, &rules, &vis, scoutable_glitched);
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
                    e.accessibility_stale = true;
                    e.visibility_stale = true;
                    e.provider_cache.clear();
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
                flatten_locations(&eng2, &json, "", &Vec::new(), &Vec::new(), false);
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
                    }
                    t.set("Active", false)?;
                    t.set("FullID", id.as_str())?;
                    t.set("Owner", lua.create_table()?)?;
                    t.set_metatable(Some(lua.named_registry_value(LOCATION_METHODS)?))?;
                    objs.raw_set(id.as_str(), &t)?;
                    return Ok(Value::Table(t));
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

    // ScriptHost
    let sh = lua.create_table()?;
    {
        let eng2 = eng.clone();
        sh.set(
            "LoadScript",
            // Some packs load a script they can only name once connected.
            lua.create_function(move |lua, (_t, file): (Table, Option<String>)| {
                let Some(file) = file else { return Ok(false) };
                // As in PopTracker, a script that won't load is logged and the
                // caller carries on.
                let ran = read_pack_file(&eng2, &file)
                    .map_err(to_lua)
                    .and_then(|txt| {
                        lua.load(txt)
                            .set_name(file.as_str())
                            .set_mode(mlua::chunk::ChunkMode::Text)
                            .call::<()>(load_script_module(&file))
                    });
                match ran {
                    Ok(()) => Ok(true),
                    Err(e) if is_memory_error(&e) => Err(e),
                    Err(e) => {
                        note_error(&eng2, &e.to_string());
                        Ok(false)
                    }
                }
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
        // `arg` is the script's while it runs; a failure is logged, as in
        // PopTracker, and no callback is told. A callback's own error is too.
        let notes = eng.clone();
        let finish =
            move |lua: &Lua, src: String, name: String, arg: Value, done: Option<Function>| {
                let chunk = lua
                    .load(src)
                    .set_name(name)
                    .set_mode(mlua::chunk::ChunkMode::Text);
                let outer: Value = lua.globals().get("arg")?;
                lua.globals().set("arg", arg)?;
                let result = chunk.eval::<Value>();
                lua.globals().set("arg", outer)?;
                let finished = match (result, done) {
                    (Ok(v), Some(done)) => done.call::<()>(v),
                    (Ok(_), None) => Ok(()),
                    (Err(e), _) => Err(e),
                };
                match finished {
                    Err(e) if is_memory_error(&e) => Err(e),
                    Err(e) => {
                        note_error(&notes, &e.to_string());
                        Ok(Value::Nil)
                    }
                    Ok(()) => Ok(Value::Nil),
                }
            };
        let finish2 = finish.clone();
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
                    finish2(lua, src, "script".to_string(), arg, done)
                },
            )?,
        )?;
    }
    for noop in ["AsyncProgress", "PushMarkdownNotification"] {
        sh.set(noop, lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    }
    // What a pack passes to `PushMarkdownNotification`, an EmoTracker call some
    // still make: PopTracker lacks both, but they cost nothing here.
    let kinds = lua.create_table()?;
    for (i, k) in ["Message", "Warning", "Error", "Celebration"]
        .iter()
        .enumerate()
    {
        kinds.set(*k, i)?;
    }
    g.set("NotificationType", kinds)?;
    g.set("ScriptHost", &sh)?;

    // A location or section object's `AccessibilityLevel`, read from the rules.
    {
        let eng2 = eng.clone();
        let methods = lua.create_table()?;
        methods.set(
            "__index",
            lua.create_function(move |lua, (t, key): (Table, Value)| {
                if key
                    .as_string()
                    .and_then(|k| k.to_str().ok().map(|k| k.to_string()))
                    .as_deref()
                    != Some("AccessibilityLevel")
                {
                    return Ok(Value::Nil);
                }
                let id: String = t.raw_get("FullID")?;
                mark_indirect(&eng2);
                settle_accessibility(lua, &eng2);
                let level = eng2.borrow().accessibility.get(&id).copied();
                Ok(Value::Integer(level.unwrap_or(LEVEL_NONE)))
            })?,
        )?;
        lua.set_named_registry_value(LOCATION_METHODS, methods)?;
    }
    // Custom items: a table the pack fills in, which `FindObjectForCode` can
    // find again. `:Set`/`:Get` keep values in its `ItemState`; overlays,
    // clicks and icons aren't drawn here.
    // What JSON items share: the overlay text shows, the rest isn't drawn here.
    lua.load(
        r##"
        local function noop() end
        return { __index = {
          SetOverlay = function(self, text) rawset(self, "BadgeText", text) end,
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
        ("None", LEVEL_NONE),
        ("Partial", LEVEL_PARTIAL),
        ("Inspect", LEVEL_INSPECT),
        ("SequenceBreak", LEVEL_SEQUENCE_BREAK),
        ("Normal", LEVEL_NORMAL),
        ("Cleared", LEVEL_CLEARED),
    ] {
        al.set(k, v)?;
    }
    g.set("AccessibilityLevel", &al)?;

    Ok(())
}

/// The module name `ScriptHost:LoadScript` passes a script as `...`:
/// `scripts/a/b.lua` is `a.b`.
fn load_script_module(file: &str) -> String {
    let name = file.strip_prefix("scripts/").unwrap_or(file);
    let name = name
        .strip_suffix(".lua")
        .or_else(|| name.strip_suffix(".LUA"))
        .unwrap_or(name);
    name.replace('/', ".")
}

/// Keep a pack handler's error for the tracker to report (see `MAX_PACK_ERRORS`).
fn note_error(eng: &E, text: &str) {
    let mut e = eng.borrow_mut();
    e.pack_error_count += 1;
    if e.pack_errors.len() < MAX_PACK_ERRORS {
        let end = (0..=text.len().min(MAX_PACK_ERROR_LEN))
            .rev()
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(0);
        e.pack_errors.push(text[..end].to_string());
    }
}

/// Is this Lua running out of memory, however many callbacks it came through?
fn is_memory_error(e: &mlua::Error) -> bool {
    match e {
        mlua::Error::MemoryError(_) => true,
        mlua::Error::CallbackError { cause, .. } => is_memory_error(cause),
        _ => false,
    }
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

/// Build the sandboxed Lua VM: only string/table/math/coroutine/utf8 stdlib
/// (base functions like `print`/`pairs` are always present) and PopTracker's
/// four harmless `os` functions, i.e. no `io`/`package`/`debug`, the rest of
/// `os` or Lua's own `require` (the host adds a pack-only one), with a memory
/// cap. Pack Lua runs here, so this
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
