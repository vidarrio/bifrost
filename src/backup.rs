//! Backups of the data dir: a nightly snapshot, a button for
//! one on demand, and a retention policy so they don't eat the disk.
//!
//! A backup is one zip, `bifrost-YYYYMMDD-HHMMSS.zip` (UTC), holding a
//! consistent copy of the database (SQLite `VACUUM INTO`) and every other
//! file under `$BIFROST_DATA` — except the backups themselves, installed
//! Archipelago releases (`ap/`, reinstallable and large), `tmp/` and
//! `.cache/`. It's everything needed to bring an instance back,
//! `config.toml` secrets included, so backups are written owner-only.
//!
//! Retention keeps the newest backup of each of the last `daily` days, the
//! last `weekly` weeks and the last `monthly` months that have one, then
//! drops the oldest while the total is over `max_mb` (never the newest). A
//! backup is skipped, not squeezed in, when the disk is nearly full.
//!
//! The backup taken on the first start of a new Bifrost version is named
//! `bifrost-YYYYMMDD-HHMMSS-pre-upgrade-from-<version>.zip`, after the
//! version that last ran (the one to go back to), and sits outside that policy:
//! the newest [`UPGRADE_KEEP`] of them are kept, whatever their age or size,
//! so going back to the previous version stays possible.
//! Restoring is a CLI step with Bifrost stopped (`bifrost backup restore`).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::config;
use crate::db::Db;

/// Pre-upgrade backups kept (see the module docs).
pub const UPGRADE_KEEP: usize = 3;
const UPGRADE_SUFFIX: &str = "-pre-upgrade-from-";

/// Free space a backup must leave behind.
const HEADROOM: u64 = 512 << 20;

#[derive(Clone, Debug)]
pub struct Policy {
    /// UTC hour of the nightly backup; `None` turns it off.
    pub hour: Option<u32>,
    pub daily: usize,
    pub weekly: usize,
    pub monthly: usize,
    pub max_bytes: u64,
}

impl Policy {
    pub fn from_config() -> Self {
        let num = |name: &str, default: u64| -> u64 {
            config::var(name)
                .map(|v| {
                    v.trim()
                        .parse()
                        .unwrap_or_else(|_| panic!("{name}: expected a number, got {v:?}"))
                })
                .unwrap_or(default)
        };
        let hour = match config::var("BIFROST_BACKUP_HOUR").as_deref().map(str::trim) {
            Some("off") => None,
            Some(h) => Some(
                h.parse::<u32>()
                    .ok()
                    .filter(|h| *h < 24)
                    .unwrap_or_else(|| panic!("BIFROST_BACKUP_HOUR: 0–23 or \"off\", got {h:?}")),
            ),
            None => Some(4),
        };
        Self {
            hour,
            daily: num("BIFROST_BACKUP_KEEP_DAILY", 7) as usize,
            weekly: num("BIFROST_BACKUP_KEEP_WEEKLY", 4) as usize,
            monthly: num("BIFROST_BACKUP_KEEP_MONTHLY", 3) as usize,
            max_bytes: num("BIFROST_BACKUP_MAX_MB", 5120) << 20,
        }
    }
}

pub fn dir() -> PathBuf {
    config::var("BIFROST_BACKUP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| config::get().data_dir.join("backups"))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Backup {
    pub name: String,
    /// When it was taken (epoch seconds, from its name).
    pub at: i64,
    pub size: u64,
}

/// A backup's name; `from` is the version a pre-upgrade backup goes back to.
fn name_for(at: i64, from: Option<&str>) -> String {
    let (y, m, d) = civil(at.div_euclid(86400));
    let s = at.rem_euclid(86400);
    format!(
        "bifrost-{y:04}{m:02}{d:02}-{:02}{:02}{:02}{}.zip",
        s / 3600,
        s / 60 % 60,
        s % 60,
        from.map(|v| format!("{UPGRADE_SUFFIX}{}", version_part(v)))
            .unwrap_or_default()
    )
}

/// A version as it may appear in a backup's name (semver's characters only).
fn version_part(v: &str) -> String {
    let v: String = v.chars().filter(|c| version_char(*c)).collect();
    if v.is_empty() { "unknown".into() } else { v }
}

fn version_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')
}

/// Is this backup one taken before an upgrade?
pub fn is_pre_upgrade(name: &str) -> bool {
    parse_name(name).is_some() && name.contains(UPGRADE_SUFFIX)
}

