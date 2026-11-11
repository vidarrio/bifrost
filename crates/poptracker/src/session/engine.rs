//! The engine's shared state, and the registry keys and limits it works under.

use crate::defs::{LocNode, MapDef};
use crate::item_json::JsonItem;
use crate::rules::Level;
use mlua::Function;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Default)]
pub(crate) struct Eng {
    pub(crate) variant: String,
    pub(crate) scoutable_default: bool,
    /// What `@disabled` means in image mods: settings.json's `disabled_image_filter`.
    pub(crate) disabled_filter: String,
    /// The instruction budget left (see `Session::charge`): settling stops at 0.
    pub(crate) budget: Rc<std::cell::Cell<u64>>,
    /// The next number for a location id seen before (see `flatten_locations`).
    pub(crate) duplicate_ids: HashMap<String, u64>,
    pub(crate) taken_ids: HashSet<String>,
    /// The first few errors from the pack's handlers, and how many in all.
    pub(crate) pack_errors: Vec<String>,
    pub(crate) pack_error_count: usize,
    pub(crate) dir: PathBuf,
    /// The pack's JSON items, in the order they were added.
    pub(crate) items: Vec<JsonItem>,
    /// Lowercase code -> the items that can provide it, in order.
    pub(crate) code_items: HashMap<String, Vec<usize>>,
    /// Which of them are composite toggles.
    pub(crate) composites: Vec<usize>,
    /// Data storage keys a pack has asked for and not been answered on, with
    /// whether by `SetNotify`, and those it has been (`replay_storage`).
    pub(crate) storage_asked: BTreeSet<(String, bool)>,
    pub(crate) storage_answered: HashSet<(String, bool)>,
    /// Items changed since code watches last ran, and whether a custom item did
    /// (which ones are in `DIRTY_CUSTOM`).
    pub(crate) dirty_items: BTreeSet<usize>,
    pub(crate) dirty_custom: bool,
    /// Calls to custom items' functions in this operation (see `count_custom_call`).
    pub(crate) custom_calls: u64,
    /// How deep custom items' `ProvidesCodeFunc`s have nested.
    pub(crate) provider_depth: u32,
    pub(crate) maps: Vec<MapDef>,
    pub(crate) nodes: Rc<Vec<LocNode>>,
    /// How many locations there were before the file being added.
    pub(crate) locations_before: usize,
    /// What's settled of the rules (see `settle_accessibility`), by location or
    /// `location/section` id, and whether a change has made it stale.
    pub(crate) accessibility: HashMap<String, Level>,
    pub(crate) visibility: HashMap<String, bool>,
    pub(crate) accessibility_stale: bool,
    pub(crate) visibility_stale: bool,
    pub(crate) updating_cache: bool,
    /// `provider_count_cached`'s results, the `$` codes it's running, and those
    /// that looked at a location and so aren't kept.
    pub(crate) provider_cache: HashMap<String, i64>,
    pub(crate) lua_codes: Vec<String>,
    pub(crate) indirect_codes: HashSet<String>,
    pub(crate) location_lookup: HashMap<String, Option<usize>>,
    /// Named layout widgets (`layouts/*.json`), e.g. `tracker_default`.
    pub(crate) layouts: HashMap<String, serde_json::Value>,
    /// The slot's valid AP location ids (checked ∪ missing). `None` until the
    /// host feeds them; while `None`, `visibility_rules` are not applied (every
    /// marker shows). Packs gate markers on `visibility_rules` that reference a
    /// location id as a bare code — present iff the id is in this set — so this
    /// is what lets us hide checks a slot's options didn't generate (e.g. the
    /// "Shipped" squares when shipsanity is off), matching native PopTracker.
    pub(crate) valid_locations: Option<HashSet<i64>>,
    pub(crate) clear_h: Vec<Function>,
    pub(crate) item_h: Vec<Function>,
    pub(crate) loc_h: Vec<Function>,
    pub(crate) retr_h: Vec<Function>,
    pub(crate) setr_h: Vec<Function>,
}

/// Registry keys of the tables holding `ScriptHost:AddOnFrameHandler`
/// callbacks (name → function) and `ScriptHost:AddWatchForCode` ones (name →
/// `{code, fn}`). They live in Lua, so the memory cap counts them. There are
/// no frames here: frame handlers run once after `init.lua`, after each clear
/// and before each `state`. Watches run after each operation that changed what
/// they watch (see `Session::run_code_watches`).
pub(crate) const FRAME_HANDLERS: &str = "bifrost.frame_handlers";
pub(crate) const CODE_WATCHES: &str = "bifrost.code_watches";

/// Registry key of the players table (slot → `{name, game}`), filled by
/// `Session::set_multiworld`.
pub(crate) const PLAYERS: &str = "bifrost.players";

/// Registry key of the metatable location and section objects share, for the
/// level they read on demand.
pub(crate) const LOCATION_METHODS: &str = "bifrost.location_methods";

/// Registry key of the metatable JSON items share for their methods.
pub(crate) const ITEM_METHODS: &str = "bifrost.item_methods";

/// Registry key of the JSON items' objects, by index + 1, and the raw field in
/// each that says which item it is.
pub(crate) const ITEM_OBJECTS: &str = "bifrost.item_objects";
pub(crate) const ITEM_INDEX: &str = "__item";

/// Registry key of the custom items changed since watches last ran (item -> true).
pub(crate) const DIRTY_CUSTOM: &str = "bifrost.dirty_custom";

/// How much of the data storage a pack can ask for, and how many times the answers
/// may lead it to ask for more.
pub(crate) const MAX_STORAGE_KEY: usize = 1024;
pub(crate) const MAX_STORAGE_KEYS: usize = 65536;
pub(crate) const MAX_STORAGE_ROUNDS: usize = 8;

/// How many times watches may set off further watches in one operation.
pub(crate) const MAX_WATCH_ROUNDS: usize = 8;

/// How many calls to custom items' functions one operation may make. Ample: real
/// packs have few such items, and their counts are kept while rules settle.
pub(crate) const MAX_CUSTOM_CALLS: u64 = 2_000_000;

/// How deep `FindObjectForCode` may nest through custom items'
/// `CanProvideCodeFunc`s that look up other codes.
pub(crate) const MAX_LOOKUP_DEPTH: u32 = 16;

/// How many of a pack's handler errors are kept, and how much of each.
pub(crate) const MAX_PACK_ERRORS: usize = 5;
pub(crate) const MAX_PACK_ERROR_LEN: usize = 500;
pub(crate) type E = Rc<RefCell<Eng>>;
