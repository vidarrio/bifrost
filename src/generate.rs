//! Admin "Generate" orchestration: stage the signups into the season dir, roll
//! the seed (via the AP adapter), map the result to Bifrost slots +
//! auto-claims, and deploy it as the season's `output/current.zip`. Writing
//! `current.zip` is what (re)starts the game: today an external watcher on that
//! file restarts the AP server; Bifrost never needs root.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::db::{Db, State};
use crate::meta::Meta;
use crate::runtime::Runtimes;
use crate::{ap_adapter, config};

/// Assets for the shared Minecraft "common game" (in the data dir's
/// `common/`), staged into the seed when the admin enables it (see
/// `Db::mc_enabled`). The host-side `mc-deploy` step then ships the resulting
/// `.apmc` to the Minecraft LXC.
const COMMON_APWORLD: &str = "minecraft.apworld";
const COMMON_YAML: &str = "Commons.yaml";
/// The Universal Tracker apworld, shipped in the runtime's `vendor/` and staged
/// into the season's `worlds/` on every deploy so the in-logic feature always
/// has it even after a reset clears the custom worlds. No YAML references it,
/// so it's inert during generation.
const TRACKER_APWORLD: &str = "tracker.apworld";
/// The AP slot name the common-game YAML declares — used to tag its Bifrost
/// slot communal.
pub const COMMON_SLOT_NAME: &str = "Commons";

/// Client-side Minecraft assets staged under `common/client/` so Bifrost can
/// serve them for download from the common-game page. Keep these in sync with
/// what's installed on the Minecraft host and the `MC_*` display strings in render.rs.
pub const MC_NEOFORGE_JAR: &str = "neoforge-26.2.0.90-installer.jar";
pub const MC_MOD_JAR: &str = "aprandomizer-2.2.1-26.2.jar";

/// (running, last message).
pub type GenState = Arc<Mutex<(bool, String)>>;

pub fn set(state: &GenState, running: bool, msg: impl Into<String>) {
    if let Ok(mut g) = state.lock() {
        g.0 = running;
        g.1 = msg.into();
    }
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

/// Remove files in `dir` with the given extension (e.g. "yaml").
async fn clear_ext(dir: &Path, ext: &str) -> Result<(), String> {
    let mut rd = tokio::fs::read_dir(dir)
        .await
        .map_err(|e| format!("read {}: {e}", dir.display()))?;
    while let Ok(Some(entry)) = rd.next_entry().await {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some(ext) {
            let _ = tokio::fs::remove_file(&p).await;
        }
    }
    Ok(())
}

/// Remove previously-generated seed archives (AP_*.zip), leaving current.zip.
async fn clear_old_seeds(season: i64) {
    if let Ok(mut rd) = tokio::fs::read_dir(config::get().season(season).output).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("AP_") && name.ends_with(".zip") {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
    }
}

/// The newest AP_*.zip in output/, if any.
async fn newest_seed(season: i64) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut rd = tokio::fs::read_dir(config::get().season(season).output).await.ok()?;
    while let Ok(Some(entry)) = rd.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if !(name.starts_with("AP_") && name.ends_with(".zip")) {
            continue;
        }
        let Ok(t) = entry.metadata().await.and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(bt, _)| t > *bt) {
            best = Some((t, entry.path()));
        }
    }
    best.map(|(_, p)| p)
}

// ---- generated per-player output files (for the admin to hand out) ----

/// A distributable file inside the generated seed (`output/current.zip`): either
/// a per-player patch/output file or the spoiler. The `.archipelago` multidata
/// (the server file) is never exposed.
pub struct OutputFile {
    /// Exact zip member name — the download key, validated on the way back out.
    pub member: String,
    /// Slot name parsed from the filename (for the spoiler: "Spoiler").
    pub label: String,
    /// File extension (world-specific, e.g. `apbg3`, `apmc`), or `txt`.
    pub ext: String,
    pub is_spoiler: bool,
    pub size: u64,
    /// AP slot number parsed from `P<n>` — matches a file to its game page.
    /// `None` for the spoiler.
    pub player: Option<i64>,
}