/// The time a backup's name says it was taken.
pub fn parse_name(name: &str) -> Option<i64> {
    let ts = name.strip_prefix("bifrost-")?.strip_suffix(".zip")?;
    let ts = match ts.split_once(UPGRADE_SUFFIX) {
        Some((ts, from)) if !from.is_empty() && from.chars().all(version_char) => ts,
        Some(_) => return None,
        None => ts,
    };
    let (date, time) = ts.split_once('-')?;
    if date.len() != 8 || time.len() != 6 || !ts.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return None;
    }
    let n = |s: &str| s.parse::<i64>().ok();
    let (y, m, d) = (n(&date[..4])?, n(&date[4..6])?, n(&date[6..])?);
    let (hh, mm, ss) = (n(&time[..2])?, n(&time[2..4])?, n(&time[4..])?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86400 + hh * 3600 + mm * 60 + ss)
}

/// Backups on disk, newest first.
pub fn list() -> Vec<Backup> {
    let mut v: Vec<Backup> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            Some(Backup {
                at: parse_name(&name)?,
                size: e.metadata().ok()?.len(),
                name,
            })
        })
        .collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.at));
    v
}

/// A backup's path, if `name` is one.
pub fn path(name: &str) -> Option<PathBuf> {
    parse_name(name)?;
    let p = dir().join(name);
    p.is_file().then_some(p)
}

// ---- calendar (UTC, proleptic Gregorian; Howard Hinnant's algorithms) ----

fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Which backups (newest first) the policy keeps.
pub fn plan(backups: &[Backup], p: &Policy) -> Vec<bool> {
    let mut keep = vec![false; backups.len()];
    if backups.is_empty() {
        return keep;
    }
    keep[0] = true;
    let day = |b: &Backup| b.at.div_euclid(86400);
    // 1970-01-01 was a Thursday; shift so weeks start on Monday.
    let week = |b: &Backup| (day(b) + 3).div_euclid(7);
    let month = |b: &Backup| {
        let (y, m, _) = civil(day(b));
        y * 12 + m
    };
    let mut bucket = |n: usize, key: &dyn Fn(&Backup) -> i64| {
        let mut seen = Vec::new();
        for (i, b) in backups.iter().enumerate() {
            let k = key(b);
            if !seen.contains(&k) {
                if seen.len() == n {
                    break;
                }
                seen.push(k);
                keep[i] = true; // newest of its bucket (the list is newest first)
            }
        }
    };
    bucket(p.daily, &day);
    bucket(p.weekly, &week);
    bucket(p.monthly, &month);
    let mut total: u64 = backups
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(b, _)| b.size)
        .sum();
    for i in (1..backups.len()).rev() {
        if total <= p.max_bytes {
            break;
        }
        if keep[i] {
            keep[i] = false;
            total -= backups[i].size;
        }
    }
    keep
}

/// Delete the backups the policy doesn't keep. Returns their names.
pub fn prune(p: &Policy) -> Vec<String> {
    let (upgrades, all): (Vec<Backup>, Vec<Backup>) =
        list().into_iter().partition(|b| is_pre_upgrade(&b.name));
    let keep = plan(&all, p);
    let mut gone = Vec::new();
    let upgrades = upgrades
        .into_iter()
        .enumerate()
        .map(|(i, b)| (b, i < UPGRADE_KEEP));
    for (b, k) in all.into_iter().zip(keep).chain(upgrades) {
        if !k && std::fs::remove_file(dir().join(&b.name)).is_ok() {
            gone.push(b.name.clone());
        }
    }
    if !gone.is_empty() {
        tracing::info!("backups: removed {} under the retention policy", gone.len());
    }
    gone
}

fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0)
        .then(|| s.f_bavail as u64 * s.f_frsize as u64)
}

/// A path as the filesystem resolves it (symlinks, `..`), so the backup dir
/// is recognised inside the data dir however either is spelled.
fn resolved(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// What a backup leaves out, relative to the data dir. `data` and `backups`
/// are [`resolved`].
fn excluded(data: &Path, rel: &Path, backups: &Path) -> bool {
    let top = rel
        .components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned());
    // `ap`, `runtime`: reinstallable (installed releases; the native bundle's
    // unpacked Python + AP, which a restore must not swap for a copy without
    // its symlinks and exec bits). `.cache`: tools that treat the data dir as
    // $HOME (the container's user).
    matches!(top.as_deref(), Some("ap" | "runtime" | "tmp" | ".cache"))
        || rel
            .to_str()
            .is_some_and(|r| r == "bifrost.db" || r.starts_with("bifrost.db-"))
        || data.join(rel).starts_with(backups)
        || top
            .as_deref()
            .is_some_and(|t| t.starts_with(".restore-") || t.starts_with(".pre-restore-"))
}

