//! The games each Archipelago install provides, for the Games page: what
//! players can sign up with, without having to start a signup to find out.
//!
//! Listing them loads every world, so it runs once per install (installs never
//! change), in the background, sandboxed like the other helpers, and the
//! result is kept in `$BIFROST_DATA/games/<version>.json`. A page request only
//! ever reads that file; when it's missing, the page says the list is being
//! gathered and the listing starts.
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::ap_adapter::{self, GameEntry};
use crate::apinstall::Install;
use crate::config;

/// Versions being listed right now (one run each at a time).
static RUNNING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

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

/// List the install's games in the background, unless they're listed already
/// or a listing is running.
pub fn ensure(ap: Install) {
    if cached(&ap).is_some() || !RUNNING.lock().unwrap().insert(ap.version.clone()) {
        return;
    }
    tokio::spawn(async move {
        match ap_adapter::list_games(&ap).await {
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
            Err(e) => tracing::warn!("couldn't list Archipelago {}'s games: {e}", ap.version),
        }
        RUNNING.lock().unwrap().remove(&ap.version);
    });
}
