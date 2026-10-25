//! Universal-Tracker in-logic recomputer.
//!
//! The `/game/<slot>` page shows which of a slot's locations are reachable right
//! now ("in logic"). AP's multidata has no such field — it has to be derived by
//! running each world's logic against the slot's current items. We do that out
//! of band: shell out to `bifrost_inlogic.py` (sibling of `bifrost_check.py`),
//! which drives the installed Universal Tracker apworld headlessly, connects to
//! the live room as a read-only tracker for one slot, and writes that slot's
//! in-logic location names as JSON. We cache the result per slot in the DB; the
//! game page just reads the cache.
//!
//! Cost control: each run loads ~300 MB + does a full regen (~4 s), so we run at
//! most ONE at a time (the loop is sequential) and recompute a slot only when its
//! `(sent, received)` state has actually moved since the last cached result.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::process::Command;

use crate::ap::{ApConfig, Board};
use crate::db::Db;

const AP_PYTHON: &str = "/srv/archipelago/venv/bin/python";
const AP_DIR: &str = "/srv/archipelago";
const SCRIPT: &str = "/srv/archipelago/bifrost_inlogic.py";
/// How often to look for slots whose state moved. The actual UT runs only fire
/// for changed slots, so an idle board costs one cheap state comparison a cycle.
const POLL: Duration = Duration::from_secs(20);
/// Hard cap per UT run (the script has its own 120 s internal cap; this is the
/// outer safety net so a wedged child can't stall the loop forever).
const RUN_TIMEOUT: Duration = Duration::from_secs(150);

/// Spawn the recomputer. No-op without an AP config (nothing to connect to).
pub fn spawn(board: Arc<RwLock<Board>>, db: Db, cfg: Option<ApConfig>) {
    let Some(cfg) = cfg else {
        return;
    };
    // UT connects with host:port, not the ws:// URL Bifrost uses for its own
    // client, so strip the scheme once.
    let connect = cfg
        .url
        .strip_prefix("wss://")
        .or_else(|| cfg.url.strip_prefix("ws://"))
        .unwrap_or(&cfg.url)
        .trim_end_matches('/')
        .to_string();
    let password = cfg.password.clone();
    tokio::spawn(async move {
        loop {
            if let Err(e) = tick(&board, &db, &connect, &password).await {
                tracing::warn!("inlogic: {e}");
            }
            tokio::time::sleep(POLL).await;
        }
    });
}

async fn tick(
    board: &Arc<RwLock<Board>>,
    db: &Db,
    connect: &str,
    password: &str,
) -> Result<(), String> {
    // Snapshot season + per-slot (ap_slot, name, game, sent, received) under one
    // short lock; never hold it across a subprocess.
    let (season, slots) = {
        let b = board.read().map_err(|_| "board lock poisoned")?;
        let season = match &b.seed {
            Some(s) if !s.is_empty() => s.clone(),
            _ => return Ok(()), // no season yet
        };
        let slots: Vec<(i64, String, String, i64, i64)> = b
            .slots
            .iter()
            .map(|(k, s)| (*k, s.name.clone(), s.game.clone(), s.sent as i64, s.received as i64))
            .collect();
        (season, slots)
    };

    for (ap_slot, name, game, sent, received) in slots {
        if name.is_empty() {
            continue; // can't connect a tracker without the slot's name
        }
        // Skip slots whose state matches the cached result (nothing to recompute).
        if db.inlogic_state(&season, ap_slot).await == Some((sent, received)) {
            continue;
        }
        match run_one(ap_slot, &name, connect, password).await {
            Ok((ut_game, locs, true)) => {
                let g = if ut_game.is_empty() { game } else { ut_game };
                let json = serde_json::to_string(&locs).unwrap_or_else(|_| "[]".into());
                db.inlogic_set(
                    &season,
                    ap_slot,
                    now(),
                    &g,
                    sent,
                    received,
                    locs.len() as i64,
                    &json,
                    false,
                    None,
                )
                .await;
            }
            // Failure: ran-but-captured-nothing (Ok(..,false)), timed out, or
            // crashed. Record the CURRENT state with an error so we don't re-run
            // (and jam the serialized loop) until the slot's state actually moves
            // — important for a world like Starcraft 2 whose logic is too large
            // to compute in time. Preserve any last-known good list so the page
            // keeps showing it rather than going blank.
            other => {
                let msg = match other {
                    Err(e) => e,
                    _ => "could not compute (world too large or unsupported)".to_string(),
                };
                tracing::warn!("inlogic {name}: {msg}");
                let (count, locs_json) = match db.inlogic_get(&season, ap_slot).await {
                    Some(p) if p.err.is_none() => {
                        let n = serde_json::from_str::<Vec<String>>(&p.locations)
                            .map(|v| v.len())
                            .unwrap_or(0);
                        (n as i64, p.locations)
                    }
                    _ => (0, "[]".to_string()),
                };
                db.inlogic_set(
                    &season,
                    ap_slot,
                    now(),
                    &game,
                    sent,
                    received,
                    count,
                    &locs_json,
                    false,
                    Some(&msg),
                )
                .await;
            }
        }
    }
    Ok(())
}

/// Run UT for one slot and parse the JSON the helper writes. Returns
/// `(game, in_logic_names, captured)`.
async fn run_one(
    ap_slot: i64,
    name: &str,
    connect: &str,
    password: &str,
) -> Result<(String, Vec<String>, bool), String> {
    let out = format!("/tmp/bifrost_inlogic_{ap_slot}.json");
    let _ = tokio::fs::remove_file(&out).await;

    let mut child = Command::new(AP_PYTHON)
        .arg(SCRIPT)
        .arg(name)
        .arg(password)
        .arg(connect)
        .arg(&out)
        .current_dir(AP_DIR)
        .env("HOME", "/tmp")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;

    match tokio::time::timeout(RUN_TIMEOUT, child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(format!("wait: {e}")),
        Err(_) => {
            let _ = child.start_kill();
            return Err("timed out".into());
        }
    }

    let data = tokio::fs::read_to_string(&out)
        .await
        .map_err(|e| format!("no result file: {e}"))?;
    let _ = tokio::fs::remove_file(&out).await;

    let v: serde_json::Value =
        serde_json::from_str(&data).map_err(|e| format!("bad json: {e}"))?;
    if let Some(err) = v.get("error").and_then(|x| x.as_str()) {
        return Err(err.to_string());
    }
    let game = v
        .get("game")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let captured = v.get("captured").and_then(|x| x.as_bool()).unwrap_or(false);
    let locs = v
        .get("in_logic")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok((game, locs, captured))
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