/// Write the zip: the database snapshot plus every other kept file.
fn write_zip(out: &Path, data: &Path, db_snapshot: &Path, backups: &Path) -> Result<u64, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(out)
        .map_err(|e| format!("{}: {e}", out.display()))?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    let mut add = |name: &str, src: &Path| -> Result<(), String> {
        let mut f = std::fs::File::open(src).map_err(|e| format!("{}: {e}", src.display()))?;
        zip.start_file(name, opts).map_err(|e| e.to_string())?;
        std::io::copy(&mut f, &mut zip).map_err(|e| format!("{}: {e}", src.display()))?;
        Ok(())
    };
    add("bifrost.db", db_snapshot)?;
    let mut files = 1u64;
    let mut stack = vec![data.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(rel) = p.strip_prefix(data) else {
                continue;
            };
            if excluded(data, rel, backups) {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file() {
                // A file that vanished mid-walk (a save being replaced) is skipped.
                match add(&rel.to_string_lossy(), &p) {
                    Ok(()) => files += 1,
                    Err(e) => tracing::warn!("backup: skipped {e}"),
                }
            }
        }
    }
    zip.finish()
        .map_err(|e| e.to_string())?
        .flush()
        .map_err(|e| e.to_string())?;
    Ok(files)
}

static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The last backup run's outcome, for the settings page: (when, message, ok).
static LAST: std::sync::Mutex<Option<(i64, String, bool)>> = std::sync::Mutex::new(None);

pub fn last() -> Option<(i64, String, bool)> {
    LAST.lock().unwrap().clone()
}

pub fn running() -> bool {
    RUNNING.try_lock().is_err()
}

/// Take a backup now, then apply the retention policy.
pub async fn create(db: &Db, p: &Policy) -> Result<Backup, String> {
    create_kind(db, p, None).await
}

/// Take the backup before an upgrade from version `from` (kept outside the
/// retention policy).
pub async fn create_pre_upgrade(db: &Db, p: &Policy, from: &str) -> Result<Backup, String> {
    create_kind(db, p, Some(from)).await
}

async fn create_kind(db: &Db, p: &Policy, from: Option<&str>) -> Result<Backup, String> {
    let res = create_inner(db, p, from).await;
    let msg = match &res {
        Ok(b) => {
            tracing::info!("backup written: {} ({} bytes)", b.name, b.size);
            (
                crate::auth::now(),
                format!("Backed up to {}.", b.name),
                true,
            )
        }
        Err(e) => {
            tracing::error!("backup failed: {e}");
            (crate::auth::now(), format!("Backup failed: {e}"), false)
        }
    };
    *LAST.lock().unwrap() = Some(msg);
    res
}

async fn create_inner(db: &Db, p: &Policy, from: Option<&str>) -> Result<Backup, String> {
    let _one = RUNNING
        .try_lock()
        .map_err(|_| "a backup is already running".to_string())?;
    let dir = dir();
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    // Room for another one the size of the last (or of the database, if
    // there's none yet), plus headroom.
    let estimate = list().first().map(|b| b.size).unwrap_or_else(|| {
        std::fs::metadata(config::get().db_path())
            .map(|m| m.len())
            .unwrap_or(0)
    });
    if let Some(free) = free_bytes(&dir)
        && free < estimate + HEADROOM
    {
        return Err(format!(
            "only {} MiB free in {} — skipped rather than fill the disk",
            free >> 20,
            dir.display()
        ));
    }
    let mut at = crate::auth::now();
    while dir.join(name_for(at, from)).exists() {
        at += 1;
    }
    let name = name_for(at, from);
    let partial = dir.join(format!(".partial-{name}"));
    let snapshot = dir.join(format!(".partial-{name}.db"));
    let _ = std::fs::remove_file(&partial);
    let _ = std::fs::remove_file(&snapshot);
    db.snapshot_to(&snapshot)
        .await
        .map_err(|e| format!("database snapshot: {e}"))?;
    let (data, out, snap, bdir) = (
        resolved(&config::get().data_dir),
        partial.clone(),
        snapshot.clone(),
        resolved(&dir),
    );
    let written = tokio::task::spawn_blocking(move || write_zip(&out, &data, &snap, &bdir))
        .await
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&snapshot);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }
    std::fs::rename(&partial, dir.join(&name)).map_err(|e| e.to_string())?;
    prune(p);
    let size = std::fs::metadata(dir.join(&name))
        .map(|m| m.len())
        .unwrap_or(0);
    Ok(Backup { name, at, size })
}

