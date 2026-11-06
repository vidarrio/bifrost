//! Instance configuration and on-disk layout, resolved once from the
//! environment at startup.
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
//!   packs/<season>/<game>/        poptracker packs
//!   uploads/                      submitted player YAMLs
//!   common/<key>/                 the catalog of common games (see crate::common)
//!   archives/<seed>.tar.gz        raw season bundles
//!   connect_password.request
//!   seasons/<id>/                 an AP "user dir" per season (see SeasonDirs)
//!   seasons/_core/                empty user dir for checks before any season
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
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::net::Cidr;

pub struct Config {
    /// HTTP listen port (`BIFROST_PORT`).
    pub port: u16,
    /// HTTP listen address (`BIFROST_BIND`): every interface by default;
    /// `127.0.0.1` behind a reverse proxy on the same host.
    pub bind: std::net::IpAddr,
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
    /// Bifrost runs the live season's AP server itself (`BIFROST_AP_MODE`
    /// `managed`, the default) or connects to one run elsewhere (`external`:
    /// `BIFROST_AP_URL` + passwords, journal-sourced activity).
    pub ap_managed: bool,
    /// Where managed AP servers listen (`BIFROST_AP_BIND`).
    pub ap_bind: String,
    /// The ports live seasons' AP servers get, one each (`BIFROST_AP_PORTS`,
    /// e.g. `38281-38300`). Its size caps how many seasons can be live at once.
    pub ap_ports: RangeInclusive<u16>,
    /// The GitHub repo further Archipelago releases are installed from:
    /// Bifrost's fork (`packaging/archipelago/`) by default.
    pub ap_repo: String,
    /// The Python that builds an installed release's venv.
    pub base_python: PathBuf,
}

/// The directories of one season. Laid out exactly like an Archipelago *user
/// dir* (AP's `user_path`), so AP — and Universal Tracker — resolve the
/// season's YAMLs, custom worlds, output, logs and `host.yaml` with their own
/// defaults once the helper scripts point `user_path` here.
#[derive(Clone)]
pub struct SeasonDirs {
    pub root: PathBuf,
    /// The season these dirs belong to (`None` for scratch dirs).
    pub id: Option<crate::id::Id>,
    /// Run this AP install rather than the season's own (test runs).
    pub install: Option<crate::apinstall::Install>,
    /// Staged per-player YAMLs (AP default `Players`).
    pub players: PathBuf,
    /// Custom `.apworld`s. AP scans `user_path("worlds")` whenever the user dir
    /// differs from the install dir.
    pub worlds: PathBuf,
    /// Rolled seeds, the deployed `current.zip` and the server's save.
    pub output: PathBuf,
    pub logs: PathBuf,
    /// Players' submitted YAMLs (before generation stages them into Players/).
    pub submissions: PathBuf,
    /// Uploaded apworlds waiting for approval — outside `worlds/`, so AP
    /// never loads them.
    pub pending: PathBuf,
}

impl SeasonDirs {
    fn new(root: PathBuf, id: Option<crate::id::Id>) -> Self {
        Self {
            id,
            install: None,
            players: root.join("Players"),
            worlds: root.join("worlds"),
            output: root.join("output"),
            logs: root.join("logs"),
            submissions: root.join("submissions"),
            pending: root.join("pending"),
            root,
        }
    }

    pub fn with_install(mut self, install: crate::apinstall::Install) -> Self {
        self.install = Some(install);
        self
    }