/// List the player-facing files in the current seed zip: per-player output files
/// (see [`parse_player_file`] for the naming schemes handled) plus the spoiler.
/// Excludes the `.archipelago` multidata (the server file). Blocking — call from
/// `spawn_blocking`. Empty if no seed has been generated yet.
pub fn list_outputs(season: i64) -> Vec<OutputFile> {
    let Ok(f) = std::fs::File::open(config::get().season(season).current_zip()) else {
        return Vec::new();
    };
    let Ok(mut zip) = zip::ZipArchive::new(f) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let Ok(e) = zip.by_index(i) else { continue };
        let Ok(name) = e.name() else { continue };
        let name = name.to_string();
        let size = e.size();
        if name.ends_with(".archipelago") {
            continue; // server multidata, not for players
        }
        if name.ends_with("_Spoiler.txt") {
            out.push(OutputFile {
                member: name,
                label: "Spoiler".into(),
                ext: "txt".into(),
                is_spoiler: true,
                size,
                player: None,
            });
            continue;
        }
        if let Some((num, slot, ext)) = parse_player_file(&name) {
            out.push(OutputFile {
                member: name,
                label: slot,
                ext,
                is_spoiler: false,
                size,
                player: Some(num),
            });
        }
    }
    // Players first (A→Z), spoiler last.
    out.sort_by(|a, b| {
        a.is_spoiler
            .cmp(&b.is_spoiler)
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });
    out
}

/// Parse an AP per-player output filename into (slot number, slot label, ext).
/// Handles the standard `AP_<seed>_P<n>_<slot>.<ext>` AND world-specific variants
/// that use `-` as the separator and/or append `_<apversion>` before the ext —
/// e.g. shapez 2 writes `AP-<seed>-P<n>-<slot>_0.6.8.zip`. Seed and player number
/// are pure digits, so the boundaries stay unambiguous even when the slot name
/// itself contains underscores.
fn parse_player_file(name: &str) -> Option<(i64, String, String)> {
    let s = name.strip_prefix("AP")?;
    let s = strip_sep(s)?; // separator after the "AP" prefix
    let seed_end = s.find(|c: char| !c.is_ascii_digit())?;
    if seed_end == 0 {
        return None; // seed must be at least one digit
    }
    let s = strip_sep(&s[seed_end..])?; // separator after the seed
    let s = s.strip_prefix('P')?;
    let num_end = s.find(|c: char| !c.is_ascii_digit())?;
    let num: i64 = s[..num_end].parse().ok()?;
    let s = strip_sep(&s[num_end..])?; // separator after P<n>
    let dot = s.rfind('.')?;
    let ext = s[dot + 1..].to_string();
    let label = strip_version_suffix(&s[..dot]).to_string();
    Some((num, label, ext))
}

/// Strip a single leading `_` or `-` separator (AP output filenames use either).
fn strip_sep(s: &str) -> Option<&str> {
    let mut ch = s.chars();
    matches!(ch.next(), Some('_' | '-')).then(|| ch.as_str())
}

/// Drop a trailing `_<version>` tag some worlds append to the slot name (e.g.
/// shapez 2 adds `_0.6.8`). Only a tail of digits-and-dots containing a dot is
/// treated as a version, so a slot like `Player_2` is left intact.
fn strip_version_suffix(label: &str) -> &str {
    if let Some(idx) = label.rfind('_') {
        let tail = &label[idx + 1..];
        if tail.contains('.') && tail.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return &label[..idx];
        }
    }
    label
}

/// The AP slot number a per-player output file belongs to (`P<n>` in the name),
/// used to gate a claimant to only their own slot's files. `None` for the
/// spoiler or anything that isn't a per-player file.
pub fn member_player(name: &str) -> Option<i64> {
    parse_player_file(name).map(|(n, _, _)| n)
}

/// Read a staged Minecraft client asset for download: `which` is "neoforge" (the
/// NeoForge installer) or "mod" (the AP mod jar). Returns (download filename,
/// bytes), or `None` if unknown or not staged. Blocking — call from
/// `spawn_blocking`.
pub fn read_client_asset(which: &str) -> Option<(&'static str, Vec<u8>)> {
    let name = match which {
        "neoforge" => MC_NEOFORGE_JAR,
        "mod" => MC_MOD_JAR,
        _ => return None,
    };
    let bytes = std::fs::read(config::get().common_dir().join("client").join(name)).ok()?;
    Some((name, bytes))
}

/// Read one member's bytes out of the current seed zip, but only if `member` is
/// actually a file in it (so a crafted query can't pull anything arbitrary), and
/// never the `.archipelago` server file. Blocking — call from `spawn_blocking`.
pub fn read_output(season: i64, member: &str) -> Option<Vec<u8>> {
    if member.ends_with(".archipelago") {
        return None;
    }
    let f = std::fs::File::open(config::get().season(season).current_zip()).ok()?;
    let mut zip = zip::ZipArchive::new(f).ok()?;
    let mut e = zip.by_name(member).ok()?; // errors if not a member
    let mut buf = Vec::with_capacity(e.size() as usize);
    std::io::Read::read_to_end(&mut e, &mut buf).ok()?;
    Some(buf)
}

