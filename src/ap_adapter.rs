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
use crate::apinstall::{self, Install};
use crate::config::{self, SeasonDirs};
use crate::meta::{Meta, SlotCounts};
use crate::sandbox::{self, Job};

const CHECK_TIMEOUT: Duration = Duration::from_secs(180);
const META_TIMEOUT: Duration = Duration::from_secs(300);
const COUNTS_TIMEOUT: Duration = Duration::from_secs(120);
/// Default wall-clock cap on a generation (`BIFROST_GENERATE_TIMEOUT_MIN`).
const GENERATE_TIMEOUT: Duration = Duration::from_secs(120 * 60);
const SETTINGS_TIMEOUT: Duration = Duration::from_secs(60);
const PNG_TIMEOUT: Duration = Duration::from_secs(120);

/// What a helper may touch besides the AP install, scripts and season dir
/// (all read-only).
#[derive(Default)]
struct Access {
    write: Vec<PathBuf>,
    read: Vec<PathBuf>,
    network: bool,
    env: Vec<(&'static str, OsString)>,
    /// Generation: its own, larger resource ceilings.
    generate: bool,
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
    let ap = apinstall::for_dirs(season);
    ensure_host_yaml(season, &ap).await?;
    let mut all_args = vec![cfg.script(script).into_os_string()];
    all_args.extend(args);
    let mut read = vec![
        ap.dir.clone(),
        cfg.runtime_dir.join("scripts"),
        season.root.clone(),
    ];
    read.extend(access.read);
    let job = Job {
        program: &ap.python,
        args: all_args,
        cwd: &season.root,
        env: helper_env(season, &ap).into_iter().chain(access.env).collect(),
        read,
        write: access.write,
        network: access.network,
    };
    let cmd = if access.generate { sandbox::get().generation(&job) } else { sandbox::get().command(&job) };
    sandbox::run(cmd, timeout).await
}

/// The environment every AP process gets (see also `apserver`). Bifrost owns
/// the install's dependencies, so AP's own pip-on-startup is off.
pub fn ap_env(season: &SeasonDirs, ap: &Install) -> Vec<(&'static str, OsString)> {
    vec![
        ("BIFROST_AP_DIR", ap.dir.clone().into_os_string()),
        ("BIFROST_SEASON_DIR", season.root.clone().into_os_string()),
        ("PYTHONDONTWRITEBYTECODE", "1".into()),
        ("SKIP_REQUIREMENTS_UPDATE", "1".into()),
    ]
}

fn helper_env(season: &SeasonDirs, ap: &Install) -> Vec<(&'static str, OsString)> {
    ap_env(season, ap)
}

/// AP writes a default `host.yaml` when it finds none, which a read-only
/// season dir would refuse. Create it up front: a copy of the operator's
/// instance-wide `$BIFROST_DATA/host.yaml` when there is one (hint cost,
/// release/collect modes… baked into each seed at generation), else AP's
/// defaults, from a step that imports only AP's settings module (no worlds,
/// so no world code runs with write access).
async fn ensure_host_yaml(season: &SeasonDirs, ap: &Install) -> Result<(), String> {
    let dest = season.root.join("host.yaml");
    if dest.exists() {
        return Ok(());
    }
    let cfg = config::get();
    let template = cfg.data_dir.join("host.yaml");
    if template.is_file() {
        return tokio::fs::copy(&template, &dest)
            .await
            .map(|_| ())
            .map_err(|e| format!("copying {} into the season: {e}", template.display()));
    }
    const INIT: &str = "import bifrost_ap, settings\nsettings.get_settings()\n";
    let job = Job {
        program: &ap.python,
        // -P: don't put the cwd (the season dir) on sys.path.
        args: vec!["-P".into(), "-c".into(), INIT.into()],
        cwd: &season.root,
        env: {
            let mut e = helper_env(season, ap);
            e.push((
                "PYTHONPATH",
                cfg.runtime_dir.join("scripts").into_os_string(),
            ));
            e
        },
        read: vec![ap.dir.clone(), cfg.runtime_dir.join("scripts")],
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

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A custom apworld's pin: its content hash when it was accepted, who
/// uploaded it, and how it got in.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Pin {
    pub sha256: String,
    pub uploaded_by: String,
    pub uploaded_at: i64,
    /// Who approved it; empty when the instance auto-approves uploads.
    #[serde(default)]
    pub approved_by: String,
    /// Installed from the curated catalog.
    #[serde(default)]
    pub catalog: bool,
    /// A world Bifrost ships (Universal Tracker, the common game), as staged
    /// into this season — trusted even after an upgrade changes the shipped copy.
    #[serde(default)]
    pub shipped: bool,
}

impl Pin {
    /// "from the catalog", "approved by X", or "auto-approved".
    pub fn provenance(&self) -> String {
        if self.shipped {
            "shipped with Bifrost".into()
        } else if self.catalog {
            "from the catalog".into()
        } else if self.approved_by.is_empty() {
            "auto-approved".into()
        } else {
            format!("approved by {}", self.approved_by)
        }
    }
}

/// The season's pins (`worlds.lock.json`), by file name. Empty if none.
pub fn world_pins(season: &SeasonDirs) -> std::collections::BTreeMap<String, Pin> {
    std::fs::read(season.worlds_lock())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Serialises every read-modify-write of a season's pins (concurrent
/// uploads, deletes).
static PINS: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

/// Install an apworld and pin its hash (`pin.sha256` is filled in here). The
/// only way custom worlds should enter a season — see [`crate::worlds`].
pub fn install_world(season: &SeasonDirs, file: &str, bytes: &[u8], mut pin: Pin) -> Result<(), String> {
    let _lock = PINS.lock().unwrap_or_else(|p| p.into_inner());
    std::fs::write(season.worlds.join(file), bytes).map_err(|e| format!("install: {e}"))?;
    pin.sha256 = sha256_hex(bytes);
    let mut pins = world_pins(season);
    pins.insert(file.to_string(), pin);
    write_pins(season, &pins)
}

/// Pin a shipped world just staged into the season as `file`.
pub fn pin_shipped(season: &SeasonDirs, file: &str) -> Result<(), String> {
    let _lock = PINS.lock().unwrap_or_else(|p| p.into_inner());
    let bytes = std::fs::read(season.worlds.join(file)).map_err(|e| format!("{file}: {e}"))?;
    let mut pins = world_pins(season);
    pins.insert(
        file.to_string(),
        Pin { sha256: sha256_hex(&bytes), uploaded_by: "bifrost".into(), shipped: true, ..Pin::default() },
    );
    write_pins(season, &pins)
}

/// Forget a removed world's pin (`None` = all of them).
pub fn unpin_world(season: &SeasonDirs, file: Option<&str>) -> Result<(), String> {
    let _lock = PINS.lock().unwrap_or_else(|p| p.into_inner());
    let mut pins = world_pins(season);
    match file {
        Some(f) => {
            pins.remove(f);
        }
        None => pins.clear(),
    }
    write_pins(season, &pins)
}

/// File names in the season's `worlds/` that aren't Bifrost-shipped worlds:
/// the current shipped copies, or ones pinned as shipped when staged.
pub fn custom_worlds(season: &SeasonDirs) -> Vec<String> {
    let trusted = trusted_world_hashes();
    let pins = world_pins(season);
    let mut out: Vec<String> = std::fs::read_dir(&season.worlds)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("apworld"))
        .filter(|e| {
            let shipped = pins.get(&*e.file_name().to_string_lossy()).filter(|p| p.shipped);
            std::fs::read(e.path())
                .map(|b| {
                    let h = sha256_hex(&b);
                    !trusted.contains(&h) && shipped.is_none_or(|p| p.sha256 != h)
                })
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
        generate: true,
        ..Default::default()
    };
    let timeout = config::var("BIFROST_GENERATE_TIMEOUT_MIN")
        .and_then(|m| m.parse::<u64>().ok())
        .map_or(GENERATE_TIMEOUT, |m| Duration::from_secs(m * 60));
    let out = run_helper(
        "bifrost_generate.py",
        vec![],
        season,
        access,
        timeout,
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
        env: crate::ap::quiet_key().map(|k| ("BIFROST_QUIET_KEY", k.into())).into_iter().collect(),
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
        // -I and cwd /: with `-c` Python would put the cwd — the uploaded pack
        // — first on sys.path, so a pack shipping `PIL/` would run its code.
        args: vec!["-I".into(), "-c".into(), SCRIPT.into(), dir.into()],
        cwd: Path::new("/"),
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
