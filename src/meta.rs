//! Season metadata: static facts decoded from the seed (`output/current.zip`)
//! by the AP adapter — per-slot item totals and id→name maps for items and
//! locations. Loaded once per season (subprocess) and cached in the board.

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;

use crate::{ap_adapter, config};

/// Authoritative save state for one AP slot: how many locations it has checked
/// (`sent`) and the items it has been granted (`items`, each `[item, location,
/// finder]`). The received count is `items.len()`.
#[derive(Debug, Default, Deserialize)]
pub struct SlotCounts {
    #[serde(default)]
    pub sent: u64,
    #[serde(default)]
    pub items: Vec<[i64; 3]>,
    /// Location ids this slot has checked (for the PopTracker "Done"/grey state).
    #[serde(default)]
    pub checked: Vec<i64>,
}

/// Read every slot's true sent/received totals from the season's save
/// (blocks on a Python subprocess). Returns an empty map before the server has
/// written a save.
pub async fn load_counts(season: i64) -> Result<HashMap<i64, SlotCounts>, String> {
    ap_adapter::load_counts(&config::get().season(season)).await
}

#[derive(Debug, Deserialize)]
pub struct SlotMeta {
    /// The slot's player name and game.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub game: String,
    #[serde(default)]
    pub world_size: u32,
    /// Total items destined for this slot anywhere in the multiworld. Minus how
    /// many it has received so far, this gives "still to be found".
    #[serde(default)]
    pub items_for: u32,
    /// Where each of this slot's items is placed: item id -> list of
    /// [finder_slot, location_id]. Powers the hint box's per-item copy counting.
    #[serde(default)]
    pub placements: HashMap<i64, Vec<[i64; 2]>>,
    /// Every AP location id that exists for this slot (the seed's full location
    /// set for the slot). Fed to the PopTracker engine so `visibility_rules` can
    /// hide markers for options the slot never generated. Static per season.
    #[serde(default)]
    pub all_locations: Vec<i64>,
    /// The slot's starting inventory (precollected item ids). Delivered on
    /// connect in AP, so a live tracker autotracks them — but they're not in the
    /// savefile's received_items, so the engine must be fed them separately or
    /// precollected gates (e.g. a starting season) evaluate false. Static.
    #[serde(default)]
    pub precollected: Vec<i64>,
    /// The slot's slot data, as AP sends it to the slot's clients on connect.
    /// Passed to the slot's PopTracker pack, which may read the seed's options
    /// from it (e.g. starting recipes that aren't starting items). Static.
    #[serde(default)]
    pub slot_data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct Meta {
    /// The seed's name (AP `seed_name`) — Bifrost's season key today.
    #[serde(default)]
    pub seed: Option<String>,
    /// AP slot -> totals.
    #[serde(default)]
    pub slots: BTreeMap<i64, SlotMeta>,
    /// game -> (item id -> name).
    #[serde(default)]
    pub item_names: HashMap<String, HashMap<i64, String>>,
    /// game -> (location id -> name).
    #[serde(default)]
    pub location_names: HashMap<String, HashMap<i64, String>>,
    /// finder slot NAME -> (location NAME -> item classification flags), for
    /// classifying a sent item's importance (0b001 progression / 0b010 useful /
    /// 0b100 trap / 0 filler) from a journal "sent" line's (sender, location).
    #[serde(default)]
    pub loc_flags: HashMap<String, HashMap<String, i64>>,
}

impl Meta {
    pub fn item_name(&self, game: &str, id: i64) -> Option<&str> {
        self.item_names.get(game)?.get(&id).map(String::as_str)
    }
    pub fn location_name(&self, game: &str, id: i64) -> Option<&str> {
        self.location_names.get(game)?.get(&id).map(String::as_str)
    }
    pub fn totals(&self, ap_slot: i64) -> Option<&SlotMeta> {
        self.slots.get(&ap_slot)
    }
    /// Where this slot's items sit: item id -> [[finder_slot, location_id], …].
    pub fn placements(&self, ap_slot: i64) -> Option<&HashMap<i64, Vec<[i64; 2]>>> {
        self.slots.get(&ap_slot).map(|s| &s.placements)
    }
    /// Classification flags of the item a finder has at a named location
    /// (sender + location from a "sent" log line). None if unknown.
    pub fn item_flags(&self, finder_name: &str, loc_name: &str) -> Option<i64> {
        self.loc_flags.get(finder_name)?.get(loc_name).copied()
    }

    /// Look up an item id by its exact name in a game (inverse of item_name).
    pub fn item_id_by_name(&self, game: &str, name: &str) -> Option<i64> {
        self.item_names
            .get(game)?
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(id, _)| *id)
    }
}

/// Decode the current seed's metadata (blocks on a Python subprocess).
pub async fn load(season: i64) -> Result<Meta, String> {
    let dirs = config::get().season(season);
    ap_adapter::decode_meta(&dirs, &dirs.current_zip()).await
}
