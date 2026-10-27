//! The native single-file bundle: the Bifrost
//! binary with a zip of its runtime appended — a relocatable Python
//! (python-build-standalone) with Archipelago's requirements installed into
//! it, the pinned Archipelago source, and Bifrost's scripts + vendored worlds
//! — followed by a trailer: the payload's length (u64 LE) and [`MAGIC`].
//!
//! On start, a binary that carries a payload unpacks it once into
//! `$BIFROST_DATA/runtime/<bundle id>/` (atomically: `.partial-…` then
//! rename) and uses it as the bundled install, unless the operator pointed
//! `BIFROST_AP_DIR` & co. elsewhere. A newer bundle unpacks next to the old
//! one; old runtimes are removed unless an installed Archipelago release's
//! venv still points into one.
//!
//! See `packaging/bundle/` for how a bundle is built.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

const MAGIC: &[u8; 16] = b"BIFROST-BUNDLE01";
const TRAILER: u64 = 8 + 16;
/// The payload entry naming the bundle (its build), e.g. `0.0.1-ap0.6.8-20261026`.
const ID_ENTRY: &str = "BUNDLE_ID";
const COMPLETE: &str = ".complete";

/// A bundled runtime unpacked under the data dir.
pub struct Runtime {
    pub root: PathBuf,
}

impl Runtime {
    pub fn ap_dir(&self) -> PathBuf {
        self.root.join("archipelago")
    }
    pub fn python(&self) -> PathBuf {
        self.root.join("python/bin/python3")
    }
    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join("bifrost")
    }
}

struct Payload {
    exe: PathBuf,
    offset: u64,
    len: u64,
}

fn detect() -> Option<Payload> {
    let exe = std::env::current_exe().ok()?;
    let mut f = std::fs::File::open(&exe).ok()?;
    let size = f.metadata().ok()?.len();
    if size < TRAILER {
        return None;
    }
    f.seek(SeekFrom::End(-(TRAILER as i64))).ok()?;
    let mut t = [0u8; TRAILER as usize];
    f.read_exact(&mut t).ok()?;
    if &t[8..] != MAGIC {
        return None;
    }
    let len = u64::from_le_bytes(t[..8].try_into().ok()?);
    let offset = size.checked_sub(TRAILER)?.checked_sub(len)?;
    Some(Payload { exe, offset, len })
}

/// Whether this binary carries a bundled runtime.
pub fn present() -> bool {
    detect().is_some()
}

/// A window onto the payload inside the executable.
struct Slice {
    f: std::fs::File,
    start: u64,
    len: u64,
    pos: u64,
}

impl Read for Slice {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.len.saturating_sub(self.pos);
        let n = (buf.len() as u64).min(left) as usize;
        if n == 0 {
            return Ok(0);
        }
        self.f.seek(SeekFrom::Start(self.start + self.pos))?;
        let n = self.f.read(&mut buf[..n])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Slice {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(d) => self.len as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if pos < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before start"));
        }
        self.pos = pos as u64;
        Ok(self.pos)
    }
}

fn open(p: &Payload) -> Result<zip::ZipArchive<std::io::BufReader<Slice>>, String> {
    let f = std::fs::File::open(&p.exe).map_err(|e| e.to_string())?;
    let slice = Slice { f, start: p.offset, len: p.len, pos: 0 };
    zip::ZipArchive::new(std::io::BufReader::new(slice)).map_err(|e| format!("bundled runtime: {e}"))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && !id.starts_with('.') && id.chars().all(|c| c.is_ascii_alphanumeric() || "._-+".contains(c))
}

/// Where a bundle keeps its data when `BIFROST_DATA` isn't set: the usual
/// service location for root, the user's data dir otherwise.
pub fn default_data_dir() -> PathBuf {
    if unsafe { libc::geteuid() } == 0 {
        return "/var/lib/bifrost".into();
    }
    std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| ".".into())
        .join("bifrost")
}

/// `target` of a symlink at `link` (both relative to the extraction root)
/// stays inside the root.
fn link_inside(link: &Path, target: &Path) -> bool {
    if target.is_absolute() {
        return false;
    }
    let mut depth: i64 = link.components().count() as i64 - 1;
    for c in target.components() {
        match c {
            Component::ParentDir => depth -= 1,
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return false,
        }
        if depth < 0 {
            return false;
        }
    }
    true
}

fn extract(zip: &mut zip::ZipArchive<std::io::BufReader<Slice>>, dest: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = e.enclosed_name() else {
            return Err(format!("unsafe path in bundle: {:?}", e.name()));
        };
        let out = dest.join(&rel);
        if e.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        if e.is_symlink() {
            let mut target = String::new();
            e.read_to_string(&mut target).map_err(|e| e.to_string())?;
            if !link_inside(&rel, Path::new(&target)) {
                return Err(format!("bundle symlink {} points outside it", rel.display()));
            }
            std::os::unix::fs::symlink(&target, &out).map_err(|e| format!("{}: {e}", out.display()))?;
            continue;
        }
        let mut f = std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        std::io::copy(&mut e, &mut f).map_err(|e| format!("{}: {e}", out.display()))?;
        if let Some(mode) = e.unix_mode() {
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o755));
        }
    }
    Ok(())
}

