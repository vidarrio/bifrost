//! Admin "Generate" orchestration: stage the signups, roll the seed, map the
//! result to Bifrost slots + auto-claims, and deploy it. Runs as the bifrost
//! user (which has write access to Players/, custom_worlds/, output/ only);
//! writing `current.zip` triggers the root `archipelago-reload.path` unit that
//! restarts the game — Bifrost never needs root.

use std::process::Stdio;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::process::Command;

use crate::db::Db;

const AP_DIR: &str = "/srv/archipelago";
const AP_PYTHON: &str = "/srv/archipelago/venv/bin/python";
const GEN_SCRIPT: &str = "/srv/archipelago/bifrost_generate.py";
const META_SCRIPT: &str = "/srv/archipelago/bifrost_meta.py";
const PLAYERS: &str = "/srv/archipelago/Players";
const CUSTOM_WORLDS: &str = "/srv/archipelago/custom_worlds";
const OUTPUT: &str = "/srv/archipelago/output";

/// Canonical assets for the shared Minecraft "common game", staged into the
/// seed when the admin enables it (see `Db::mc_enabled`). The host-side
/// `mc-deploy` step then ships the resulting `.apmc` to the Minecraft LXC.
const COMMON_DIR: &str = "/srv/archipelago/common";
const COMMON_APWORLD: &str = "minecraft.apworld";
const COMMON_YAML: &str = "Commons.yaml";
/// The AP slot name the common-game YAML declares — used to tag its Bifrost
/// slot communal.
pub const COMMON_SLOT_NAME: &str = "Commons";

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
async fn clear_ext(dir: &str, ext: &str) -> Result<(), String> {
    let mut rd = tokio::fs::read_dir(dir)
        .await
        .map_err(|e| format!("read {dir}: {e}"))?;
    while let Ok(Some(entry)) = rd.next_entry().await {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some(ext) {
            let _ = tokio::fs::remove_file(&p).await;
        }
    }
    Ok(())
}

/// Remove previously-generated seed archives (AP_*.zip), leaving current.zip.
async fn clear_old_seeds() {
    if let Ok(mut rd) = tokio::fs::read_dir(OUTPUT).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("AP_") && name.ends_with(".zip") {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
    }
}

/// Deploy a rolled seed as `current.zip` (which fires the root reload path unit)
/// and drop the old save. Removing the existing file first is essential: the old
/// `current.zip` is owned by the `archipelago` user, and `std::fs::copy` would
/// try to chmod it to match the source — which fails with EPERM for `bifrost`
/// even though the file is group-writable. Removing it (the dir is writable)
/// lets the copy create a fresh bifrost-owned file.
async fn deploy_seed(seed_zip: &str) -> Result<(), String> {
    let dst = format!("{OUTPUT}/current.zip");
    let _ = tokio::fs::remove_file(&dst).await;
    tokio::fs::copy(seed_zip, &dst)
        .await
        .map_err(|e| format!("deploy: {e}"))?;
    // Best-effort save wipe. The AUTHORITATIVE wipe lives in
    // archipelago-reload.service, which (on the reload this copy just fired)
    // stops the old server, deletes current.apsave *after* its shutdown
    // auto-save, then starts fresh. Deleting here can't win that race — the
    // dying old server re-writes current.apsave on SIGTERM with the previous
    // seed's hints/checks, and without the reload-unit delete the new server
    // would load those stale hints against the new (different) placements.
    let _ = tokio::fs::remove_file(format!("{OUTPUT}/current.apsave")).await;
    Ok(())
}

/// The newest AP_*.zip in output/, if any.
async fn newest_seed() -> Option<String> {
    let mut best: Option<(std::time::SystemTime, String)> = None;
    let mut rd = tokio::fs::read_dir(OUTPUT).await.ok()?;
    while let Ok(Some(entry)) = rd.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if !(name.starts_with("AP_") && name.ends_with(".zip")) {
            continue;
        }
        let Ok(t) = entry.metadata().await.and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(bt, _)| t > *bt) {
            best = Some((t, entry.path().to_string_lossy().to_string()));
        }
    }
    best.map(|(_, p)| p)
}

