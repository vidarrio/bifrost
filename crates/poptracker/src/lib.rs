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
//!   touches, plus PopTracker's item model for every item type, which packs
//!   read and write through their objects.
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
//! - Custom items' clicks and icons.
//! - The reactive recompute loop and renderer data
//!   (maps/coordinates/item layout) for the visual tab.
//! - Memory-autotracking packs (SNES/N64) are inherently unsupported server-side.

mod archive;
mod defs;
mod host;
mod item_json;
mod items;
mod json;
mod layout;
mod pack;
mod rules;
mod sandbox;
mod session;
mod state;

pub use archive::{extract_zip, pack_root};
pub use item_json::ImageFilter;
pub use pack::{Pack, read_asset};
pub use session::Session;
pub use state::{ItemState, LocationState, MapInfo, Marker, SectionState, Summary, TrackerState};

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
