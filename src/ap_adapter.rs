//! The one door to Archipelago internals. Everything that needs AP's own code —
//! YAML validation, seed generation, multidata/world-registry decode, savefile
//! decode, Universal Tracker in-logic — goes through here, as a helper script
//! (`scripts/bifrost_*.py`) run under the AP interpreter. Nothing else in the
//! crate spawns AP's Python or knows the helper scripts exist, so an AP upgrade
//! touches only this module.
//!
//! Helpers import worlds, i.e. run apworld code, so every one is started
//! through the [`sandbox`](crate::sandbox) with the narrowest access it needs:
//! no network (except in-logic, which must reach the AP server), the AP install
//! and scripts read-only, the season dir read-only, and only its own output
//! paths writable. Each helper gets `BIFROST_AP_DIR` / `BIFROST_SEASON_DIR`
//! and runs in the season dir; `scripts/bifrost_ap.py` pins AP's paths.
//!
//! Custom (uploaded) apworlds only run when the sandbox is strong enough
//! ([`check_worlds`]); worlds shipped with Bifrost are recognised by hash.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

use crate::checker::CheckResult;
use crate::config::{self, SeasonDirs};
use crate::meta::{Meta, SlotCounts};
use crate::sandbox::{self, Job};

const CHECK_TIMEOUT: Duration = Duration::from_secs(180);
const META_TIMEOUT: Duration = Duration::from_secs(300);
const COUNTS_TIMEOUT: Duration = Duration::from_secs(120);
const GENERATE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const SETTINGS_TIMEOUT: Duration = Duration::from_secs(60);
const PNG_TIMEOUT: Duration = Duration::from_secs(120);

/// What a helper may touch besides the AP install, scripts and season dir
/// (all read-only).
#[derive(Default)]
struct Access {
    write: Vec<PathBuf>,
    read: Vec<PathBuf>,
    network: bool,
}

/// A sandboxed helper-script run against `season`.
async fn run_helper(
    script: &str,
    args: Vec<OsString>,
    season: &SeasonDirs,
    access: Access,
    timeout: Duration,
) -> Result<Output, String> {
    let cfg = config::get();
    ensure_host_yaml(season).await?;
    let mut all_args = vec![cfg.script(script).into_os_string()];
    all_args.extend(args);
    let mut read = vec![
        cfg.ap_dir.clone(),
        cfg.runtime_dir.join("scripts"),
        season.root.clone(),
    ];
    read.extend(access.read);
    let job = Job {
        program: &cfg.ap_python,
        args: all_args,
        cwd: &season.root,
        env: helper_env(season),
        read,
        write: access.write,
        network: access.network,
    };
    sandbox::run(sandbox::get().command(&job), timeout).await
}

fn helper_env(season: &SeasonDirs) -> Vec<(&'static str, OsString)> {
    vec![
        (
            "BIFROST_AP_DIR",
            config::get().ap_dir.clone().into_os_string(),
        ),
        ("BIFROST_SEASON_DIR", season.root.clone().into_os_string()),
        ("PYTHONDONTWRITEBYTECODE", "1".into()),
    ]
}

/// AP writes a default `host.yaml` when it finds none, which a read-only
/// season dir would refuse. Create it up front in a step that imports only
/// AP's settings module (no worlds, so no world code runs with write access).
async fn ensure_host_yaml(season: &SeasonDirs) -> Result<(), String> {
    if season.root.join("host.yaml").exists() {
        return Ok(());
    }
    let cfg = config::get();
    const INIT: &str = "import bifrost_ap, settings\nsettings.get_settings()\n";
    let job = Job {
        program: &cfg.ap_python,
        args: vec!["-c".into(), INIT.into()],
        cwd: &season.root,
        env: {
            let mut e = helper_env(season);
            e.push((
                "PYTHONPATH",
                cfg.runtime_dir.join("scripts").into_os_string(),
            ));
            e
        },
        read: vec![cfg.ap_dir.clone(), cfg.runtime_dir.join("scripts")],
        write: vec![season.root.clone()],
        network: false,
    };
    let out = sandbox::run(sandbox::get().command(&job), SETTINGS_TIMEOUT).await?;
    if !season.root.join("host.yaml").exists() {
        return Err(format!(
            "couldn't create the season's host.yaml: {}",
            stderr_tail(&out, 2)
        ));
    }
    Ok(())
}

/// The apworlds Bifrost itself ships (vendored or operator-supplied common
/// game), by SHA-256. Anything else in a season's `worlds/` is custom.
fn trusted_world_hashes() -> Vec<String> {
    let cfg = config::get();
    [
        cfg.vendor_file("tracker.apworld"),
        cfg.common_dir().join("minecraft.apworld"),
    ]
    .iter()
    .filter_map(|p| std::fs::read(p).ok())
    .map(|b| sha256_hex(&b))
    .collect()
}

