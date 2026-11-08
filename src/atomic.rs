//! Replacing a file in one step, so whoever reads it sees the old file or the
//! whole new one, never part of either.

use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Write `bytes` to `path` in one step. It's written to a temporary file
/// beside it (a hidden name ending `.tmp`, unique to this write), flushed to
/// disk and renamed over `path`; the temporary file is removed if any of that
/// fails. A new file gets the usual permissions, as `std::fs::write` gives
/// one; the parent directory must exist.
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_mode(path, bytes, 0o666)
}

/// [`write`], readable by this user only.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_mode(path, bytes, 0o600)
}

/// [`write`] off the async runtime's threads.
pub async fn write_async(path: PathBuf, bytes: Vec<u8>) -> io::Result<()> {
    tokio::task::spawn_blocking(move || write(&path, &bytes))
        .await
        .map_err(io::Error::other)?
}

fn write_mode(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    sweep(path);
    let (tmp, mut f) = create_temp(path, mode)?;
    let written = f
        .write_all(bytes)
        .and_then(|()| f.sync_all())
        .and_then(|()| std::fs::rename(&tmp, path));
    drop(f);
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written?;
    // The rename itself on disk too; not every filesystem lets a directory be
    // synced, and the file is in place either way.
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
    }
    Ok(())
}

/// Remove the temporary files of `path` that a crash left behind: any over an
/// hour old (a write takes far less). Best effort.
fn sweep(path: &Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let prefix = format!(".{}.", name.to_string_lossy());
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let hour = std::time::Duration::from_secs(3600);
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let n = e.file_name();
        let n = n.to_string_lossy();
        let old = || {
            e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|a| a > hour))
        };
        // Only this helper's own names (`<pid>-<n>.tmp` after the prefix), not
        // a longer file name's temps.
        let ours = n
            .strip_prefix(&prefix)
            .and_then(|r| r.strip_suffix(".tmp"))
            .is_some_and(|r| {
                r.split_once('-').is_some_and(|(a, b)| {
                    [a, b]
                        .iter()
                        .all(|x| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit()))
                })
            });
        if ours && old() {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Numbers the temporary files, unique within this process.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A new temporary file beside `path`. Its name is unique to this process and
/// write, and it's created fresh (never opened if something is already there),
/// so concurrent writes of one file can't write into each other's.
fn create_temp(path: &Path, mode: u32) -> io::Result<(PathBuf, std::fs::File)> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?
        .to_string_lossy();
    let mut last = None;
    // A name already taken is one a crash left behind (its process id since
    // reused): try the next.
    for _ in 0..16 {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_file_name(format!(".{name}.{}-{n}.tmp", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)
        {
            Ok(f) => return Ok((tmp, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no temporary name")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A fresh, empty folder for one test.
    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bifrost-atomic-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn writes_and_replaces_leaving_no_temp() {
        let d = dir("replace");
        let p = d.as_path().join("pins.json");
        write(&p, b"one").unwrap();
        write(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert_eq!(entries(d.as_path()), ["pins.json"]);
    }

    #[test]
    fn private_is_owner_only() {
        let d = dir("private");
        let p = d.as_path().join("filter.bpf");
        write_private(&p, b"x").unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_failed_rename_leaves_the_old_file_and_no_temp() {
        let d = dir("rename");
        // A directory where the file should go: the rename fails.
        let p = d.as_path().join("game.toml");
        std::fs::create_dir(&p).unwrap();
        std::fs::write(p.join("inside"), b"kept").unwrap();
        assert!(write(&p, b"new").is_err());
        assert_eq!(entries(d.as_path()), ["game.toml"]);
        assert_eq!(std::fs::read(p.join("inside")).unwrap(), b"kept");
    }

    #[test]
    fn sweeps_old_leftovers_only() {
        let d = dir("sweep");
        let p = d.join("f");
        let old = d.join(".f.1-0.tmp");
        let new = d.join(".f.1-1.tmp");
        let other = d.join(".g.1-0.tmp");
        let longer = d.join(".f.x.1-0.tmp");
        for x in [&old, &new, &other, &longer] {
            std::fs::write(x, b"left").unwrap();
        }
        let day_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(86400);
        for x in [&old, &other, &longer] {
            std::fs::File::options()
                .write(true)
                .open(x)
                .unwrap()
                .set_modified(day_ago)
                .unwrap();
        }
        write(&p, b"ok").unwrap();
        assert_eq!(
            entries(&d),
            [".f.1-1.tmp", ".f.x.1-0.tmp", ".g.1-0.tmp", "f"]
        );
    }

    #[test]
    fn steps_past_a_taken_name() {
        let d = dir("taken");
        let p = d.join("f");
        // The next names this process would try, as if a crash left them.
        // (Tests running alongside take numbers too, so the write may land
        // past them without retrying; it must not open one either way.)
        let start = NEXT.load(Ordering::Relaxed);
        let taken: Vec<PathBuf> = (start..start + 15)
            .map(|n| d.join(format!(".f.{}-{n}.tmp", std::process::id())))
            .collect();
        for t in &taken {
            std::fs::write(t, b"left").unwrap();
        }
        write(&p, b"ok").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"ok");
        for t in &taken {
            assert_eq!(std::fs::read(t).unwrap(), b"left");
        }
    }

    #[tokio::test]
    async fn writes_async() {
        let d = dir("async");
        let p = d.join("host.yaml");
        write_async(p.clone(), b"yaml".to_vec()).await.unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"yaml");
        assert_eq!(entries(&d), ["host.yaml"]);
    }

    #[test]
    fn a_missing_folder_is_an_error() {
        let d = dir("missing");
        assert!(write(&d.as_path().join("nope/file"), b"x").is_err());
    }
}
