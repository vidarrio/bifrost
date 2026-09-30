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
const CUSTOM: &str = "/srv/archipelago/custom_worlds";
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

/// Entry point: run the whole flow, updating `state` as it goes.
pub async fn run(db: Db, state: GenState) {
    match run_inner(&db, &state).await {
        Ok(msg) => set(&state, false, msg),
        Err(e) => set(&state, false, format!("Failed — {e}")),
    }
}

async fn run_inner(db: &Db, state: &GenState) -> Result<String, String> {
    let subs = db.submissions().await.map_err(|e| e.to_string())?;
    if subs.is_empty() {
        return Err("no submissions to generate from".into());
    }

    set(state, true, "Staging submissions…");
    clear_ext(PLAYERS, "yaml").await?;
    clear_ext(CUSTOM, "apworld").await?;
    clear_old_seeds().await;
    for sub in &subs {
        let dst = format!("{PLAYERS}/{}.yaml", slug(&sub.slot_name));
        tokio::fs::copy(&sub.yaml_file, &dst)
            .await
            .map_err(|e| format!("staging {}'s YAML: {e}", sub.slot_name))?;
        if let Some(aw) = &sub.apworld_file {
            let dst = format!("{CUSTOM}/{}.apworld", slug(&sub.slot_name));
            tokio::fs::copy(aw, &dst)
                .await
                .map_err(|e| format!("staging {}'s apworld: {e}", sub.slot_name))?;
        }
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
    let ap_slots: Vec<(i64, String, String)> = meta
        .get("slots")
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
        .unwrap_or_default();
    db.seed_from_submissions(&seed_name, &ap_slots, &subs)
        .await
        .map_err(|e| e.to_string())?;

    set(state, true, "Deploying…");
    tokio::fs::copy(&seed_zip, format!("{OUTPUT}/current.zip"))
        .await
        .map_err(|e| format!("deploy: {e}"))?;
    let _ = tokio::fs::remove_file(format!("{OUTPUT}/current.apsave")).await;

    Ok(format!(
        "Generated seed {seed_name} — {} AP slots. The game is restarting.",
        ap_slots.len()
    ))
}