async fn run_meta(seed_zip: &str) -> Result<Value, String> {
    let out = Command::new(AP_PYTHON)
        .arg(META_SCRIPT)
        .arg(seed_zip)
        .current_dir(AP_DIR)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| format!("spawn meta: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    serde_json::from_str(line.trim()).map_err(|e| format!("parse meta: {e}"))
}

// ---- generated per-player output files (for the admin to hand out) ----

const CURRENT_ZIP: &str = "/srv/archipelago/output/current.zip";

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
/// named `AP_<seed>_P<n>_<slot>.<ext>` plus the spoiler. Excludes the
/// `.archipelago` multidata (the server file). Blocking — call from
/// `spawn_blocking`. Empty if no seed has been generated yet.
pub fn list_outputs() -> Vec<OutputFile> {
    let Ok(f) = std::fs::File::open(CURRENT_ZIP) else {
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

/// Parse `AP_<seed>_P<n>_<slot>.<ext>` into (slot, ext). The seed and player
/// number are pure digits, so the boundaries are unambiguous even when the slot
/// name itself contains underscores.
fn parse_player_file(name: &str) -> Option<(i64, String, String)> {
    let s = name.strip_prefix("AP_")?;
    let seed_end = s.find(|c: char| !c.is_ascii_digit())?;
    let s = s[seed_end..].strip_prefix("_P")?;
    let num_end = s.find(|c: char| !c.is_ascii_digit())?;
    let num: i64 = s[..num_end].parse().ok()?;
    let s = s[num_end..].strip_prefix('_')?;
    let dot = s.rfind('.')?;
    Some((num, s[..dot].to_string(), s[dot + 1..].to_string()))
}

/// The AP slot number a per-player output file belongs to (`P<n>` in the name),
/// used to gate a claimant to only their own slot's files. `None` for the
/// spoiler or anything that isn't a per-player file.
pub fn member_player(name: &str) -> Option<i64> {
    parse_player_file(name).map(|(n, _, _)| n)
}

/// Read one member's bytes out of the current seed zip, but only if `member` is
/// actually a file in it (so a crafted query can't pull anything arbitrary), and
/// never the `.archipelago` server file. Blocking — call from `spawn_blocking`.
pub fn read_output(member: &str) -> Option<Vec<u8>> {
    if member.ends_with(".archipelago") {
        return None;
    }
    let f = std::fs::File::open(CURRENT_ZIP).ok()?;
    let mut zip = zip::ZipArchive::new(f).ok()?;
    let mut e = zip.by_name(member).ok()?; // errors if not a member
    let mut buf = Vec::with_capacity(e.size() as usize);
    std::io::Read::read_to_end(&mut e, &mut buf).ok()?;
    Some(buf)
}

// ---- admin maintenance actions (each writes only inside our ReadWritePaths) ----

/// Remove every installed custom apworld from custom_worlds/.
pub async fn clear_apworlds() -> Result<(), String> {
    clear_ext(CUSTOM_WORLDS, "apworld").await
}

/// Remove every staged player YAML from Players/.
pub async fn clear_player_yamls() -> Result<(), String> {
    clear_ext(PLAYERS, "yaml").await
}

/// Delete the running session's save (a fresh start on the next server load).
pub async fn clear_save() {
    let _ = tokio::fs::remove_file(format!("{OUTPUT}/current.apsave")).await;
}

/// Delete the deployed world (current.zip) and archived seeds (AP_*.zip).
pub async fn clear_world() {
    let _ = tokio::fs::remove_file(format!("{OUTPUT}/current.zip")).await;
    clear_old_seeds().await;
}

/// Remove one installed apworld by file name (no path separators allowed).
pub async fn delete_apworld(name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains('/') || !name.ends_with(".apworld") {
        return Err("invalid apworld name".into());
    }
    tokio::fs::remove_file(format!("{CUSTOM_WORLDS}/{name}"))
        .await
        .map_err(|e| format!("remove {name}: {e}"))
}

/// The installed custom apworlds, by file name.
pub async fn list_apworlds() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(CUSTOM_WORLDS).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".apworld") {
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

/// The seed's AP roster as (slot number, player name, game) from meta JSON.
fn parse_ap_slots(meta: &Value) -> Vec<(i64, String, String)> {
    meta.get("slots")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| {
                    let num = k.parse::<i64>().ok()?;
                    let name = v.get("name")?.as_str()?.to_string();
                    let game = v
                        .get("game")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                        .to_string();
                    Some((num, name, game))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Entry point: run the whole flow, updating `state` as it goes.
pub async fn run(db: Db, state: GenState) {
    match run_inner(&db, &state).await {
        Ok(msg) => set(&state, false, msg),
        Err(e) => set(&state, false, format!("Failed — {e}")),
    }
}

/// Re-roll the *current* lineup: reuse the already-staged Players/ + installed
/// custom_worlds/, generate a fresh seed (and fresh save, picking up the current
/// host.yaml), and carry the existing slots + claims onto it. Doesn't touch
/// submissions or signups.
pub async fn regenerate(db: Db, state: GenState, old_season: String) {
    match regen_inner(&db, &state, &old_season).await {
        Ok(msg) => set(&state, false, msg),
        Err(e) => set(&state, false, format!("Failed — {e}")),
    }
}

async fn regen_inner(db: &Db, state: &GenState, old_season: &str) -> Result<String, String> {
    // Must have a staged lineup to re-roll.
    let mut has_yaml = false;
    if let Ok(mut rd) = tokio::fs::read_dir(PLAYERS).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            if e.path().extension().and_then(|x| x.to_str()) == Some("yaml") {
                has_yaml = true;
                break;
            }
        }
    }
    if !has_yaml {
        return Err("no staged player YAMLs to re-roll — generate a season first".into());
    }

    set(
        state,
        true,
        "Re-rolling the current lineup… (this can take a minute)",
    );
    clear_old_seeds().await;
    let out = Command::new(AP_PYTHON)
        .arg(GEN_SCRIPT)
        .current_dir(AP_DIR)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| format!("spawn generate: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let mut tail: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).collect();
        let tail = tail.split_off(tail.len().saturating_sub(3)).join(" | ");
        return Err(format!("generation error: {tail}"));
    }

    let seed_zip = newest_seed().await.ok_or("no seed was produced")?;
    set(state, true, "Mapping slots and carrying over claims…");
    let meta = run_meta(&seed_zip).await?;
    let seed_name = meta
        .get("seed")
        .and_then(Value::as_str)
        .ok_or("seed metadata missing its name")?
        .to_string();
    let ap_slots = parse_ap_slots(&meta);
    db.carry_over_season(old_season, &seed_name, &ap_slots)
        .await
        .map_err(|e| e.to_string())?;

    set(state, true, "Deploying…");
    deploy_seed(&seed_zip).await?;

    Ok(format!(
        "Regenerated seed {seed_name} — {} AP slots, claims carried over. The game is restarting on a fresh save.",
        ap_slots.len()
    ))
}

async fn run_inner(db: &Db, state: &GenState) -> Result<String, String> {
    let subs = db.submissions().await.map_err(|e| e.to_string())?;
    if subs.is_empty() {
        return Err("no submissions to generate from".into());
    }

    set(state, true, "Staging submissions…");
    // Only the player YAMLs are staged here; custom apworlds are installed into
    // custom_worlds/ at signup time (so the checker can validate against them),
    // so we leave that directory alone.
    clear_ext(PLAYERS, "yaml").await?;
    clear_old_seeds().await;
    for sub in &subs {
        let dst = format!("{PLAYERS}/{}.yaml", slug(&sub.slot_name));
        tokio::fs::copy(&sub.yaml_file, &dst)
            .await
            .map_err(|e| format!("staging {}'s YAML: {e}", sub.slot_name))?;
    }

    // Shared Minecraft common game: if the admin enabled it before generating,
    // stage its apworld + YAML so the roll includes the communal slot. The
    // resulting .apmc is shipped to the Minecraft LXC afterwards by `mc-deploy`.
    let mc = db.mc_enabled().await;
    if mc {
        set(state, true, "Adding the Minecraft common game…");
        tokio::fs::copy(
            format!("{COMMON_DIR}/{COMMON_APWORLD}"),
            format!("{CUSTOM_WORLDS}/{COMMON_APWORLD}"),
        )
        .await
        .map_err(|e| format!("staging the Minecraft apworld: {e}"))?;
        tokio::fs::copy(
            format!("{COMMON_DIR}/{COMMON_YAML}"),
            format!("{PLAYERS}/{COMMON_YAML}"),
        )
        .await
        .map_err(|e| format!("staging the common-game YAML: {e}"))?;
    }

    set(
        state,
        true,
        "Generating the multiworld… (this can take a minute)",
    );
    let out = Command::new(AP_PYTHON)
        .arg(GEN_SCRIPT)
        .current_dir(AP_DIR)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| format!("spawn generate: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let mut tail: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).collect();
        let tail = tail.split_off(tail.len().saturating_sub(3)).join(" | ");
        return Err(format!("generation error: {tail}"));
    }

    let seed_zip = newest_seed().await.ok_or("no seed was produced")?;
    set(state, true, "Mapping slots and claims…");
    let meta = run_meta(&seed_zip).await?;
    let seed_name = meta
        .get("seed")
        .and_then(Value::as_str)
        .ok_or("seed metadata missing its name")?
        .to_string();
    let ap_slots = parse_ap_slots(&meta);
    db.seed_from_submissions(
        &seed_name,
        &ap_slots,
        &subs,
        mc.then_some(COMMON_SLOT_NAME),
    )
    .await
    .map_err(|e| e.to_string())?;

    set(state, true, "Deploying…");
    deploy_seed(&seed_zip).await?;

    // A clean generate closes signups for the season now under way; the admin
    // reopens them for the next signup period.
    let _ = db.set_signups_open(false).await;

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
