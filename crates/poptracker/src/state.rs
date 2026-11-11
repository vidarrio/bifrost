//! The tracker state the crate reports: its public output types.

use crate::item_json::ImageFilter;
use serde::Serialize;

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
    /// What to draw it with.
    pub filters: Vec<ImageFilter>,
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
    /// Its default marker orientation (north…) and border.
    pub location_orientation: Option<String>,
    pub location_border: Option<f64>,
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
    /// The highlight a pack set on it from a hint, as PopTracker's `Highlight`:
    /// -1 avoid, 0 none, 1 no priority, 2 unspecified, 3 priority.
    pub highlight: i64,
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
    pub orientation: Option<String>,
    pub border: Option<f64>,
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
