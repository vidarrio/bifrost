//! Instance configuration and on-disk layout, resolved once from the
//! environment at startup (design doc §3a).
//!
//! Two roots, never mixed:
//!
//! - **Runtime** (read-only, replaced on upgrade): the AP install
//!   (`BIFROST_AP_DIR`), its Python (`BIFROST_AP_PYTHON`), and Bifrost's own
//!   shipped files (`BIFROST_RUNTIME_DIR`: `scripts/` helper scripts +
//!   `vendor/` bundled apworlds — the repo's own `scripts/` and `vendor/`).
//! - **State** (`BIFROST_DATA`): everything mutable — the one directory an
//!   operator backs up or mounts as a volume.
//!
//! ```text
//! $BIFROST_DATA/
//!   bifrost.db
//!   packs/<season>/<ap_slot>/     poptracker packs
//!   uploads/                      submitted player YAMLs
//!   common/                       operator-supplied shared-game assets
//!   archives/<seed>.tar.gz        raw season bundles
//!   mc-whitelist.json
//!   connect_password.request
//!   seasons/<id>/                 an AP "user dir" per season (see SeasonDirs)
//! ```
//!
//! Every path in the app comes from here; nothing else hardcodes a location.

use std::path::PathBuf;
use std::sync::OnceLock;

pub struct Config {
    /// HTTP listen port (`BIFROST_PORT`).
    pub port: u16,
    /// Root of all mutable state (`BIFROST_DATA`).
    pub data_dir: PathBuf,
    /// The Archipelago install, read-only (`BIFROST_AP_DIR`).
    pub ap_dir: PathBuf,
    /// The interpreter that runs AP and the helper scripts (`BIFROST_AP_PYTHON`).
    pub ap_python: PathBuf,
    /// Bifrost's shipped runtime files (`BIFROST_RUNTIME_DIR`).
    pub runtime_dir: PathBuf,
    /// systemd unit whose journal carries the AP server's log
    /// (`BIFROST_AP_JOURNAL_UNIT`). Goes away once Bifrost supervises AP itself.
    pub ap_journal_unit: String,
}

/// The directories of one season. Laid out exactly like an Archipelago *user
/// dir* (AP's `user_path`), so AP — and Universal Tracker — resolve the
/// season's YAMLs, custom worlds, output, logs and `host.yaml` with their own
/// defaults once the helper scripts point `user_path` here.
pub struct SeasonDirs {
    pub root: PathBuf,
    /// Staged per-player YAMLs (AP default `Players`).
    pub players: PathBuf,
    /// Custom `.apworld`s. AP scans `user_path("worlds")` whenever the user dir
    /// differs from the install dir.
    pub worlds: PathBuf,
    /// Rolled seeds, the deployed `current.zip` and the server's save.
    pub output: PathBuf,
    pub logs: PathBuf,
}

impl SeasonDirs {
    fn new(root: PathBuf) -> Self {
        Self {
            players: root.join("Players"),
            worlds: root.join("worlds"),
            output: root.join("output"),
            logs: root.join("logs"),
            root,
        }
    }

    /// The deployed seed the AP server runs.
    pub fn current_zip(&self) -> PathBuf {
        self.output.join("current.zip")
    }

    /// The AP server's save for `current.zip` (AP names it after the multidata).
    pub fn current_save(&self) -> PathBuf {
        self.output.join("current.apsave")
    }
}

/// The runtime still runs a single season; its state lives under this id until
/// seasons become first-class (design doc §7).
const CURRENT_SEASON: &str = "current";