    /// Create the season's directories (idempotent).
    pub fn ensure(&self) -> std::io::Result<()> {
        for d in [
            &self.players,
            &self.worlds,
            &self.output,
            &self.logs,
            &self.submissions,
            &self.pending,
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }

    /// The deployed seed the AP server runs.
    pub fn current_zip(&self) -> PathBuf {
        self.output.join("current.zip")
    }

    /// Pinned SHA-256 (+ uploader) of each custom apworld in `worlds/`.
    pub fn worlds_lock(&self) -> PathBuf {
        self.root.join("worlds.lock.json")
    }

    /// The AP server's save for `current.zip` (AP names it after the multidata).
    pub fn current_save(&self) -> PathBuf {
        self.output.join("current.apsave")
    }
}

impl Config {
    /// Resolve from the environment and `$BIFROST_DATA/config.toml`, and
    /// make the file's settings available to [`var`]. Panics on a malformed
    /// file (startup should fail loudly, not run half-configured).
    ///
    /// A single-file bundle (see `bundle`) unpacks its runtime here and
    /// supplies the defaults for the AP install, interpreter and runtime dir;
    /// without `BIFROST_DATA` it keeps its data in the user's data dir.
    pub fn load() -> Self {
        let data_dir = env_var("BIFROST_DATA").unwrap_or_else(|| {
            if crate::bundle::present() {
                crate::bundle::default_data_dir()
                    .to_string_lossy()
                    .into_owned()
            } else {
                "/var/lib/bifrost".into()
            }
        });
        // SAFETY: geteuid only reads this process's user id.
        let me = unsafe { libc::geteuid() };
        let data_dir = if me == 0 {
            tracing::warn!(
                "running as root (BIFROST_ALLOW_ROOT): game code runs as root, \
                 and without bubblewrap nothing contains it"
            );
            // SAFETY: umask only sets this process's file creation mask.
            unsafe { libc::umask(0o077) };
            let real = root_data_dir(Path::new(&data_dir), me)
                .unwrap_or_else(|e| panic!("not running as root: {e}"));
            real.to_str()
                .unwrap_or_else(|| panic!("not running as root: {} isn't UTF-8", real.display()))
                .to_owned()
        } else {
            data_dir
        };
        let file = load_file(&Path::new(&data_dir).join(CONFIG_FILE))
            .unwrap_or_else(|e| panic!("{CONFIG_FILE}: {e}"));
        if FILE_VARS.set(file).is_err() {
            panic!("config::load called twice");
        }
        let bundled: HashMap<&str, String> = match crate::bundle::prepare(Path::new(&data_dir)) {
            Ok(Some(rt)) => {
                let s = |p: PathBuf| p.to_string_lossy().into_owned();
                HashMap::from([
                    ("BIFROST_AP_DIR", s(rt.ap_dir())),
                    ("BIFROST_AP_PYTHON", s(rt.python())),
                    ("BIFROST_RUNTIME_DIR", s(rt.runtime_dir())),
                    ("BIFROST_PYTHON", s(rt.python())),
                ])
            }
            Ok(None) => HashMap::new(),
            Err(e) => panic!("bundled runtime: {e}"),
        };
        Self::resolve(|k| match k {
            "BIFROST_DATA" => Some(data_dir.clone()),
            _ => var(k).or_else(|| bundled.get(k).cloned()),
        })
    }