/// The version of the Archipelago install Bifrost runs (`__version__` in its
/// `Utils.py`), read once. `None` if it can't be determined.
pub fn ap_version() -> Option<&'static str> {
    static V: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        let src = std::fs::read_to_string(config::get().ap_dir.join("Utils.py")).ok()?;
        parse_version(&src)
    })
    .as_deref()
}

fn parse_version(utils_py: &str) -> Option<String> {
    utils_py.lines().find_map(|l| {
        let v = l.strip_prefix("__version__")?.trim_start().strip_prefix('=')?.trim();
        let v = v.strip_prefix('"').and_then(|v| v.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))?;
        (!v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c)))
            .then(|| v.to_string())
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A custom apworld's pin: its content hash when it was accepted, and who
/// uploaded it.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Pin {
    pub sha256: String,
    pub uploaded_by: String,
    pub uploaded_at: i64,
}

/// The season's pins (`worlds.lock.json`), by file name. Empty if none.
pub fn world_pins(season: &SeasonDirs) -> std::collections::BTreeMap<String, Pin> {
    std::fs::read(season.worlds_lock())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_pins(
    season: &SeasonDirs,
    pins: &std::collections::BTreeMap<String, Pin>,
) -> Result<(), String> {
    let path = season.worlds_lock();
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(pins).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, json)
        .and_then(|_| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("writing {}: {e}", path.display()))
}

/// Install an uploaded apworld and pin its hash. The only way custom worlds
/// should enter a season.
pub fn install_world(
    season: &SeasonDirs,
    file: &str,
    bytes: &[u8],
    uploaded_by: &str,
) -> Result<(), String> {
    let mut pins = world_pins(season);
    std::fs::write(season.worlds.join(file), bytes).map_err(|e| format!("install: {e}"))?;
    pins.insert(
        file.to_string(),
        Pin {
            sha256: sha256_hex(bytes),
            uploaded_by: uploaded_by.to_string(),
            uploaded_at: crate::auth::now(),
        },
    );
    write_pins(season, &pins)
}

/// Forget a removed world's pin (`None` = all of them).
pub fn unpin_world(season: &SeasonDirs, file: Option<&str>) -> Result<(), String> {
    let mut pins = world_pins(season);
    match file {
        Some(f) => {
            pins.remove(f);
        }
        None => pins.clear(),
    }
    write_pins(season, &pins)
}