/// Delete one backup.
pub fn delete(name: &str) -> Result<(), String> {
    let p = path(name).ok_or("No such backup.")?;
    std::fs::remove_file(p).map_err(|e| e.to_string())
}

/// Seconds from `now` until the next `hour`:00 UTC.
fn until_next(now: i64, hour: u32) -> i64 {
    let target = i64::from(hour) * 3600;
    let into_day = now.rem_euclid(86400);
    let wait = target - into_day;
    if wait > 0 { wait } else { wait + 86400 }
}

/// The nightly backup loop.
pub fn spawn_nightly(db: Db) {
    let p = Policy::from_config();
    let Some(hour) = p.hour else {
        tracing::info!("nightly backups are off (BIFROST_BACKUP_HOUR=off)");
        return;
    };
    tracing::info!(
        "nightly backups at {hour:02}:00 UTC to {} (keeping {} daily, {} weekly, {} monthly, up to {} MiB)",
        dir().display(),
        p.daily,
        p.weekly,
        p.monthly,
        p.max_bytes >> 20
    );
    tokio::spawn(async move {
        loop {
            let wait = until_next(crate::auth::now(), hour);
            tokio::time::sleep(std::time::Duration::from_secs(wait as u64)).await;
            let _ = create(&db, &p).await;
        }
    });
}

