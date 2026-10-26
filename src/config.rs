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
//!
//! **Settings** — every `BIFROST_*` value, secrets included — come from the
//! environment or from `$BIFROST_DATA/config.toml` (environment wins), read
//! through [`var`]. The file keeps secrets out of service unit files and
//! process environments; `bifrost config example` prints a template.
//! `BIFROST_DATA` itself is environment-only (it locates the file).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::net::Cidr;

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
    /// Reverse proxies in front of Bifrost (`BIFROST_TRUSTED_PROXIES`, IPs or
    /// CIDRs): their `X-Forwarded-For` is believed for client addresses, and in
    /// forward-auth mode they're the only source of identity headers.
    pub trusted_proxies: Vec<Cidr>,
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
    /// Resolve from the environment and `$BIFROST_DATA/config.toml`, and
    /// make the file's settings available to [`var`]. Panics on a malformed
    /// file (startup should fail loudly, not run half-configured).
    pub fn load() -> Self {
        let data_dir = env_var("BIFROST_DATA").unwrap_or_else(|| "/var/lib/bifrost".into());
        let file = load_file(&Path::new(&data_dir).join(CONFIG_FILE))
            .unwrap_or_else(|e| panic!("{CONFIG_FILE}: {e}"));
        if FILE_VARS.set(file).is_err() {
            panic!("config::load called twice");
        }
        Self::resolve(var)
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
            trusted_proxies: var("BIFROST_TRUSTED_PROXIES")
                .map(|l| {
                    Cidr::parse_list(&l).unwrap_or_else(|e| panic!("BIFROST_TRUSTED_PROXIES: {e}"))
                })
                .unwrap_or_default(),
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

pub const CONFIG_FILE: &str = "config.toml";

/// Settings from the config file, keyed by their `BIFROST_*` name.
static FILE_VARS: OnceLock<HashMap<String, String>> = OnceLock::new();

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// A `BIFROST_*` setting: the environment, else the config file. Before
/// [`Config::load`] (and in tests) only the environment is consulted.
pub fn var(name: &str) -> Option<String> {
    env_var(name).or_else(|| FILE_VARS.get()?.get(name).cloned())
}

/// `[section] key` in config.toml ↔ the `BIFROST_*` setting it provides.
const FILE_KEYS: &[(&str, &str, &str)] = &[
    ("server", "port", "BIFROST_PORT"),
    ("server", "public_url", "BIFROST_PUBLIC_URL"),
    ("server", "trusted_proxies", "BIFROST_TRUSTED_PROXIES"),
    ("server", "insecure_cookies", "BIFROST_INSECURE_COOKIES"),
    ("archipelago", "dir", "BIFROST_AP_DIR"),
    ("archipelago", "python", "BIFROST_AP_PYTHON"),
    ("archipelago", "journal_unit", "BIFROST_AP_JOURNAL_UNIT"),
    ("archipelago", "url", "BIFROST_AP_URL"),
    ("archipelago", "slot", "BIFROST_SLOT"),
    ("archipelago", "password", "BIFROST_AP_PASSWORD"),
    ("archipelago", "admin_password", "BIFROST_AP_ADMIN_PASSWORD"),
    ("runtime", "dir", "BIFROST_RUNTIME_DIR"),
    ("sandbox", "mode", "BIFROST_SANDBOX"),
    ("sandbox", "bwrap", "BIFROST_BWRAP"),
    ("sandbox", "mem_mb", "BIFROST_SANDBOX_MEM_MB"),
    ("sandbox", "cpu_secs", "BIFROST_SANDBOX_CPU_SECS"),
    ("sandbox", "file_mb", "BIFROST_SANDBOX_FILE_MB"),
    (
        "sandbox",
        "allow_unsandboxed_custom_worlds",
        "BIFROST_ALLOW_UNSANDBOXED_CUSTOM_WORLDS",
    ),
    ("forward_auth", "secret_header", "BIFROST_FWD_SECRET_HEADER"),
    ("forward_auth", "secret", "BIFROST_FWD_SECRET"),
    (
        "forward_auth",
        "subject_header",
        "BIFROST_FWD_SUBJECT_HEADER",
    ),
    (
        "forward_auth",
        "username_header",
        "BIFROST_FWD_USERNAME_HEADER",
    ),
    ("forward_auth", "groups_header", "BIFROST_FWD_GROUPS_HEADER"),
    ("forward_auth", "role_groups", "BIFROST_FWD_ROLE_GROUPS"),
    ("forward_auth", "sign_in_url", "BIFROST_FWD_SIGN_IN_URL"),
    ("forward_auth", "sign_out_url", "BIFROST_FWD_SIGN_OUT_URL"),
    ("minecraft", "address", "BIFROST_MC_ADDR"),
    ("season", "coop", "BIFROST_COOP"),
];

/// Read the config file (absent = empty). Warns when it's readable by others,
/// since it may hold secrets.
fn load_file(path: &Path) -> Result<HashMap<String, String>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => return Err(e.to_string()),
    };
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(m) = std::fs::metadata(path)
            && m.permissions().mode() & 0o077 != 0
        {
            tracing::warn!(
                "{} is readable by group/others — it may hold secrets; chmod 600 it",
                path.display()
            );
        }
    }
    parse_file(&text)
}

