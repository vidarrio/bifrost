//! The Archipelago installs Bifrost can run, and which one each season uses.
//!
//! There's always the **bundled** install (`BIFROST_AP_DIR` — baked into the
//! image or bundle, read-only). Sysadmins can install further releases from
//! the settings page; each lands in `$BIFROST_DATA/ap/<version>/` (source +
//! its own venv) and is marked ready only after it passed a test generation.
//! One install is **active**: signup checks and new generations use it. A
//! season is **pinned** to the install that generated its seed, so a live
//! season keeps running — server and helpers — on the version that rolled it
//! until it's re-rolled or archived; switching the active version never
//! changes a live game under its players.
//!
//! The state is held in memory (resolved synchronously by `ap_adapter` and the
//! AP server supervisor) and loaded from the database at startup.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::config::{self, SeasonDirs};

/// A file in a managed install's dir that marks it complete and tested.
const READY: &str = ".bifrost-ready";

#[derive(Clone, Debug, PartialEq)]
pub struct Install {
    /// Its `__version__` (managed installs are also named by it).
    pub version: String,
    pub dir: PathBuf,
    pub python: PathBuf,
    /// The install that shipped with Bifrost (can't be removed).
    pub bundled: bool,
}

struct State {
    /// The active version, when it isn't the bundled one.
    active: Option<String>,
    /// Season → the version that generated its current seed.
    pins: BTreeMap<i64, String>,
}

static STATE: RwLock<State> = RwLock::new(State { active: None, pins: BTreeMap::new() });

/// The version an AP install declares (`__version__` in its `Utils.py`).
pub fn read_version(dir: &Path) -> Option<String> {
    parse_version(&std::fs::read_to_string(dir.join("Utils.py")).ok()?)
}

fn parse_version(utils_py: &str) -> Option<String> {
    utils_py.lines().find_map(|l| {
        let v = l.strip_prefix("__version__")?.trim_start().strip_prefix('=')?.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))?;
        valid_version(v).then(|| v.to_string())
    })
}

/// A version string safe to use as a directory name and a git tag.
pub fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 32
        && !v.starts_with('.')
        && v.chars().all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c))
}

pub fn bundled() -> Install {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let cfg = config::get();
    Install {
        version: V
            .get_or_init(|| read_version(&cfg.ap_dir).unwrap_or_else(|| "unknown".into()))
            .clone(),
        dir: cfg.ap_dir.clone(),
        python: cfg.ap_python.clone(),
        bundled: true,
    }
}

/// Where managed installs live.
pub fn root() -> PathBuf {
    config::get().data_dir.join("ap")
}

fn managed(version: &str) -> Option<Install> {
    let dir = root().join(version);
    (valid_version(version) && dir.join(READY).is_file()).then(|| Install {
        version: version.to_string(),
        python: dir.join("venv/bin/python"),
        dir,
        bundled: false,
    })
}

/// Every ready install: the bundled one first, then managed ones by version.
pub fn installed() -> Vec<Install> {
    let b = bundled();
    let mut managed: Vec<Install> = std::fs::read_dir(root())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| managed(e.file_name().to_str()?))
        .filter(|i| i.version != b.version)
        .collect();
    managed.sort_by(|a, b| version_cmp(&a.version, &b.version));
    std::iter::once(b).chain(managed).collect()
}

/// An install by version (bundled or managed).
pub fn find(version: &str) -> Option<Install> {
    let b = bundled();
    if b.version == version {
        return Some(b);
    }
    managed(version)
}

/// The install signup checks and new generations use.
pub fn active() -> Install {
    let v = STATE.read().unwrap().active.clone();
    v.and_then(|v| find(&v)).unwrap_or_else(bundled)
}

/// The install a season's helpers and server run: the one its seed was
/// generated with, else the active one.
pub fn for_season(id: i64) -> Install {
    let v = STATE.read().unwrap().pins.get(&id).cloned();
    match v.map(|v| (find(&v), v)) {
        Some((Some(i), _)) => i,
        Some((None, v)) => {
            tracing::warn!("season {id} was generated with Archipelago {v}, which isn't installed any more — using the active one");
            active()
        }
        None => active(),
    }
}

/// The install to run helpers for `dirs` with.
pub fn for_dirs(dirs: &SeasonDirs) -> Install {
    if let Some(i) = &dirs.install {
        return i.clone();
    }
    dirs.id.map(for_season).unwrap_or_else(active)
}

/// Load the active version and season pins (startup).
pub fn load(active: Option<String>, pins: BTreeMap<i64, String>) {
    let mut st = STATE.write().unwrap();
    st.active = active.filter(|v| {
        let ok = find(v).is_some();
        if !ok {
            tracing::warn!("the active Archipelago {v} isn't installed — using the bundled {}", bundled().version);
        }
        ok
    });
    st.pins = pins;
}

pub fn set_active(version: Option<String>) {
    STATE.write().unwrap().active = version;
}

pub fn pin(season: i64, version: Option<String>) {
    let mut st = STATE.write().unwrap();
    match version {
        Some(v) => st.pins.insert(season, v),
        None => st.pins.remove(&season),
    };
}

/// Seasons pinned to `version`.
pub fn pinned_to(version: &str) -> Vec<i64> {
    let mut v: Vec<i64> = STATE
        .read()
        .unwrap()
        .pins
        .iter()
        .filter(|(_, p)| p.as_str() == version)
        .map(|(s, _)| *s)
        .collect();
    v.sort();
    v
}

/// Mark a managed install complete and tested.
pub fn mark_ready(dir: &Path) -> std::io::Result<()> {
    std::fs::write(dir.join(READY), b"")
}

/// Order versions numerically where they're numeric (0.6.10 > 0.6.9).
pub fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |s: &str| -> Vec<(u64, String)> {
        s.split(['.', '-', '+'])
            .map(|p| {
                let digits: String = p.chars().take_while(char::is_ascii_digit).collect();
                (digits.parse().unwrap_or(0), p[digits.len()..].to_string())
            })
            .collect()
    };
    parts(a).cmp(&parts(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_read_from_utils() {
        assert_eq!(parse_version("x = 1\n__version__ = \"0.6.8\"\n").as_deref(), Some("0.6.8"));
        assert_eq!(parse_version("__version__='0.7.0-rc1'").as_deref(), Some("0.7.0-rc1"));
        assert_eq!(parse_version("__version__ = \"../x\""), None);
        assert_eq!(parse_version("version = \"1\""), None);
    }

    #[test]
    fn versions_order_numerically() {
        use std::cmp::Ordering::*;
        assert_eq!(version_cmp("0.6.10", "0.6.9"), Greater);
        assert_eq!(version_cmp("0.6.9", "0.6.9"), Equal);
        assert_eq!(version_cmp("0.6.8", "0.7.0"), Less);
        assert!(!valid_version("../0.6") && !valid_version(".git") && valid_version("0.6.9"));
    }
}