/// Restore a backup into the data dir (Bifrost must be stopped). The current
/// contents move to `.pre-restore-<time>/` rather than being deleted.
pub fn restore(file: &Path) -> Result<String, String> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let cfg = config::get();
    // Where Bifrost would be listening, whatever `bind` says.
    let addr = cfg.local_addr();
    if std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(2)).is_ok() {
        return Err(format!(
            "something is listening on {addr} — stop Bifrost before restoring"
        ));
    }
    let data = &cfg.data_dir;
    let now = crate::auth::now();
    let staging = data.join(format!(".restore-{now}"));
    let aside = data.join(format!(".pre-restore-{now}"));
    let f = std::fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("not a backup zip: {e}"))?;
    if zip.by_name("bifrost.db").is_err() {
        return Err("that zip has no bifrost.db — not a Bifrost backup".into());
    }
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let extracted = (|| -> Result<(), String> {
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
            let Some(rel) = entry.enclosed_name() else {
                return Err(format!("unsafe path in backup: {:?}", entry.name()));
            };
            if entry.is_dir() {
                continue;
            }
            let dest = staging.join(rel);
            // Owner-only, like the backup itself: it holds config.toml's
            // secrets and the account database.
            if let Some(parent) = dest.parent() {
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)
                    .map_err(|e| e.to_string())?;
            }
            let mut out = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&dest)
                .map_err(|e| format!("{}: {e}", dest.display()))?;
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            out.write_all(&buf).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    if let Err(e) = extracted {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    // Move what's there aside (keeping backups, installed AP releases and
    // tmp in place), then move the backup's contents in.
    std::fs::create_dir_all(&aside).map_err(|e| e.to_string())?;
    let backups = resolved(&dir());
    let data = &resolved(data);
    for e in std::fs::read_dir(data)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let p = e.path();
        let rel = PathBuf::from(e.file_name());
        let db_file = rel.to_string_lossy().starts_with("bifrost.db");
        if p == staging || p == aside || excluded(data, &rel, &backups) && !db_file {
            continue;
        }
        std::fs::rename(&p, aside.join(&rel))
            .map_err(|e| format!("moving {} aside: {e}", p.display()))?;
    }
    for e in std::fs::read_dir(&staging)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        std::fs::rename(e.path(), data.join(e.file_name())).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_dir(&staging);
    Ok(format!(
        "restored {} into {}; the previous contents are in {}",
        file.display(),
        data.display(),
        aside.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(at: i64, size: u64) -> Backup {
        Backup {
            name: name_for(at, None),
            at,
            size,
        }
    }

    #[test]
    fn names_round_trip() {
        let at = 1_793_016_000 + 3723; // 2026-10-26 01:02:03 UTC-ish
        assert_eq!(parse_name(&name_for(at, None)), Some(at));
        assert_eq!(parse_name(&name_for(at, Some("0.2.0-rc.1"))), Some(at));
        assert_eq!(name_for(0, None), "bifrost-19700101-000000.zip");
        assert_eq!(
            name_for(0, Some("0.1.0")),
            "bifrost-19700101-000000-pre-upgrade-from-0.1.0.zip"
        );
        assert_eq!(
            name_for(0, Some("../x/")),
            "bifrost-19700101-000000-pre-upgrade-from-..x.zip"
        );
        assert!(is_pre_upgrade(&name_for(0, Some("0.1.0"))));
        assert!(!is_pre_upgrade(&name_for(0, None)));
        assert_eq!(
            parse_name("bifrost-20261026-040000-pre-upgrade-from-.zip"),
            None
        );
        assert_eq!(
            parse_name("bifrost-20261026-040000-pre-upgrade-from-a/b.zip"),
            None
        );
        assert_eq!(parse_name("bifrost-20261026-040000-other.zip"), None);
        assert_eq!(
            parse_name("bifrost-20261026-040000.zip"),
            Some(days_from_civil(2026, 10, 26) * 86400 + 4 * 3600)
        );
        assert_eq!(parse_name("bifrost-20261326-040000.zip"), None);
        assert_eq!(parse_name("other.zip"), None);
        assert_eq!(civil(days_from_civil(2024, 2, 29)), (2024, 2, 29));
    }

    #[test]
    fn retention_keeps_daily_weekly_monthly() {
        let p = Policy {
            hour: Some(4),
            daily: 7,
            weekly: 4,
            monthly: 3,
            max_bytes: u64::MAX,
        };
        // One backup a night for 120 nights, plus a manual one on the last day.
        let start = days_from_civil(2026, 6, 1) * 86400 + 4 * 3600;
        let mut all: Vec<Backup> = (0..120).map(|d| b(start + d * 86400, 10)).collect();
        all.push(b(start + 119 * 86400 + 3600, 10));
        all.reverse();
        let keep = plan(&all, &p);
        let kept: Vec<&Backup> = all
            .iter()
            .zip(&keep)
            .filter(|(_, k)| **k)
            .map(|(b, _)| b)
            .collect();
        assert!(keep[0], "the newest is always kept");
        assert!(!keep[1], "only the newest of a day");
        // 7 days + older weeks + older months, without duplicates.
        assert!(kept.len() <= 7 + 4 + 3 && kept.len() >= 9, "{}", kept.len());
        let oldest = kept.last().unwrap().at;
        assert!(
            start + 119 * 86400 - oldest < 100 * 86400,
            "nothing older than ~3 months"
        );
        assert_eq!(plan(&[], &p), Vec::<bool>::new());
    }

    #[test]
    fn retention_respects_the_size_cap_but_keeps_the_newest() {
        let p = Policy {
            hour: None,
            daily: 7,
            weekly: 0,
            monthly: 0,
            max_bytes: 25,
        };
        let all: Vec<Backup> = (0..7).rev().map(|d| b(d * 86400, 10)).collect();
        assert_eq!(
            plan(&all, &p),
            vec![true, true, false, false, false, false, false]
        );
        let huge = vec![b(0, 100)];
        assert_eq!(plan(&huge, &p), vec![true]);
    }

    #[test]
    fn backup_dir_is_found_through_symlinks() {
        let root = std::env::temp_dir().join(format!("bifrost-bk-{}", std::process::id()));
        let real = root.join("real");
        std::fs::create_dir_all(real.join("backups")).unwrap();
        let link = root.join("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        // Data dir given through the symlink, backup dir by its real path.
        let (data, bk) = (resolved(&link), resolved(&real.join("backups")));
        assert!(excluded(&data, Path::new("backups/bifrost-x.zip"), &bk));
        assert!(!excluded(&data, Path::new("seasons/1/x"), &bk));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn next_run() {
        assert_eq!(until_next(0, 4), 4 * 3600);
        assert_eq!(until_next(4 * 3600, 4), 86400);
        assert_eq!(until_next(5 * 3600, 4), 23 * 3600);
    }

    #[test]
    fn exclusions() {
        let d = Path::new("/data");
        let bk = Path::new("/data/backups");
        assert!(excluded(d, Path::new("ap/0.6.9/Utils.py"), bk));
        assert!(excluded(d, Path::new("tmp/x"), bk));
        assert!(excluded(
            d,
            Path::new("runtime/0.0.1-ap0.6.8/python/bin/python3"),
            bk
        ));
        assert!(excluded(d, Path::new(".cache/pip/x"), bk));
        assert!(excluded(d, Path::new("bifrost.db-wal"), bk));
        assert!(excluded(d, Path::new("backups/bifrost-x.zip"), bk));
        assert!(!excluded(d, Path::new("seasons/1/output/current.zip"), bk));
        assert!(!excluded(d, Path::new("config.toml"), bk));
        assert!(!excluded(d, Path::new("apples/x"), bk));
    }
}
