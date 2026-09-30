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
    db.seed_from_submissions(&seed_name, &ap_slots, &subs)
        .await
        .map_err(|e| e.to_string())?;

    set(state, true, "Deploying…");
    deploy_seed(&seed_zip).await?;

    // A clean generate closes signups for the season now under way; the admin
    // reopens them for the next signup period.
    let _ = db.set_signups_open(false).await;

    Ok(format!(
        "Generated seed {seed_name} — {} AP slots. The game is restarting. Signups are now closed.",
        ap_slots.len()
    ))
}
