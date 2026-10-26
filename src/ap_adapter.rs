//! The one door to Archipelago internals. Everything that needs AP's own code —
//! YAML validation, seed generation, multidata/world-registry decode, savefile
//! decode, Universal Tracker in-logic — goes through here, as a helper script
//! (`scripts/bifrost_*.py`) run under the AP interpreter. Nothing else in the
//! crate spawns AP's Python or knows the helper scripts exist, so an AP upgrade
//! (or the generation sandbox, design doc §4) touches only this module.
//!
//! Every helper runs with `BIFROST_AP_DIR` (the install) and
//! `BIFROST_SEASON_DIR` (the season's AP user dir) set and the season dir as its
//! cwd; `scripts/bifrost_ap.py` pins AP's paths from those.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use tokio::process::Command;

use crate::checker::CheckResult;
use crate::config::{self, SeasonDirs};
use crate::meta::{Meta, SlotCounts};

/// A helper-script invocation against `season`, ready to run.
fn helper(script: &str, season: &SeasonDirs) -> Command {
    let cfg = config::get();
    let mut cmd = Command::new(&cfg.ap_python);
    cmd.arg(cfg.script(script))
        .current_dir(&season.root)
        .env("BIFROST_AP_DIR", &cfg.ap_dir)
        .env("BIFROST_SEASON_DIR", &season.root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null());
    cmd
}

/// Helpers print one JSON object as their last non-empty stdout line (AP and
/// world-load chatter goes to stderr or earlier lines).
fn last_json_line<T: DeserializeOwned>(stdout: &[u8], empty: &str) -> Result<T, String> {
    let stdout = String::from_utf8_lossy(stdout);
    let line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or(empty);
    serde_json::from_str(line.trim()).map_err(|e| e.to_string())
}

/// The last few non-empty stderr lines, joined — a compact error for the UI.
fn stderr_tail(out: &Output, n: usize) -> String {
    let err = String::from_utf8_lossy(&out.stderr);
    let mut tail: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).collect();
    tail.split_off(tail.len().saturating_sub(n)).join(" | ")
}

/// A unique scratch path in the system temp dir (not state; never kept).
fn scratch_file(stem: &str, ext: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("bifrost-{stem}-{nanos}.{ext}"))
}

/// Validate a player YAML (raw contents) against the season's worlds with AP's
/// own `roll_settings`. The caller serializes runs (each loads every world).
pub async fn check_yaml(season: &SeasonDirs, yaml: &str) -> Result<CheckResult, String> {
    let path = scratch_file("check", "yaml");
    tokio::fs::write(&path, yaml)
        .await
        .map_err(|e| format!("temp write: {e}"))?;
    let result = helper("bifrost_check.py", season).arg(&path).output().await;
    let _ = tokio::fs::remove_file(&path).await;
    let out = result.map_err(|e| format!("spawn checker: {e}"))?;
    last_json_line(&out.stdout, "").map_err(|e| format!("could not read checker output: {e}"))
}

/// Decode a seed zip's multidata into the season facts the board needs.
pub async fn decode_meta(season: &SeasonDirs, seed_zip: &Path) -> Result<Meta, String> {
    let out = helper("bifrost_meta.py", season)
        .arg(seed_zip)
        .output()
        .await
        .map_err(|e| format!("spawn meta: {e}"))?;
    last_json_line(&out.stdout, "").map_err(|e| format!("parse meta: {e}"))
}

/// Every slot's authoritative sent/received state from the server's save.
/// Empty before the server has written one.
pub async fn load_counts(season: &SeasonDirs) -> Result<HashMap<i64, SlotCounts>, String> {
    let out = helper("bifrost_counts.py", season)
        .arg(season.current_save())
        .output()
        .await
        .map_err(|e| format!("spawn counts: {e}"))?;
    // Keys are stringified slot numbers; collect into an i64-keyed map.
    let raw: HashMap<String, SlotCounts> =
        last_json_line(&out.stdout, "{}").map_err(|e| format!("parse counts: {e}"))?;
    Ok(raw
        .into_iter()
        .filter_map(|(k, v)| k.parse::<i64>().ok().map(|n| (n, v)))
        .collect())
}

