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

/// A new temporary file beside `path`. Its name is unique to this process and
/// write, and it's created fresh (never opened if something is already there),
/// so concurrent writes of one file can't write into each other's.
fn create_temp(path: &Path, mode: u32) -> io::Result<(PathBuf, std::fs::File)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
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
    fn a_missing_folder_is_an_error() {
        let d = dir("missing");
        assert!(write(&d.as_path().join("nope/file"), b"x").is_err());
    }
}