// ---- admin maintenance actions (each writes only inside the season dir) ----

/// Remove every installed custom apworld from the season's worlds/.
pub async fn clear_apworlds(season: i64) -> Result<(), String> {
    let season = config::get().season(season);
    clear_ext(&season.worlds, "apworld").await?;
    ap_adapter::unpin_world(&season, None)
}

/// Stage the Universal Tracker apworld into the season's worlds/ from the
/// runtime's vendored copy. Called on every deploy so the in-logic feature
/// survives a season reset (which clears the custom worlds). Non-fatal: if the
/// vendored copy is missing, the season still generates — UT in-logic just
/// won't update.
async fn stage_tracker_apworld(season: i64) {
    let cfg = config::get();
    let src = cfg.vendor_file(TRACKER_APWORLD);
    let dst = cfg.season(season).worlds.join(TRACKER_APWORLD);
    if let Err(e) = tokio::fs::copy(&src, &dst).await {
        tracing::warn!(
            "staging Universal Tracker apworld ({} -> {}): {e}",
            src.display(),
            dst.display()
        );
    }
}

/// Remove every staged player YAML from the season's Players/.
pub async fn clear_player_yamls(season: i64) -> Result<(), String> {
    clear_ext(&config::get().season(season).players, "yaml").await
}

/// Delete the running session's save (a fresh start on the next server load).
pub async fn clear_save(season: i64) {
    let _ = tokio::fs::remove_file(config::get().season(season).current_save()).await;
}

/// Delete the deployed world (current.zip) and archived seeds (AP_*.zip).
pub async fn clear_world(season: i64) {
    let _ = tokio::fs::remove_file(config::get().season(season).current_zip()).await;
    clear_old_seeds(season).await;
}

/// Remove one installed apworld by file name (no path separators allowed).
pub async fn delete_apworld(season: i64, name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains('/') || !name.ends_with(".apworld") {
        return Err("invalid apworld name".into());
    }
    let season = config::get().season(season);
    tokio::fs::remove_file(season.worlds.join(name))
        .await
        .map_err(|e| format!("remove {name}: {e}"))?;
    ap_adapter::unpin_world(&season, Some(name))
}

/// The installed apworlds as (file name, provenance line): shipped with
/// Bifrost, or pinned hash + uploader — or flagged when unpinned/changed.
pub async fn list_apworlds(season: i64) -> Vec<(String, String)> {
    let season = config::get().season(season);
    let custom = ap_adapter::custom_worlds(&season);
    let pins = ap_adapter::world_pins(&season);
    let mut out = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&season.worlds).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".apworld") {
                continue;
            }
            let detail = if !custom.contains(&name) {
                "shipped with Bifrost".to_string()
            } else {
                let actual = tokio::fs::read(entry.path()).await.map(|b| ap_adapter::sha256_hex(&b));
                match (pins.get(&name), actual) {
                    (Some(p), Ok(h)) if p.sha256 == h => {
                        format!("sha256 {}… · uploaded by {}", &h[..12], p.uploaded_by)
                    }
                    (Some(_), _) => "⚠ changed on disk since upload — won't run".to_string(),
                    (None, _) => "⚠ not installed through Bifrost — won't run".to_string(),
                }
            };
            out.push((name, detail));
        }
    }
    out.sort();
    out
}

/// The seed's AP roster as (slot number, player name, game).
fn ap_slots(meta: &Meta) -> Vec<(i64, String, String)> {
    meta.slots
        .iter()
        .filter(|(_, s)| !s.name.is_empty())
        .map(|(num, s)| {
            let game = if s.game.is_empty() { "?" } else { &s.game };
            (*num, s.name.clone(), game.to_string())
        })
        .collect()
}

/// Decode a freshly rolled seed: its name and AP roster.
async fn read_seed(
    season: i64,
    seed_zip: &Path,
) -> Result<(String, Vec<(i64, String, String)>), String> {
    let meta = ap_adapter::decode_meta(&config::get().season(season), seed_zip).await?;
    let seed_name = meta
        .seed
        .clone()
        .ok_or("seed metadata missing its name")?;
    Ok((seed_name, ap_slots(&meta)))
}