impl Config {
    /// Resolve from the process environment.
    pub fn from_env() -> Self {
        Self::resolve(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
    }

    /// Resolve from an arbitrary variable lookup (testable).
    fn resolve(var: impl Fn(&str) -> Option<String>) -> Self {
        let ap_dir =
            PathBuf::from(var("BIFROST_AP_DIR").unwrap_or_else(|| "/opt/archipelago".into()));
        Self {
            port: var("BIFROST_PORT")
                .and_then(|s| s.parse().ok())
                .unwrap_or(8080),
            data_dir: var("BIFROST_DATA")
                .unwrap_or_else(|| "/var/lib/bifrost".into())
                .into(),
            ap_python: var("BIFROST_AP_PYTHON")
                .map(PathBuf::from)
                .unwrap_or_else(|| ap_dir.join("venv/bin/python")),
            ap_dir,
            runtime_dir: var("BIFROST_RUNTIME_DIR")
                .unwrap_or_else(|| "/opt/bifrost".into())
                .into(),
            ap_journal_unit: var("BIFROST_AP_JOURNAL_UNIT").unwrap_or_else(|| "archipelago".into()),
        }
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("bifrost.db")
    }
    pub fn packs_dir(&self) -> PathBuf {
        self.data_dir.join("packs")
    }
    pub fn uploads_dir(&self) -> PathBuf {
        self.data_dir.join("uploads")
    }
    /// Operator-supplied assets for the shared Minecraft game (apworld, YAML,
    /// and client jars under `client/`).
    pub fn common_dir(&self) -> PathBuf {
        self.data_dir.join("common")
    }
    pub fn archives_dir(&self) -> PathBuf {
        self.data_dir.join("archives")
    }
    pub fn mc_whitelist_file(&self) -> PathBuf {
        self.data_dir.join("mc-whitelist.json")
    }
    /// Bifrost writes a requested connect password here for the host to apply.
    pub fn connpw_request_file(&self) -> PathBuf {
        self.data_dir.join("connect_password.request")
    }

    /// The season the runtime is operating on.
    pub fn season(&self) -> SeasonDirs {
        SeasonDirs::new(self.data_dir.join("seasons").join(CURRENT_SEASON))
    }

    /// A helper script shipped in the runtime.
    pub fn script(&self, name: &str) -> PathBuf {
        self.runtime_dir.join("scripts").join(name)
    }
    /// A bundled apworld shipped in the runtime (e.g. Universal Tracker).
    pub fn vendor_file(&self, name: &str) -> PathBuf {
        self.runtime_dir.join("vendor").join(name)
    }

    /// Create the state directories the app writes into (idempotent).
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        let s = self.season();
        for d in [
            self.data_dir.clone(),
            self.packs_dir(),
            self.uploads_dir(),
            self.archives_dir(),
            s.players,
            s.worlds,
            s.output,
            s.logs,
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

static CONFIG: OnceLock<Config> = OnceLock::new();

/// Install the config; call once at startup before anything reads it.
pub fn init(cfg: Config) -> &'static Config {
    if CONFIG.set(cfg).is_err() {
        panic!("config::init called twice");
    }
    get()
}

/// The process config. Panics if read before [`init`].
pub fn get() -> &'static Config {
    CONFIG.get().expect("config not initialised")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::Path;

    fn with(vars: &[(&str, &str)]) -> Config {
        let m: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::resolve(|k| m.get(k).cloned())
    }

    #[test]
    fn defaults() {
        let c = with(&[]);
        assert_eq!(c.port, 8080);
        assert_eq!(c.data_dir, Path::new("/var/lib/bifrost"));
        assert_eq!(c.ap_dir, Path::new("/opt/archipelago"));
        assert_eq!(c.ap_python, Path::new("/opt/archipelago/venv/bin/python"));
        assert_eq!(
            c.script("bifrost_meta.py"),
            Path::new("/opt/bifrost/scripts/bifrost_meta.py")
        );
        assert_eq!(c.ap_journal_unit, "archipelago");
    }

    #[test]
    fn all_state_lives_under_the_data_dir() {
        let c = with(&[("BIFROST_DATA", "/data")]);
        let s = c.season();
        for p in [
            c.db_path(),
            c.packs_dir(),
            c.uploads_dir(),
            c.common_dir(),
            c.archives_dir(),
            c.mc_whitelist_file(),
            c.connpw_request_file(),
            s.root.clone(),
            s.players.clone(),
            s.worlds.clone(),
            s.output.clone(),
            s.logs.clone(),
            s.current_zip(),
            s.current_save(),
        ] {
            assert!(
                p.starts_with("/data"),
                "{} escapes the data dir",
                p.display()
            );
        }
        assert_eq!(
            s.current_zip(),
            Path::new("/data/seasons/current/output/current.zip")
        );
    }

    #[test]
    fn python_follows_ap_dir_unless_overridden() {
        let c = with(&[("BIFROST_AP_DIR", "/ap")]);
        assert_eq!(c.ap_python, Path::new("/ap/venv/bin/python"));
        let c = with(&[
            ("BIFROST_AP_DIR", "/ap"),
            ("BIFROST_AP_PYTHON", "/usr/bin/python3"),
        ]);
        assert_eq!(c.ap_python, Path::new("/usr/bin/python3"));
    }

    #[test]
    fn bad_port_falls_back() {
        assert_eq!(with(&[("BIFROST_PORT", "nope")]).port, 8080);
    }
}