    /// Resolve from an arbitrary variable lookup (testable).
    fn resolve(var: impl Fn(&str) -> Option<String>) -> Self {
        let ap_dir =
            PathBuf::from(var("BIFROST_AP_DIR").unwrap_or_else(|| "/opt/archipelago".into()));
        Self {
            port: var("BIFROST_PORT")
                .and_then(|s| s.parse().ok())
                .unwrap_or(8080),
            bind: var("BIFROST_BIND")
                .map(|s| {
                    s.trim()
                        .parse()
                        .unwrap_or_else(|_| panic!("BIFROST_BIND: not an IP address: {s:?}"))
                })
                .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)),
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
            ap_managed: var("BIFROST_AP_MODE").as_deref() != Some("external"),
            ap_bind: {
                let bind = var("BIFROST_AP_BIND").unwrap_or_else(|| "0.0.0.0".into());
                #[cfg(feature = "demo")]
                let bind = crate::demo::ap_bind(bind);
                bind
            },
            ap_ports: var("BIFROST_AP_PORTS")
                .map(|r| parse_port_range(&r).unwrap_or_else(|e| panic!("BIFROST_AP_PORTS: {e}")))
                .unwrap_or(38281..=38300),
            ap_repo: var("BIFROST_AP_REPO").unwrap_or_else(|| "vidarrio/Archipelago".into()),
            base_python: var("BIFROST_PYTHON")
                .unwrap_or_else(|| "python3".into())
                .into(),
            trusted_proxies: var("BIFROST_TRUSTED_PROXIES")
                .map(|l| {
                    Cidr::parse_list(&l).unwrap_or_else(|e| panic!("BIFROST_TRUSTED_PROXIES: {e}"))
                })
                .unwrap_or_default(),
        }
    }

    /// Where to reach this instance's own HTTP listener from the same host:
    /// the bind address, or loopback when bound to every interface.
    pub fn local_addr(&self) -> std::net::SocketAddr {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
        let ip = match self.bind {
            IpAddr::V4(v4) if v4.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(v6) if v6.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            ip => ip,
        };
        std::net::SocketAddr::new(ip, self.port)
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
    /// A file the database names by its place in the data dir (a signup's
    /// YAML), so a data dir can be moved or restored elsewhere. `None` for a
    /// path that's absolute or would leave the data dir.
    pub fn data_file(&self, rel: &str) -> Option<PathBuf> {
        let p = Path::new(rel);
        (!rel.is_empty()
            && p.components()
                .all(|c| matches!(c, std::path::Component::Normal(_))))
        .then(|| self.data_dir.join(p))
    }

    /// How the database names a file in the data dir: its path from there.
    /// `None` for a path outside it.
    pub fn data_name(&self, path: &Path) -> Option<String> {
        let rel = path
            .strip_prefix(&self.data_dir)
            .ok()?
            .to_str()?
            .to_string();
        self.data_file(&rel).map(|_| rel)
    }

    /// The catalog of common games: one folder per setup (see [`crate::common`]).
    pub fn common_dir(&self) -> PathBuf {
        self.data_dir.join("common")
    }
    pub fn archives_dir(&self) -> PathBuf {
        self.data_dir.join("archives")
    }
    /// The curated apworld catalog, one `<sha256>.apworld` per entry.
    pub fn catalog_dir(&self) -> PathBuf {
        self.data_dir.join("catalog")
    }
    /// Bifrost writes a requested connect password here for the host to apply.
    pub fn connpw_request_file(&self) -> PathBuf {
        self.data_dir.join("connect_password.request")
    }

    /// A season's directories (`seasons/<id>/`).
    pub fn season(&self, id: crate::id::Id) -> SeasonDirs {
        SeasonDirs::new(self.data_dir.join("seasons").join(id.to_string()), Some(id))
    }

    /// An empty AP user dir with no custom worlds: what YAML checks run
    /// against before any season exists (core worlds only).
    pub fn core_season(&self) -> SeasonDirs {
        SeasonDirs::new(self.data_dir.join("seasons").join("_core"), None)
    }

    /// Scratch space (`tmp/`), cleared at startup.
    pub fn tmp_dir(&self) -> PathBuf {
        self.data_dir.join("tmp")
    }

    /// A throwaway AP user dir under `tmp/` (e.g. an install's test run).
    pub fn scratch_season(&self, name: &str) -> SeasonDirs {
        SeasonDirs::new(self.tmp_dir().join(name), None)
    }

    /// A helper script shipped in the runtime.
    pub fn script(&self, name: &str) -> PathBuf {
        self.runtime_dir.join("scripts").join(name)
    }
    /// A bundled apworld shipped in the runtime (e.g. Universal Tracker).
    pub fn vendor_file(&self, name: &str) -> PathBuf {
        self.runtime_dir.join("vendor").join(name)
    }

    /// Create the instance-level state directories (idempotent).
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for d in [
            self.data_dir.clone(),
            self.packs_dir(),
            self.uploads_dir(),
            self.archives_dir(),
            self.catalog_dir(),
        ] {
            std::fs::create_dir_all(d)?;
        }
        self.core_season().ensure()?;
        self.lock_down();
        Ok(())
    }

    /// Keep what's in the data dir to Bifrost's user: it holds password hashes,
    /// tokens, spoilers and AP passwords. Only ever tightens, and only what's Bifrost's
    /// own, by its service: never as root, as anyone able to write here could point a
    /// link anywhere, and an operator's own files stay as they set them. The dir
    /// itself can still be passed through.
    fn lock_down(&self) {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SAFETY: geteuid only reads this process's user id.
        let me = unsafe { libc::geteuid() };
        if me == 0 {
            return;
        }
        let tighten = |path: &std::path::Path, keep: u32| {
            // Not following links, and nobody else's.
            let Ok(meta) = std::fs::symlink_metadata(path) else {
                return;
            };
            let mode = meta.permissions().mode() & 0o7777;
            if meta.file_type().is_symlink() || meta.uid() != me || mode & !keep == 0 {
                return;
            }
            let tighter = std::fs::Permissions::from_mode(mode & keep);
            if let Err(e) = std::fs::set_permissions(path, tighter) {
                tracing::warn!("restricting {}: {e}", path.display());
            }
        };
        tighten(&self.data_dir, 0o711);
        let Ok(entries) = std::fs::read_dir(&self.data_dir) else {
            return;
        };
        for entry in entries.flatten() {
            tighten(&entry.path(), 0o700);
        }
    }
}