/// Entry point: generate `season` from its signups, moving it through
/// `generating` to `live` (and starting its workers) — or back to where it was
/// on failure. Updates `state` as it goes.
pub async fn run(db: Db, rts: Arc<Runtimes>, season: i64) {
    let state = rts.get(season).genstate.clone();
    let Some(prev) = db.season(season).await.map(|s| s.state()) else {
        return set(&state, false, "Failed — no such season");
    };
    // Refuse up front rather than generate a world that can't go live.
    if let Some(live) = db
        .seasons()
        .await
        .into_iter()
        .find(|s| s.state() == State::Live && s.id != season)
    {
        return set(
            &state,
            false,
            format!(
                "Failed — season “{}” is live; only one season can be live at a time for now",
                live.name
            ),
        );
    }
    if let Err(e) = db.transition(season, State::Generating, now()).await {
        return set(&state, false, format!("Failed — {e}"));
    }
    match run_inner(&db, &rts, &state, season).await {
        Ok(msg) => match db.transition(season, State::Live, now()).await {
            Ok(()) => {
                rts.start_live(season).await;
                set(&state, false, msg)
            }
            Err(e) => {
                let _ = db.transition(season, prev, now()).await;
                set(&state, false, format!("Generated, but couldn't go live — {e}"))
            }
        },
        Err(e) => {
            let _ = db.transition(season, prev, now()).await;
            set(&state, false, format!("Failed — {e}"))
        }
    }
}

/// Re-roll a live season's lineup: reuse its staged Players/ + installed
/// worlds/, generate a fresh seed (and fresh save, picking up the current
/// host.yaml), and remap its slots + claims onto it. Doesn't touch
/// submissions.
pub async fn regenerate(db: Db, rts: Arc<Runtimes>, season: i64) {
    let state = rts.get(season).genstate.clone();
    if let Err(e) = db.transition(season, State::Generating, now()).await {
        return set(&state, false, format!("Failed — {e}"));
    }
    let r = regen_inner(&db, &rts, &state, season).await;
    // Live either way: on failure the previous seed is still deployed.
    let _ = db.transition(season, State::Live, now()).await;
    match r {
        Ok(msg) => set(&state, false, msg),
        Err(e) => set(&state, false, format!("Failed — {e}")),
    }
}

fn now() -> i64 {
    crate::auth::now()
}

async fn regen_inner(db: &Db, rts: &Runtimes, state: &GenState, season: i64) -> Result<String, String> {
    // Must have a staged lineup to re-roll.
    let mut has_yaml = false;
    if let Ok(mut rd) = tokio::fs::read_dir(config::get().season(season).players).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            if e.path().extension().and_then(|x| x.to_str()) == Some("yaml") {
                has_yaml = true;
                break;
            }
        }
    }
    if !has_yaml {
        return Err("no staged player YAMLs to re-roll — generate the season first".into());
    }

    set(
        state,
        true,
        "Re-rolling the current lineup… (this can take a minute)",
    );
    clear_old_seeds(season).await;
    // Always keep the Universal Tracker apworld present for the in-logic feature.
    stage_tracker_apworld(season).await;
    ap_adapter::generate(&config::get().season(season)).await?;

    let seed_zip = newest_seed(season).await.ok_or("no seed was produced")?;
    set(state, true, "Mapping slots and carrying over claims…");
    let (seed_name, ap_slots) = read_seed(season, &seed_zip).await?;
    // The new seed is the season's from here: the tracker accepts the restarted
    // server, and data derived from the old seed is dropped.
    db.set_season_seed(season, &seed_name).await.map_err(|e| e.to_string())?;
    db.clear_seed_data(season).await.map_err(|e| e.to_string())?;
    db.remap_season(season, &ap_slots).await.map_err(|e| e.to_string())?;

    set(state, true, "Deploying…");
    rts.redeploy(season, &seed_zip).await?;

    Ok(format!(
        "Regenerated seed {seed_name} — {} AP slots, claims carried over. The game is restarting on a fresh save.",
        ap_slots.len()
    ))
}

