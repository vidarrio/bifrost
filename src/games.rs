//! The games each Archipelago install provides, for the Games page: what
//! players can sign up with, without having to start a signup to find out.
//!
//! Listing them loads every world, so it runs once per install (installs never
//! change), in the background, sandboxed like the other helpers, and the
//! result is kept in `$BIFROST_DATA/games/<version>.json`. A page request only
//! ever reads that file; when it's missing, the page says the list is being
//! gathered and the listing starts.
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::ap_adapter::{self, GameEntry};
use crate::apinstall::Install;
use crate::config;

/// Versions being listed right now (one run each at a time).
static RUNNING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
/// When a version's last listing failed: page views don't retry it sooner
/// than [`RETRY`] (anyone can open the page).
static FAILED: Mutex<BTreeMap<String, Instant>> = Mutex::new(BTreeMap::new());
const RETRY: Duration = Duration::from_secs(600);

fn path(version: &str) -> PathBuf {
    config::get()
        .data_dir
        .join("games")
        .join(format!("{version}.json"))
}

/// The install's games, if they've been listed.
pub fn cached(ap: &Install) -> Option<Vec<GameEntry>> {
    let bytes = std::fs::read(path(&ap.version)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Forget a version's list, when that version is installed again or removed:
/// the next install under the same name may not be the same.
pub async fn forget(version: &str) {
    let _ = tokio::fs::remove_file(path(version)).await;
    FAILED.lock().unwrap().remove(version);
}

/// List the install's games in the background, unless they're listed already,
/// a listing is running, or one failed less than [`RETRY`] ago.
pub fn ensure(ap: Install) {
    if cached(&ap).is_some() {
        return;
    }
    let failed_lately = FAILED
        .lock()
        .unwrap()
        .get(&ap.version)
        .is_some_and(|t| t.elapsed() < RETRY);
    if failed_lately || !RUNNING.lock().unwrap().insert(ap.version.clone()) {
        return;
    }
    tokio::spawn(async move {
        // It loads every world, like a YAML check: queue behind those.
        let listed = match crate::checker::check_gate().acquire().await {
            Ok(_permit) => ap_adapter::list_games(&ap).await,
            Err(e) => Err(e.to_string()),
        };
        let listed = listed.and_then(|g| {
            if g.is_empty() {
                Err("it listed no games".to_string())
            } else {
                Ok(g)
            }
        });
        match listed {
            Ok(games) => {
                let file = path(&ap.version);
                let written = match file.parent() {
                    Some(dir) => tokio::fs::create_dir_all(dir).await,
                    None => Ok(()),
                };
                let json = serde_json::to_vec(&games).expect("games serialize");
                if let Err(e) = written.and(crate::atomic::write_async(file, json).await) {
                    tracing::warn!("couldn't keep Archipelago {}'s games list: {e}", ap.version);
                }
            }
            Err(e) => {
                tracing::warn!("couldn't list Archipelago {}'s games: {e}", ap.version);
                FAILED
                    .lock()
                    .unwrap()
                    .insert(ap.version.clone(), Instant::now());
            }
        }
        RUNNING.lock().unwrap().remove(&ap.version);
    });
}
