//! Season metadata: static facts decoded from the seed (`output/current.zip`)
//! by `bifrost_meta.py` — per-slot item totals and id→name maps for items and
//! locations. Loaded once per season (subprocess) and cached in the board.

use std::collections::{BTreeMap, HashMap};
use std::process::Stdio;

use serde::Deserialize;
use tokio::process::Command;

const AP_PYTHON: &str = "/srv/archipelago/venv/bin/python";
const AP_DIR: &str = "/srv/archipelago";
const META_SCRIPT: &str = "/srv/archipelago/bifrost_meta.py";
const SEED_ZIP: &str = "/srv/archipelago/output/current.zip";
const COUNTS_SCRIPT: &str = "/srv/archipelago/bifrost_counts.py";

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

/// Read every slot's true sent/received totals from `output/current.apsave`
/// (blocks on a Python subprocess). Returns an empty map before the server has
/// written a save.
pub async fn load_counts() -> Result<HashMap<i64, SlotCounts>, String> {
    let output = Command::new(AP_PYTHON)
        .arg(COUNTS_SCRIPT)
        .current_dir(AP_DIR)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| format!("spawn counts: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("{}");
    // Keys are stringified slot numbers; collect into an i64-keyed map.
    let raw: HashMap<String, SlotCounts> =
        serde_json::from_str(line.trim()).map_err(|e| format!("parse counts: {e}"))?;
    Ok(raw
        .into_iter()
        .filter_map(|(k, v)| k.parse::<i64>().ok().map(|n| (n, v)))
        .collect())
}

#[derive(Debug, Deserialize)]
pub struct SlotMeta {
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
}

#[derive(Debug, Deserialize)]
pub struct Meta {
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
pub async fn load() -> Result<Meta, String> {
    let output = Command::new(AP_PYTHON)
        .arg(META_SCRIPT)
        .arg(SEED_ZIP)
        .current_dir(AP_DIR)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| format!("spawn meta: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    serde_json::from_str::<Meta>(line.trim()).map_err(|e| format!("parse meta: {e}"))
}
