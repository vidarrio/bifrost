//! Bifrost side of the PopTracker map feature: stores uploaded packs per slot
//! (extracted to disk) and computes the tracker state server-side via the
//! `poptracker` engine crate.
//!
//! The engine runs a pack's Lua (sandboxed) against the slot's live AP state and
//! returns a [`poptracker::TrackerState`] (maps + markers + items). We feed it
//! the slot's received items (resolved to names via `Meta`); checked-location
//! clearing is a later refinement (needs the per-slot checked-location ids).
//!
//! `poptracker::Session` holds a Lua VM and is `!Send`, so every compute runs
//! inside one synchronous call — handlers invoke these from `spawn_blocking`.

use std::path::PathBuf;

use crate::{ap_adapter, config};

/// Per-slot extracted-pack directory: `<data>/packs/<season>/<ap_slot>/`.
fn slot_dir(season: i64, ap_slot: i64) -> PathBuf {
    config::get()
        .packs_dir()
        .join(season.to_string())
        .join(ap_slot.to_string())
}

/// Is a usable pack installed for this slot?
pub fn has_pack(season: i64, ap_slot: i64) -> bool {
    poptracker::pack_root(&slot_dir(season, ap_slot)).is_ok()
}

/// A version stamp for the slot's pack (install dir mtime, epoch secs), used to
/// cache-bust asset URLs when a pack is (re)installed — browsers otherwise keep
/// showing cached images even across a hard refresh.
pub fn pack_version(season: i64, ap_slot: i64) -> u64 {
    std::fs::metadata(slot_dir(season, ap_slot))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Validate uploaded zip bytes: must load and expose an Archipelago autotracking
/// path (memory-only/emulator packs can't be driven server-side). Returns a
/// summary on success, else an error message suitable for showing the uploader.
pub fn validate(zip_bytes: &[u8]) -> Result<poptracker::Summary, String> {
    let tmp = write_temp("validate", zip_bytes)?;
    let res = (|| {
        let pack = poptracker::Pack::load(&tmp).map_err(|e| e.to_string())?;
        let session = poptracker::Session::new(&pack).map_err(|e| e.to_string())?;
        let sum = session.summary();
        if !sum.archipelago_autotracking {
            return Err(
                "this pack has no Archipelago auto-tracking — it only works with an \
                        emulator, so it can't be tracked here"
                    .to_string(),
            );
        }
        Ok(sum)
    })();
    let _ = std::fs::remove_file(&tmp);
    res
}

/// Install (extract) an uploaded pack zip for a slot, replacing any existing one.
pub fn install(season: i64, ap_slot: i64, zip_bytes: &[u8]) -> Result<(), String> {
    let dir = slot_dir(season, ap_slot);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir: {e}"))?;
    let tmp = write_temp("install", zip_bytes)?;
    let r = poptracker::extract_zip(&tmp, &dir).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&tmp);
    r?;
    // Browsers can render an indexed PNG with its palette background instead of
    // honouring `tRNS` transparency (notably under `image-rendering:pixelated`),
    // leaving transparent icons with a background — convert them to RGBA.
    // (Called from `spawn_blocking`, where blocking on the runtime is fine.)
    tokio::runtime::Handle::current().block_on(ap_adapter::normalize_pngs(&dir));
    Ok(())
}

/// Remove a slot's installed pack.
pub fn remove(season: i64, ap_slot: i64) {
    let _ = std::fs::remove_dir_all(slot_dir(season, ap_slot));
}

/// Compute the tracker state for a slot from its installed pack + received
/// items (`(item_id, item_name)`), with the slot's AP player number.
/// Synchronous (holds a Lua VM) — call via `spawn_blocking`.
pub fn compute(
    season: i64,
    ap_slot: i64,
    items: &[(i64, String)],
    checked: &[i64],
    valid_locations: &[i64],
    player: i64,
) -> Result<poptracker::TrackerState, String> {
    let root = poptracker::pack_root(&slot_dir(season, ap_slot)).map_err(|e| e.to_string())?;
    let pack = poptracker::Pack::load(&root).map_err(|e| e.to_string())?;
    let session = poptracker::Session::new(&pack).map_err(|e| e.to_string())?;
    session.set_player(player);
    // Feed the slot's valid location ids before onClear so `visibility_rules`
    // can hide markers for checks this slot's options never generated. Empty =>
    // left disabled (every marker shows), so a missing/old meta can't blank maps.
    if !valid_locations.is_empty() {
        session.set_valid_locations(valid_locations);
    }
    session
        .on_clear(&serde_json::json!({}))
        .map_err(|e| e.to_string())?;
    for (i, (id, name)) in items.iter().enumerate() {
        session
            .on_item(i as i64, *id, name, player)
            .map_err(|e| e.to_string())?;
    }
    // Feed checked locations so the pack's `onLocation` marks sections done
    // (→ grey "Done" markers). The name is unused by AP packs' handlers.
    for id in checked {
        session.on_location(*id, "").map_err(|e| e.to_string())?;
    }
    Ok(session.state())
}

/// Read a pack asset (e.g. a map image) for serving to the browser.
pub fn read_pack_asset(season: i64, ap_slot: i64, rel: &str) -> Option<Vec<u8>> {
    let root = poptracker::pack_root(&slot_dir(season, ap_slot)).ok()?;
    let pack = poptracker::Pack::load(&root).ok()?;
    pack.asset(rel)
}

/// Write an uploaded pack to a new, unguessable, owner-only file in the temp
/// dir (created fresh, so nothing another local user prepared is followed).
fn write_temp(tag: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = std::env::temp_dir().join(format!(
        "bifrost-pack-{tag}-{}.zip",
        &crate::auth::new_token()[..32]
    ));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("write temp: {e}"))?;
    if let Err(e) = f.write_all(bytes) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("write temp: {e}"));
    }
    Ok(path)
}
