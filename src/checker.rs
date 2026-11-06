//! YAML checker — validates an Archipelago options file against the installed
//! worlds plus the season's custom worlds, via the AP adapter (AP's own
//! `roll_settings`, the same logic the official checker uses). Persists nothing.

use serde::Deserialize;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;

use crate::{ap_adapter, config};

/// Each check spawns a Python that loads every world (~300 MB). This box also
/// runs the game, so we serialize checks — a burst of requests queues instead of
/// storm-spawning and OOM-killing the MultiServer.
fn check_gate() -> &'static Semaphore {
    static SEM: OnceLock<Semaphore> = OnceLock::new();
    SEM.get_or_init(|| Semaphore::new(1))
}

/// Checks queued behind the gate (plus the running one). Beyond
/// [`MAX_QUEUED`] new checks are refused rather than piling up a backlog.
static QUEUED: AtomicUsize = AtomicUsize::new(0);
const MAX_QUEUED: usize = 8;

/// Decrements [`QUEUED`] however the check ends.
struct QueueSlot;
impl Drop for QueueSlot {
    fn drop(&mut self) {
        QUEUED.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Guard against someone uploading something enormous.
pub const MAX_YAML: usize = 512 * 1024;

#[derive(Debug, Deserialize)]
pub struct CheckResult {
    pub ok: bool,
    #[serde(default)]
    pub players: Vec<PlayerResult>,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PlayerResult {
    pub name: String,
    /// The YAML's name is one piece of text, rather than missing, a number or a choice.
    #[serde(default)]
    pub plain_name: bool,
    #[serde(default)]
    pub game: Option<String>,
}

/// Validate `yaml` (raw file contents) against the season's worlds.
pub async fn check_yaml(season: &config::SeasonDirs, yaml: &str) -> Result<CheckResult, String> {
    if yaml.len() > MAX_YAML {
        return Err(format!(
            "file too large ({} bytes; max {MAX_YAML})",
            yaml.len()
        ));
    }
    if too_nested(yaml) {
        return Err("that YAML is nested too deeply".into());
    }
    if QUEUED.fetch_add(1, Ordering::SeqCst) >= MAX_QUEUED {
        QUEUED.fetch_sub(1, Ordering::SeqCst);
        return Err("the checker is busy — try again in a minute".into());
    }
    let _slot = QueueSlot;
    // Serialize checks (see check_gate). Held for the whole subprocess run.
    let _permit = check_gate()
        .acquire()
        .await
        .map_err(|e| format!("checker busy: {e}"))?;
    ap_adapter::check_yaml(season, yaml).await
}

/// Brackets nested deeper than any real options file, which YAML's parser shouldn't
/// be asked to read.
const MAX_NESTING: usize = 256;

fn too_nested(yaml: &str) -> bool {
    let mut depth = 0usize;
    for b in yaml.bytes() {
        match b {
            b'[' | b'{' => {
                depth += 1;
                if depth > MAX_NESTING {
                    return true;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

/// A first look at a YAML whose world isn't approved, in turn with the checks.
pub async fn first_look(
    season: &config::SeasonDirs,
    yaml: &str,
) -> Result<ap_adapter::FirstLook, String> {
    if yaml.len() > MAX_YAML {
        return Err(format!(
            "file too large ({} bytes; max {MAX_YAML})",
            yaml.len()
        ));
    }
    if too_nested(yaml) {
        return Err("that YAML is nested too deeply".into());
    }
    if QUEUED.fetch_add(1, Ordering::SeqCst) >= MAX_QUEUED {
        QUEUED.fetch_sub(1, Ordering::SeqCst);
        return Err("the checker is busy — try again in a minute".into());
    }
    let _slot = QueueSlot;
    let _permit = check_gate()
        .acquire()
        .await
        .map_err(|e| format!("checker busy: {e}"))?;
    ap_adapter::first_look(season, yaml).await
}

#[cfg(test)]
mod nesting_tests {
    use super::*;

    #[test]
    fn only_absurd_nesting_is_refused() {
        assert!(!too_nested("a: [1, {b: [2]}]\nc: ]]]]\n"));
        assert!(too_nested(&"[".repeat(MAX_NESTING + 1)));
    }
}