/// Old runtimes still referenced by an installed release's venv
/// (`ap/<v>/venv/pyvenv.cfg` names the Python it was built from).
fn referenced(data_dir: &Path, runtime: &Path) -> bool {
    let needle = runtime.to_string_lossy().into_owned();
    std::fs::read_dir(data_dir.join("ap"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("venv/pyvenv.cfg")).ok())
        .any(|cfg| cfg.contains(&needle))
}

/// Unpack this binary's runtime under `data_dir` if it carries one (once per
/// bundle build). `None` for a plain binary.
pub fn prepare(data_dir: &Path) -> Result<Option<Runtime>, String> {
    let Some(p) = detect() else { return Ok(None) };
    let mut zip = open(&p)?;
    let mut id = String::new();
    zip.by_name(ID_ENTRY)
        .map_err(|_| "bundled runtime has no BUNDLE_ID".to_string())?
        .read_to_string(&mut id)
        .map_err(|e| e.to_string())?;
    let id = id.trim().to_string();
    if !valid_id(&id) {
        return Err(format!("bundled runtime has a bad id {id:?}"));
    }
    let base = data_dir.join("runtime");
    let dest = base.join(&id);
    if !dest.join(COMPLETE).is_file() {
        tracing::info!("unpacking the bundled runtime {id} into {} (first run of this build)…", dest.display());
        let partial = base.join(format!(".partial-{id}"));
        let _ = std::fs::remove_dir_all(&partial);
        std::fs::create_dir_all(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
        if let Err(e) = extract(&mut zip, &partial) {
            let _ = std::fs::remove_dir_all(&partial);
            return Err(e);
        }
        std::fs::write(partial.join(COMPLETE), b"").map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&partial, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        tracing::info!("bundled runtime ready");
    }
    Ok(Some(Runtime { root: dest }))
}

/// Remove earlier bundle builds' runtimes nothing uses any more: not this
/// build's (`current`), not one an installed release's venv was built from,
/// and not one whose Archipelago a season still runs (`pinned` versions; see
/// `apinstall::for_season`). Server startup only, after the pins are loaded —
/// never from a CLI command, which may run beside a server still using one.
pub fn cleanup(data_dir: &Path, current: &Path, pinned: &[String]) {
    let base = data_dir.join("runtime");
    for e in std::fs::read_dir(&base).into_iter().flatten().flatten() {
        let path = e.path();
        if path == current || referenced(data_dir, &path) {
            continue;
        }
        // Needed only when nothing else (this build, an installed release)
        // provides that version.
        let version = crate::apinstall::read_version(&path.join("archipelago"));
        if version.is_some_and(|v| pinned.contains(&v) && crate::apinstall::find(&v).is_none()) {
            continue;
        }
        tracing::info!("removing the old bundled runtime {}", path.display());
        let _ = std::fs::remove_dir_all(&path);
    }
}

/// An earlier bundle build's runtime, still unpacked, that carries
/// Archipelago `version`: its AP dir and Python.
pub fn previous(data_dir: &Path, version: &str) -> Option<(PathBuf, PathBuf)> {
    std::fs::read_dir(data_dir.join("runtime"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| Runtime { root: e.path() })
        .filter(|r| r.root.join(COMPLETE).is_file())
        .find(|r| crate::apinstall::read_version(&r.ap_dir()).as_deref() == Some(version))
        .map(|r| (r.ap_dir(), r.python()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symlinks_must_stay_inside() {
        assert!(link_inside(Path::new("python/bin/python3"), Path::new("python3.13")));
        assert!(link_inside(Path::new("python/lib/x"), Path::new("../bin/y")));
        assert!(!link_inside(Path::new("python/bin/python3"), Path::new("../../../etc/passwd")));
        assert!(!link_inside(Path::new("a"), Path::new("/etc/passwd")));
    }

    #[test]
    fn ids() {
        assert!(valid_id("0.0.1-ap0.6.8-20261026"));
        assert!(!valid_id("../x") && !valid_id(".hidden") && !valid_id(""));
    }

    #[test]
    fn slice_reads_a_window() {
        let path = std::env::temp_dir().join(format!("bifrost-slice-{}", std::process::id()));
        std::fs::write(&path, b"0123456789").unwrap();
        let mut s = Slice { f: std::fs::File::open(&path).unwrap(), start: 3, len: 4, pos: 0 };
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        assert_eq!(out, "3456");
        s.seek(SeekFrom::End(-1)).unwrap();
        out.clear();
        s.read_to_string(&mut out).unwrap();
        assert_eq!(out, "6");
        let _ = std::fs::remove_file(&path);
    }
}