async fn run_inner(db: &Db, rts: &Runtimes, state: &GenState, season: i64) -> Result<String, String> {
    let subs = db.submissions(season).await.map_err(|e| e.to_string())?;
    if subs.is_empty() {
        return Err("no submissions to generate from".into());
    }

    set(state, true, "Staging submissions…");
    // Only the player YAMLs are staged here; custom apworlds are installed into
    // the season's worlds/ at signup time (so the checker can validate against them),
    // so we leave that directory alone.
    let mc = db.season(season).await.is_some_and(|s| s.mc_enabled);
    let season_id = season;
    let season = config::get().season(season);
    let common = config::get().common_dir();
    clear_ext(&season.players, "yaml").await?;
    clear_old_seeds(season_id).await;
    for sub in &subs {
        let dst = season.players.join(format!("{}.yaml", slug(&sub.slot_name)));
        tokio::fs::copy(&sub.yaml_file, &dst)
            .await
            .map_err(|e| format!("staging {}'s YAML: {e}", sub.slot_name))?;
    }

    // Shared Minecraft common game: if the admin enabled it before generating,
    // stage its apworld + YAML so the roll includes the communal slot. The
    // resulting .apmc is shipped to the Minecraft LXC afterwards by `mc-deploy`.
    if mc {
        set(state, true, "Adding the Minecraft common game…");
        tokio::fs::copy(
            common.join(COMMON_APWORLD),
            season.worlds.join(COMMON_APWORLD),
        )
        .await
        .map_err(|e| format!("staging the Minecraft apworld: {e}"))?;
        tokio::fs::copy(common.join(COMMON_YAML), season.players.join(COMMON_YAML))
        .await
        .map_err(|e| format!("staging the common-game YAML: {e}"))?;
    }

    // Always keep the Universal Tracker apworld present for the in-logic feature.
    stage_tracker_apworld(season_id).await;

    set(
        state,
        true,
        "Generating the multiworld… (this can take a minute)",
    );
    ap_adapter::generate(&season).await?;

    let seed_zip = newest_seed(season_id).await.ok_or("no seed was produced")?;
    set(state, true, "Mapping slots and claims…");
    let (seed_name, ap_slots) = read_seed(season_id, &seed_zip).await?;
    db.set_season_seed(season_id, &seed_name).await.map_err(|e| e.to_string())?;
    db.seed_from_submissions(
        season_id,
        &ap_slots,
        &subs,
        mc.then_some(COMMON_SLOT_NAME),
    )
    .await
    .map_err(|e| e.to_string())?;

    set(state, true, "Deploying…");
    rts.redeploy(season_id, &seed_zip).await?;

    let common = if mc {
        " The Minecraft common game is included — run “Deploy common game” (manage → archipelago) to ship it to the Minecraft server."
    } else {
        ""
    };
    Ok(format!(
        "Generated seed {seed_name} — {} AP slots. The game is restarting. Signups are now closed.{common}",
        ap_slots.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::{member_player, parse_player_file};

    #[test]
    fn parses_the_standard_underscore_name() {
        let (num, slot, ext) =
            parse_player_file("AP_94805671998530581428_P12_MissMandalyn.apcivvi").unwrap();
        assert_eq!((num, slot.as_str(), ext.as_str()), (12, "MissMandalyn", "apcivvi"));
    }

    #[test]
    fn parses_the_common_game_apmc() {
        let (num, slot, ext) =
            parse_player_file("AP_94805671998530581428_P3_Commons.apmc").unwrap();
        assert_eq!((num, slot.as_str(), ext.as_str()), (3, "Commons", "apmc"));
    }

    #[test]
    fn parses_shapez2_dash_and_version_variant() {
        // shapez 2: dash separators + a trailing `_<apversion>` before `.zip`.
        let (num, slot, ext) =
            parse_player_file("AP-94805671998530581428-P14-Nid_0.6.8.zip").unwrap();
        assert_eq!((num, slot.as_str(), ext.as_str()), (14, "Nid", "zip"));
        assert_eq!(member_player("AP-94805671998530581428-P14-Nid_0.6.8.zip"), Some(14));
    }

    #[test]
    fn keeps_underscores_in_a_slot_name() {
        let (num, slot, _) = parse_player_file("AP_123_P2_Some_Long_Name.apbg3").unwrap();
        assert_eq!((num, slot.as_str()), (2, "Some_Long_Name"));
    }

    #[test]
    fn does_not_strip_a_non_version_underscore_tail() {
        // A trailing `_2` (no dot) is part of the name, not a version tag.
        let (_, slot, _) = parse_player_file("AP_123_P2_Player_2.zip").unwrap();
        assert_eq!(slot, "Player_2");
    }

    #[test]
    fn rejects_non_player_files() {
        assert_eq!(parse_player_file("AP_123_Spoiler.txt"), None);
        assert_eq!(parse_player_file("AP_123.archipelago"), None);
        assert_eq!(parse_player_file("random.txt"), None);
    }
}