/// File names in the season's `worlds/` that aren't Bifrost-shipped worlds.
pub fn custom_worlds(season: &SeasonDirs) -> Vec<String> {
    let trusted = trusted_world_hashes();
    let mut out: Vec<String> = std::fs::read_dir(&season.worlds)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("apworld"))
        .filter(|e| {
            std::fs::read(e.path())
                .map(|b| !trusted.contains(&sha256_hex(&b)))
                .unwrap_or(true)
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

/// Refuse to import worlds when a custom one isn't exactly what was uploaded
/// (unpinned, or changed on disk since), or when custom ones are present and
/// the sandbox can't contain them (design doc §4's trusted/untrusted split).
pub fn check_worlds(season: &SeasonDirs) -> Result<(), String> {
    let custom = custom_worlds(season);
    let pins = world_pins(season);
    for file in &custom {
        let actual = std::fs::read(season.worlds.join(file)).map(|b| sha256_hex(&b));
        match (pins.get(file), actual) {
            (Some(pin), Ok(h)) if pin.sha256 == h => {}
            (None, _) => {
                return Err(format!(
                    "apworld {file} wasn't installed through Bifrost (no pinned hash) — \
                     delete it or upload it again"
                ));
            }
            _ => {
                return Err(format!(
                    "apworld {file} changed on disk since it was uploaded — \
                     delete it or upload it again"
                ));
            }
        }
    }
    if custom.is_empty() || sandbox::get().custom_worlds_allowed() {
        Ok(())
    } else {
        Err(format!(
            "custom apworlds ({}) can't run without a sandbox on this host — install bubblewrap, \
             use the container image, or remove them",
            custom.join(", ")
        ))
    }
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

/// A fresh private scratch dir in the system temp dir (bound writable into
/// the sandbox; removed after the run).
async fn scratch_dir(stem: &str) -> Result<PathBuf, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("bifrost-{stem}-{}-{nanos}", std::process::id()));
    tokio::fs::create_dir(&dir)
        .await
        .map_err(|e| format!("scratch dir: {e}"))?;
    Ok(dir)
}

/// Validate a player YAML (raw contents) against the season's worlds with AP's
/// own `roll_settings`. The caller serializes runs (each loads every world).
pub async fn check_yaml(season: &SeasonDirs, yaml: &str) -> Result<CheckResult, String> {
    check_worlds(season)?;
    let dir = scratch_dir("check").await?;
    let path = dir.join("player.yaml");
    let result = async {
        tokio::fs::write(&path, yaml)
            .await
            .map_err(|e| format!("temp write: {e}"))?;
        let access = Access {
            read: vec![dir.clone()],
            ..Default::default()
        };
        run_helper(
            "bifrost_check.py",
            vec![path.clone().into()],
            season,
            access,
            CHECK_TIMEOUT,
        )
        .await
        .map_err(|e| format!("checker: {e}"))
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    let out = result?;
    last_json_line(&out.stdout, "").map_err(|e| format!("could not read checker output: {e}"))
}

/// Decode a seed zip's multidata into the season facts the board needs.
pub async fn decode_meta(season: &SeasonDirs, seed_zip: &Path) -> Result<Meta, String> {
    check_worlds(season)?;
    let access = Access {
        read: vec![seed_zip.to_path_buf()],
        ..Default::default()
    };
    let out = run_helper(
        "bifrost_meta.py",
        vec![seed_zip.into()],
        season,
        access,
        META_TIMEOUT,
    )
    .await
    .map_err(|e| format!("meta: {e}"))?;
    last_json_line(&out.stdout, "").map_err(|e| format!("parse meta: {e}"))
}

/// Every slot's authoritative sent/received state from the server's save.
/// Empty before the server has written one.
pub async fn load_counts(season: &SeasonDirs) -> Result<HashMap<i64, SlotCounts>, String> {
    let save = season.current_save();
    let out = run_helper(
        "bifrost_counts.py",
        vec![save.clone().into()],
        season,
        Access::default(),
        COUNTS_TIMEOUT,
    )
    .await
    .map_err(|e| format!("counts: {e}"))?;
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
    check_worlds(season)?;
    let access = Access {
        write: vec![season.output.clone(), season.logs.clone()],
        ..Default::default()
    };
    let out = run_helper(
        "bifrost_generate.py",
        vec![],
        season,
        access,
        GENERATE_TIMEOUT,
    )
    .await
    .map_err(|e| format!("generate: {e}"))?;
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
/// (`connect` is `host:port`). Killed after `timeout`. The one helper that
/// keeps network access: it has to reach the AP server.
pub async fn inlogic(
    season: &SeasonDirs,
    slot_name: &str,
    password: &str,
    connect: &str,
    timeout: Duration,
) -> Result<InLogic, String> {
    check_worlds(season)?;
    // The helper writes its result to a file (its stdout is UT/AP noise).
    let dir = scratch_dir("inlogic").await?;
    let out = dir.join("result.json");
    let access = Access {
        write: vec![dir.clone()],
        network: true,
        ..Default::default()
    };
    let args = vec![
        slot_name.into(),
        password.into(),
        connect.into(),
        out.clone().into(),
    ];
    let run = run_helper("bifrost_inlogic.py", args, season, access, timeout).await;
    let data = tokio::fs::read_to_string(&out).await;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    run?;
    parse_inlogic(&data.map_err(|e| format!("no result file: {e}"))?)
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
/// Pillow that ships in the AP interpreter — sandboxed (untrusted images), with
/// only `dir` writable. Best-effort; call from async code.
pub async fn normalize_pngs(dir: &Path) {
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
    let cfg = config::get();
    let job = Job {
        program: &cfg.ap_python,
        args: vec!["-c".into(), SCRIPT.into(), dir.into()],
        cwd: dir,
        env: vec![("PYTHONDONTWRITEBYTECODE", "1".into())],
        read: vec![],
        write: vec![dir.to_path_buf()],
        network: false,
    };
    if let Err(e) = sandbox::run(sandbox::get().command(&job), PNG_TIMEOUT).await {
        tracing::warn!("normalizing pack PNGs: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ap_version_is_read_from_utils() {
        let src = "import os\n__version__ = \"0.6.9\"\nversion_tuple = tuplize_version(__version__)\n";
        assert_eq!(parse_version(src).as_deref(), Some("0.6.9"));
        assert_eq!(parse_version("__version__ = '0.7.0-rc1'").as_deref(), Some("0.7.0-rc1"));
        assert_eq!(parse_version("__version__ = get()"), None);
        assert_eq!(parse_version("x = 1"), None);
    }

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