/// Map config.toml onto `BIFROST_*` names. Unknown keys are errors (a typo'd
/// secret should fail loudly, not silently fall back to a default).
fn parse_file(text: &str) -> Result<HashMap<String, String>, String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let mut out = HashMap::new();
    for (section, value) in &table {
        let toml::Value::Table(keys) = value else {
            return Err(format!("top-level `{section}` must be a [section]"));
        };
        for (key, v) in keys {
            let name = FILE_KEYS
                .iter()
                .find(|(s, k, _)| s == section && k == key)
                .map(|(_, _, n)| *n)
                .ok_or_else(|| format!("unknown setting [{section}] {key}"))?;
            let v = match v {
                toml::Value::String(s) => s.clone(),
                toml::Value::Integer(i) => i.to_string(),
                toml::Value::Boolean(b) => if *b { "1" } else { "0" }.to_string(),
                toml::Value::Array(items) => items
                    .iter()
                    .map(|i| match i {
                        toml::Value::String(s) => Ok(s.clone()),
                        other => Err(format!(
                            "[{section}] {key}: expected strings, got {}",
                            other.type_str()
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .join(","),
                other => {
                    return Err(format!(
                        "[{section}] {key}: unsupported {} value",
                        other.type_str()
                    ));
                }
            };
            out.insert(name.to_string(), v);
        }
    }
    Ok(out)
}

/// A commented config.toml template (`bifrost config example`).
pub const EXAMPLE: &str = r#"# $BIFROST_DATA/config.toml — every setting here can also come from the
# environment variable in the comment (the environment wins). Keep this file
# chmod 600: it holds secrets.

[server]
# port = 8080                        # BIFROST_PORT
# public_url = "https://ap.example"  # BIFROST_PUBLIC_URL (CLI-printed links)
# trusted_proxies = ["10.0.0.5"]     # BIFROST_TRUSTED_PROXIES (reverse proxies: X-Forwarded-For, forward-auth)
# insecure_cookies = false           # BIFROST_INSECURE_COOKIES (plain-HTTP dev only)

[archipelago]
# dir = "/opt/archipelago"           # BIFROST_AP_DIR
# python = "/opt/archipelago/venv/bin/python"  # BIFROST_AP_PYTHON
# url = "ws://127.0.0.1:38281"       # BIFROST_AP_URL
# slot = "Tracker"                   # BIFROST_SLOT (enables the live board)
# password = ""                      # BIFROST_AP_PASSWORD (secret)
# admin_password = ""                # BIFROST_AP_ADMIN_PASSWORD (secret)
# journal_unit = "archipelago"       # BIFROST_AP_JOURNAL_UNIT

[runtime]
# dir = "/opt/bifrost"               # BIFROST_RUNTIME_DIR (scripts/ + vendor/)

[sandbox]
# mode = "auto"                      # BIFROST_SANDBOX: auto | bwrap | none
# mem_mb = 4096                      # BIFROST_SANDBOX_MEM_MB
# cpu_secs = 1800                    # BIFROST_SANDBOX_CPU_SECS
# file_mb = 1024                     # BIFROST_SANDBOX_FILE_MB
# allow_unsandboxed_custom_worlds = false  # BIFROST_ALLOW_UNSANDBOXED_CUSTOM_WORLDS

[forward_auth]                       # only with `bifrost auth mode forward-auth`
# secret_header = "x-bifrost-proxy"  # BIFROST_FWD_SECRET_HEADER
# secret = ""                        # BIFROST_FWD_SECRET (secret)
# role_groups = ["sysadmin=admins@idp", "user=users@idp"]  # BIFROST_FWD_ROLE_GROUPS
# sign_in_url = "/oauth2/sign_in"    # BIFROST_FWD_SIGN_IN_URL
# sign_out_url = "/oauth2/sign_out"  # BIFROST_FWD_SIGN_OUT_URL

[minecraft]
# address = "mc.example:25565"       # BIFROST_MC_ADDR (shown on the common-game card)
"#;

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
    fn config_file_maps_to_settings() {
        let m = parse_file(
            r#"
[archipelago]
password = "s3cret"
[server]
port = 9000
insecure_cookies = true
trusted_proxies = ["10.0.0.1", "10.0.0.2"]
"#,
        )
        .unwrap();
        assert_eq!(m["BIFROST_AP_PASSWORD"], "s3cret");
        assert_eq!(m["BIFROST_PORT"], "9000");
        assert_eq!(m["BIFROST_INSECURE_COOKIES"], "1");
        assert_eq!(m["BIFROST_TRUSTED_PROXIES"], "10.0.0.1,10.0.0.2");
        assert!(
            parse_file("[archipelago]\npasswrod = \"x\"").is_err(),
            "typos are errors"
        );
        assert!(parse_file("port = 1").is_err(), "settings live in sections");
        assert!(parse_file("").unwrap().is_empty());
    }

    #[test]
    fn example_file_parses_and_covers_every_key() {
        let uncommented: String = EXAMPLE
            .lines()
            .map(|l| l.trim_start_matches("# ").trim_start_matches('#'))
            .filter(|l| l.starts_with('[') || l.contains(" = "))
            .map(|l| l.split("  #").next().unwrap().to_string() + "\n")
            .collect();
        let m = parse_file(&uncommented).unwrap();
        assert!(m.len() >= 20, "{m:?}");
    }

    #[test]
    fn bad_port_falls_back() {
        assert_eq!(with(&[("BIFROST_PORT", "nope")]).port, 8080);
    }
}