pub const CONFIG_FILE: &str = "config.toml";

/// Bifrost runs game code and keeps data others may be able to write to, so root
/// has to ask for it. Only the environment can, as the data dir isn't to be trusted
/// by root until [`root_data_dir`] has passed.
pub fn root_refused(euid: u32, allow: Option<&std::ffi::OsStr>) -> bool {
    euid == 0 && allow.is_none_or(|v| v != "1")
}

/// Nobody but `me` or root can change `path`. A sticky directory lets others add
/// entries but not replace `me`'s, which will do for the folders above the data dir.
fn check_held(
    path: &Path,
    meta: &std::fs::Metadata,
    me: u32,
    sticky_ok: bool,
) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if meta.uid() != me && meta.uid() != 0 {
        return Err(format!("{} belongs to another user", path.display()));
    }
    let mode = meta.permissions().mode();
    if !meta.is_symlink()
        && mode & 0o022 != 0
        && !(sticky_ok && meta.is_dir() && mode & 0o1000 != 0)
    {
        return Err(format!("others can write to {}", path.display()));
    }
    Ok(())
}

/// Nobody but `me` or root can change the way to `path` as written: each folder and
/// link on it, then where each link leads. A part that doesn't exist yet ends it,
/// unless all of it should exist by now.
fn check_way(path: &Path, me: u32, links_left: u32, all_there: bool) -> Result<(), String> {
    use std::path::Component;
    let show = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    let absolute = std::env::current_dir()
        .map_err(|e| format!("current directory: {e}"))?
        .join(path);
    let mut at = PathBuf::from("/");
    for part in absolute.components() {
        match part {
            Component::Normal(name) => at.push(name),
            Component::ParentDir => {
                at.pop();
                continue;
            }
            _ => continue,
        }
        let meta = match std::fs::symlink_metadata(&at) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !all_there => return Ok(()),
            Err(e) => return Err(show(&at, e)),
        };
        check_held(&at, &meta, me, true)?;
        if meta.is_symlink() {
            if links_left == 0 {
                return Err(format!("too many links at {}", at.display()));
            }
            let target = std::fs::read_link(&at).map_err(|e| show(&at, e))?;
            at.pop();
            let target = at.join(target);
            check_way(&target, me, links_left - 1, all_there)?;
            at = target.canonicalize().map_err(|e| show(&target, e))?;
        }
    }
    Ok(())
}

/// The data dir a process running as root may use, with its links resolved so
/// nobody can later repoint them: it, all it holds and the folders above it can
/// only be changed by `me` or root. Everything in it is trusted from here on (the
/// settings, the installed Archipelago, the runtime), so this covers all of it.
fn root_data_dir(data_dir: &Path, me: u32) -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let show = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    check_way(data_dir, me, 40, false)?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(data_dir)
        .map_err(|e| show(data_dir, e))?;
    // Again, all of it now: what's held and in place here can't be swapped, even in
    // a sticky folder, and something could have been put there before it was created.
    check_way(data_dir, me, 40, true)?;
    let real = data_dir.canonicalize().map_err(|e| show(data_dir, e))?;
    for (i, dir) in real.ancestors().enumerate() {
        let meta = std::fs::symlink_metadata(dir).map_err(|e| show(dir, e))?;
        check_held(dir, &meta, me, i > 0)?;
    }
    let mut todo = vec![real.clone()];
    while let Some(dir) = todo.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(show(&dir, e)),
        };
        for entry in entries {
            let path = entry.map_err(|e| show(&dir, e))?.path();
            // A running server's scratch files come and go.
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(show(&path, e)),
            };
            check_held(&path, &meta, me, false)?;
            if meta.is_dir() {
                todo.push(path);
            }
        }
    }
    Ok(real)
}