/// Roll the season's staged `Players/` into `output/AP_<seed>.zip`.
pub async fn generate(season: &SeasonDirs) -> Result<(), String> {
    let out = helper("bifrost_generate.py", season)
        .output()
        .await
        .map_err(|e| format!("spawn generate: {e}"))?;
    if !out.status.success() {
        return Err(format!("generation error: {}", stderr_tail(&out, 3)));
    }
    Ok(())
}

/// Universal Tracker's view of one slot: its game, the location names in logic
/// right now, and whether UT actually delivered a result.
pub struct InLogic {
    pub game: String,
    pub locations: Vec<String>,
    pub captured: bool,
}

/// Run Universal Tracker headlessly for one slot against the live room
/// (`connect` is `host:port`). Killed after `timeout`.
pub async fn inlogic(
    season: &SeasonDirs,
    slot_name: &str,
    password: &str,
    connect: &str,
    timeout: Duration,
) -> Result<InLogic, String> {
    // The helper writes its result to a file (its stdout is UT/AP noise).
    let out = scratch_file("inlogic", "json");
    let mut child = helper("bifrost_inlogic.py", season)
        .arg(slot_name)
        .arg(password)
        .arg(connect)
        .arg(&out)
        .env("HOME", std::env::temp_dir())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;

    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(format!("wait: {e}")),
        Err(_) => {
            let _ = child.start_kill();
            let _ = tokio::fs::remove_file(&out).await;
            return Err("timed out".into());
        }
    }

    let data = tokio::fs::read_to_string(&out)
        .await
        .map_err(|e| format!("no result file: {e}"))?;
    let _ = tokio::fs::remove_file(&out).await;
    parse_inlogic(&data)
}

fn parse_inlogic(data: &str) -> Result<InLogic, String> {
    let v: serde_json::Value = serde_json::from_str(data).map_err(|e| format!("bad json: {e}"))?;
    if let Some(err) = v.get("error").and_then(|x| x.as_str()) {
        return Err(err.to_string());
    }
    Ok(InLogic {
        game: v
            .get("game")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        captured: v.get("captured").and_then(|x| x.as_bool()).unwrap_or(false),
        locations: v
            .get("in_logic")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Convert indexed/palette (and grayscale) PNGs under `dir` to RGBA, using the
/// Pillow that ships in the AP interpreter. Best-effort and blocking — call from
/// `spawn_blocking`.
pub fn normalize_pngs(dir: &Path) {
    const SCRIPT: &str = r#"
import sys, os
from PIL import Image
for r, _, fs in os.walk(sys.argv[1]):
    for f in fs:
        if f.lower().endswith('.png'):
            p = os.path.join(r, f)
            try:
                im = Image.open(p)
                if im.mode in ('P', 'PA', 'L', 'LA', '1'):
                    im.convert('RGBA').save(p)
            except Exception:
                pass
"#;
    let _ = std::process::Command::new(&config::get().ap_python)
        .arg("-c")
        .arg(SCRIPT)
        .arg(dir)
        .stdin(Stdio::null())
        .output();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_is_read_from_the_last_non_empty_line() {
        let out = b"loading worlds...\n{\"a\": 1}\n\n  \n";
        let v: serde_json::Value = last_json_line(out, "").unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn empty_stdout_uses_the_fallback() {
        let v: HashMap<String, u32> = last_json_line(b"\n", "{}").unwrap();
        assert!(v.is_empty());
        assert!(last_json_line::<serde_json::Value>(b"", "").is_err());
    }

    #[test]
    fn inlogic_result_parses() {
        let r = parse_inlogic(
            r#"{"slot":"A","game":"Hollow Knight","in_logic":["X","Y"],"count":2,"captured":true}"#,
        )
        .unwrap();
        assert_eq!(r.game, "Hollow Knight");
        assert_eq!(r.locations, ["X", "Y"]);
        assert!(r.captured);
    }

    #[test]
    fn inlogic_error_is_surfaced() {
        let e = parse_inlogic(r#"{"slot":"A","in_logic":[],"error":"disabled"}"#).err();
        assert_eq!(e.as_deref(), Some("disabled"));
    }
}