/// Settings from the config file, keyed by their `BIFROST_*` name.
static FILE_VARS: OnceLock<HashMap<String, String>> = OnceLock::new();

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// A `BIFROST_*` setting: the environment, else the config file. Before
/// [`Config::load`] (and in tests) only the environment is consulted. An
/// empty value is unset either way (e.g. the template's `secret = ""`).
pub fn var(name: &str) -> Option<String> {
    env_var(name).or_else(|| {
        FILE_VARS
            .get()?
            .get(name)
            .filter(|v| !v.trim().is_empty())
            .cloned()
    })
}

/// `38281-38300` (or a single port) → an inclusive range.
fn parse_port_range(s: &str) -> Result<RangeInclusive<u16>, String> {
    let (a, b) = s.trim().split_once('-').unwrap_or((s.trim(), s.trim()));
    let port = |p: &str| {
        p.trim()
            .parse::<u16>()
            .ok()
            .filter(|&p| p > 0)
            .ok_or_else(|| format!("bad port {p:?}"))
    };
    let (a, b) = (port(a)?, port(b)?);
    if a > b {
        return Err(format!("{a}-{b} is backwards"));
    }
    Ok(a..=b)
}

/// `[section] key` in config.toml ↔ the `BIFROST_*` setting it provides.
const FILE_KEYS: &[(&str, &str, &str)] = &[
    ("server", "port", "BIFROST_PORT"),
    ("server", "bind", "BIFROST_BIND"),
    ("server", "public_url", "BIFROST_PUBLIC_URL"),
    ("server", "trusted_proxies", "BIFROST_TRUSTED_PROXIES"),
    ("server", "insecure_cookies", "BIFROST_INSECURE_COOKIES"),
    ("archipelago", "mode", "BIFROST_AP_MODE"),
    ("archipelago", "bind", "BIFROST_AP_BIND"),
    ("archipelago", "ports", "BIFROST_AP_PORTS"),
    ("archipelago", "dir", "BIFROST_AP_DIR"),
    ("archipelago", "python", "BIFROST_AP_PYTHON"),
    ("archipelago", "repo", "BIFROST_AP_REPO"),
    ("archipelago", "base_python", "BIFROST_PYTHON"),
    ("archipelago", "journal_unit", "BIFROST_AP_JOURNAL_UNIT"),
    ("archipelago", "url", "BIFROST_AP_URL"),
    ("archipelago", "slot", "BIFROST_SLOT"),
    ("archipelago", "password", "BIFROST_AP_PASSWORD"),
    ("archipelago", "admin_password", "BIFROST_AP_ADMIN_PASSWORD"),
    ("runtime", "dir", "BIFROST_RUNTIME_DIR"),
    ("backup", "dir", "BIFROST_BACKUP_DIR"),
    ("backup", "hour", "BIFROST_BACKUP_HOUR"),
    ("backup", "keep_daily", "BIFROST_BACKUP_KEEP_DAILY"),
    ("backup", "keep_weekly", "BIFROST_BACKUP_KEEP_WEEKLY"),
    ("backup", "keep_monthly", "BIFROST_BACKUP_KEEP_MONTHLY"),
    ("backup", "max_mb", "BIFROST_BACKUP_MAX_MB"),
    ("updates", "check", "BIFROST_UPDATE_CHECK"),
    ("sandbox", "mode", "BIFROST_SANDBOX"),
    ("sandbox", "bwrap", "BIFROST_BWRAP"),
    ("sandbox", "mem_mb", "BIFROST_SANDBOX_MEM_MB"),
    ("sandbox", "cpu_secs", "BIFROST_SANDBOX_CPU_SECS"),
    ("sandbox", "pack_timeout_secs", "BIFROST_PACK_TIMEOUT_SECS"),
    ("sandbox", "file_mb", "BIFROST_SANDBOX_FILE_MB"),
    ("sandbox", "generate_mem_mb", "BIFROST_GENERATE_MEM_MB"),
    ("sandbox", "generate_cpu_secs", "BIFROST_GENERATE_CPU_SECS"),
    (
        "sandbox",
        "generate_timeout_min",
        "BIFROST_GENERATE_TIMEOUT_MIN",
    ),
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
    ("forward_auth", "season_pages", "BIFROST_FWD_SEASON_PAGES"),
    ("forward_auth", "sign_in_url", "BIFROST_FWD_SIGN_IN_URL"),
    ("forward_auth", "sign_out_url", "BIFROST_FWD_SIGN_OUT_URL"),
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

/// Where and why config.toml doesn't parse, without quoting the line: toml's
/// own message shows it, and the line may hold a secret.
fn parse_error(text: &str, e: &toml::de::Error) -> String {
    let what = e.message().lines().next().unwrap_or("invalid TOML");
    match e.span() {
        Some(span) => {
            // On a character boundary, as slicing elsewhere would panic and
            // the panic would quote the file; and at the end of the file, on
            // its last line rather than the empty one after it.
            let mut at = text.floor_char_boundary(span.start);
            if at == text.len() {
                at = text.trim_end_matches(['\n', '\r']).len();
            }
            let before = &text[..at];
            let line = before.matches('\n').count() + 1;
            let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
            format!("not valid TOML at line {line}, column {column}: {what}")
        }
        None => format!("not valid TOML: {what}"),
    }
}

/// Map config.toml onto `BIFROST_*` names. Unknown keys are errors (a typo'd
/// secret should fail loudly, not silently fall back to a default).
fn parse_file(text: &str) -> Result<HashMap<String, String>, String> {
    let table: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| parse_error(text, &e))?;
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
                .ok_or_else(|| {
                    if section == "minecraft" {
                        "[minecraft] is gone: a common game's address is set on the season's \
                         admin page, under Common games, or in its setup's game.toml"
                            .to_string()
                    } else {
                        format!("unknown setting [{section}] {key}")
                    }
                })?;
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
# bind = "0.0.0.0"                   # BIFROST_BIND (127.0.0.1 behind a proxy on this host)
# public_url = "https://ap.example"  # BIFROST_PUBLIC_URL (CLI-printed links)
# trusted_proxies = ["10.0.0.5"]     # BIFROST_TRUSTED_PROXIES (reverse proxies: X-Forwarded-For, forward-auth)
# insecure_cookies = false           # BIFROST_INSECURE_COOKIES (plain-HTTP dev only)

[archipelago]
# mode = "managed"                   # BIFROST_AP_MODE: managed (Bifrost runs the AP server) | external
# ports = "38281-38300"              # BIFROST_AP_PORTS (one per live season; the range size caps live seasons)
# bind = "0.0.0.0"                   # BIFROST_AP_BIND (managed servers)
# (The host players connect to is set on the instance settings page.)
# dir = "/opt/archipelago"           # BIFROST_AP_DIR
# python = "/opt/archipelago/venv/bin/python"  # BIFROST_AP_PYTHON
# — installing further releases from the settings page:
# repo = "vidarrio/Archipelago"  # BIFROST_AP_REPO (GitHub owner/name; Bifrost's fork)
# base_python = "python3"            # BIFROST_PYTHON (builds each release's venv; needs venv + pip)
# — external mode only (managed mode generates per-season passwords):
# url = "ws://127.0.0.1:38281"       # BIFROST_AP_URL
# slot = "Tracker"                   # BIFROST_SLOT (tracker's slot; managed: first slot)
# password = ""                      # BIFROST_AP_PASSWORD (secret)
# admin_password = ""                # BIFROST_AP_ADMIN_PASSWORD (secret)
# journal_unit = "archipelago"       # BIFROST_AP_JOURNAL_UNIT

[runtime]
# dir = "/opt/bifrost"               # BIFROST_RUNTIME_DIR (scripts/ + vendor/)

[backup]
# dir = "/var/lib/bifrost/backups"   # BIFROST_BACKUP_DIR (another disk is better)
# hour = 4                           # BIFROST_BACKUP_HOUR: nightly, UTC (0–23) | "off"
# keep_daily = 7                     # BIFROST_BACKUP_KEEP_DAILY: newest of each of the last N days
# keep_weekly = 4                    # BIFROST_BACKUP_KEEP_WEEKLY: … of the last N weeks
# keep_monthly = 3                   # BIFROST_BACKUP_KEEP_MONTHLY: … of the last N months
# max_mb = 5120                      # BIFROST_BACKUP_MAX_MB: drop the oldest beyond this total

[updates]
# check = true                       # BIFROST_UPDATE_CHECK: ask GitHub daily for a newer Bifrost (shown to sysadmins)

[sandbox]
# mode = "auto"                      # BIFROST_SANDBOX: auto | bwrap | none
# mem_mb = 4096                      # BIFROST_SANDBOX_MEM_MB
# cpu_secs = 1800                    # BIFROST_SANDBOX_CPU_SECS
# pack_timeout_secs = 30             # BIFROST_PACK_TIMEOUT_SECS
# file_mb = 1024                     # BIFROST_SANDBOX_FILE_MB
# generate_mem_mb = 16384            # BIFROST_GENERATE_MEM_MB (address space, not RSS)
# generate_cpu_secs = 7200           # BIFROST_GENERATE_CPU_SECS
# generate_timeout_min = 120         # BIFROST_GENERATE_TIMEOUT_MIN
# allow_unsandboxed_custom_worlds = false  # BIFROST_ALLOW_UNSANDBOXED_CUSTOM_WORLDS

[forward_auth]                       # only with `bifrost auth mode forward-auth`
# secret_header = "x-bifrost-proxy"  # BIFROST_FWD_SECRET_HEADER
# secret = ""                        # BIFROST_FWD_SECRET (secret)
# role_groups = ["sysadmin=archipelago_admins", "moderator=archipelago_moderators", "user=archipelago_users"]  # BIFROST_FWD_ROLE_GROUPS
# season_pages = false              # BIFROST_FWD_SEASON_PAGES: only once the proxy covers /board, /log, /season
# sign_in_url = "/oauth2/sign_in"    # BIFROST_FWD_SIGN_IN_URL
# sign_out_url = "/oauth2/sign_out"  # BIFROST_FWD_SIGN_OUT_URL

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
        assert_eq!(c.bind.to_string(), "0.0.0.0");
        assert_eq!(c.data_dir, Path::new("/var/lib/bifrost"));
        assert_eq!(c.ap_dir, Path::new("/opt/archipelago"));
        assert_eq!(c.ap_python, Path::new("/opt/archipelago/venv/bin/python"));
        assert_eq!(
            c.script("bifrost_meta.py"),
            Path::new("/opt/bifrost/scripts/bifrost_meta.py")
        );
        assert_eq!(c.ap_journal_unit, "archipelago");
        assert_eq!(c.ap_ports, 38281..=38300);
    }

    #[test]
    fn bind_address() {
        let b = |v: &str| with(&[("BIFROST_BIND", v)]);
        assert_eq!(b(" 127.0.0.1 ").bind.to_string(), "127.0.0.1");
        assert_eq!(b("::1").bind.to_string(), "::1");
        // Reaching our own listener: loopback for "every interface".
        assert_eq!(with(&[]).local_addr().to_string(), "127.0.0.1:8080");
        assert_eq!(b("::").local_addr().to_string(), "[::1]:8080");
        assert_eq!(b("192.0.2.5").local_addr().to_string(), "192.0.2.5:8080");
        assert!(std::panic::catch_unwind(|| with(&[("BIFROST_BIND", "nope")])).is_err());
    }

    #[test]
    fn port_ranges() {
        assert_eq!(parse_port_range("40000-40009"), Ok(40000..=40009));
        assert_eq!(parse_port_range(" 38281 "), Ok(38281..=38281));
        assert!(parse_port_range("40009-40000").is_err());
        assert!(parse_port_range("0-10").is_err());
        assert!(parse_port_range("a-b").is_err());
        assert_eq!(
            with(&[("BIFROST_AP_PORTS", "5000-5001")]).ap_ports,
            5000..=5001
        );
    }

    #[test]
    fn all_state_lives_under_the_data_dir() {
        let c = with(&[("BIFROST_DATA", "/data")]);
        let id = crate::id::Id::new();
        let s = c.season(id);
        for p in [
            c.db_path(),
            c.packs_dir(),
            c.uploads_dir(),
            c.common_dir(),
            c.archives_dir(),
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
            Path::new(&format!("/data/seasons/{id}/output/current.zip"))
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
    fn files_the_database_names_stay_in_the_data_dir() {
        let c = with(&[("BIFROST_DATA", "/data")]);
        let path = c
            .season(crate::id::Id::new())
            .root
            .join("submissions/a.yaml");
        let rel = c.data_name(&path).unwrap();
        assert!(!rel.starts_with('/'), "{rel}");
        assert_eq!(c.data_file(&rel).unwrap(), path);
        for bad in ["", "/etc/passwd", "../x", "seasons/../../x", "./x"] {
            assert!(c.data_file(bad).is_none(), "{bad}");
        }
        assert!(c.data_name(Path::new("/elsewhere/a.yaml")).is_none());
    }

    #[test]
    fn the_old_minecraft_section_says_where_it_went() {
        let e = parse_file("[minecraft]\naddress = \"mc.example:25565\"").unwrap_err();
        assert!(e.contains("Common games"), "{e}");
    }

    #[test]
    fn a_file_that_doesnt_parse_isnt_quoted_back() {
        for (text, at) in [
            ("[archipelago]\npassword = \"s3cret\n", "line 2, column 19:"),
            (
                "[archipelago]\npassword = \"s3cret\"\npassword = \"s3cret\"\n",
                "line 3, column 1:",
            ),
            ("[archipelago]\npassword = s3cret\n", "line 2, column 12:"),
            // Columns count characters, and CRLF ends a line.
            ("[a]\r\nb = \"s3cretåäö\" x\r\n", "line 2, column 17:"),
            // At the end of the file, the last line.
            ("[a]\nb = \"\"\"s3cret\n", "line 2, column 14:"),
            (
                "[archipelago]\npassword = \"s3cret\"\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\npassword = 1\n",
                "line 22, column 1:",
            ),
        ] {
            let err = parse_file(text).unwrap_err();
            assert!(!err.contains("s3cret"), "{err}");
            assert!(err.contains(&format!(" at {at} ")), "{text:?}: {err}");
        }
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

    #[test]
    fn root_has_to_ask() {
        use std::ffi::OsStr;
        assert!(root_refused(0, None));
        assert!(root_refused(0, Some(OsStr::new("0"))));
        assert!(root_refused(0, Some(OsStr::new(""))));
        assert!(!root_refused(0, Some(OsStr::new("1"))));
        assert!(!root_refused(1000, None));
    }

    #[test]
    fn root_only_uses_a_data_dir_nobody_else_can_change() {
        use std::os::unix::fs::PermissionsExt;
        let me = unsafe { libc::geteuid() };
        let tmp = std::env::temp_dir().join(format!("bifrost-held-{}", std::process::id()));
        let data = tmp.join("data");
        let deep = data.join("ap/1.0");
        let file = deep.join("python");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(&file, b"").unwrap();
        let set = |p: &Path, mode| {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap()
        };
        for d in [&tmp, &data, &data.join("ap"), &deep] {
            set(d, 0o700);
        }
        set(&file, 0o700);
        let check = |p: &Path| root_data_dir(p, me);

        // The folders above are the test machine's own: skip if they'd fail anyway.
        if check(&data).is_ok() {
            // Through a link, and it's the real path that's used.
            let link = tmp.join("link");
            std::os::unix::fs::symlink(&data, &link).unwrap();
            assert_eq!(check(&link).unwrap(), data.canonicalize().unwrap());
            // Created when missing, and private.
            let fresh = tmp.join("fresh/er");
            check(&fresh).unwrap();
            assert_eq!(
                std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
                0o700
            );

            // Deep inside, a file or a folder.
            set(&file, 0o720);
            assert!(check(&data).unwrap_err().contains("others can write"));
            set(&file, 0o700);
            set(&deep, 0o777);
            assert!(check(&data).unwrap_err().contains("others can write"));
            set(&deep, 0o700);
            // Sticky will do above the data dir, but not for it.
            set(&data, 0o1777);
            assert!(check(&data).is_err());
            set(&data, 0o700);
            set(&tmp, 0o1777);
            check(&data).unwrap();
            set(&tmp, 0o700);

            // Once created, a part that's gone is an error.
            assert!(check_way(&tmp.join("gone/x"), me, 40, false).is_ok());
            assert!(check_way(&tmp.join("gone/x"), me, 40, true).is_err());

            // A way there others can change counts, wherever it leads.
            let open = tmp.join("open");
            std::fs::create_dir(&open).unwrap();
            std::os::unix::fs::symlink(&data, open.join("link")).unwrap();
            set(&open, 0o777);
            assert!(
                check(&open.join("link"))
                    .unwrap_err()
                    .contains("others can write")
            );
            set(&open, 0o1777);
            check(&open.join("link")).unwrap();
        } else {
            eprintln!("skipped: the folders above the temp dir are open to others");
        }
        if me != 0 {
            let meta = std::fs::symlink_metadata(&tmp).unwrap();
            assert!(
                check_held(&tmp, &meta, me + 1, false)
                    .unwrap_err()
                    .contains("another user")
            );
        }
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
