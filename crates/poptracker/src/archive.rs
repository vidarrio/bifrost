//! Extracting a pack zip, and finding the pack inside it.

use crate::{Error, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Cap on total bytes written when extracting a pack, so a small "zip bomb"
/// can't fill the host's disk. Real packs are a few MB; this is generous.
const MAX_EXTRACT_BYTES: u64 = 512 * 1024 * 1024;

/// Caps on a pack's files and directories together, and on how deep they go, for
/// the same reason (they also bound how long extracting can take).
const MAX_EXTRACT_FILES: usize = 100_000;
const MAX_EXTRACT_DEPTH: usize = 32;

/// Extract a pack zip into `dest` (with a zip-slip guard). Used both for the
/// temp-dir load path and to install an uploaded pack to permanent storage.
pub fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    extract_zip_capped(zip_path, dest, MAX_EXTRACT_BYTES)
}

fn extract_zip_capped(zip_path: &Path, dest: &Path, cap: u64) -> Result<()> {
    use std::io::Read;
    let file = std::fs::File::open(zip_path).map_err(|e| Error::Io(e.to_string()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| Error::Pack(format!("zip: {e}")))?;
    if zip.len() > MAX_EXTRACT_FILES {
        return Err(Error::Pack("pack has too many files".into()));
    }
    let mut budget = cap;
    let mut made = HashSet::new();
    for i in 0..zip.len() {
        let mut f = zip
            .by_index(i)
            .map_err(|e| Error::Pack(format!("zip entry {i}: {e}")))?;
        // enclosed_name() rejects absolute paths and `..` traversal.
        let Some(rel) = f.enclosed_name() else {
            continue;
        };
        if rel.components().count() > MAX_EXTRACT_DEPTH {
            return Err(Error::Pack("pack's folders are too deep".into()));
        }
        // The entry itself and every folder it brings: a zip needn't list them.
        for p in rel.ancestors().filter(|p| !p.as_os_str().is_empty()) {
            // A hash: a pack's long names shouldn't all be kept in memory.
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(p, &mut h);
            if !made.insert(std::hash::Hasher::finish(&h)) {
                break;
            }
            if made.len() > MAX_EXTRACT_FILES {
                return Err(Error::Pack("pack has too many files".into()));
            }
        }
        let out = dest.join(rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| Error::Io(e.to_string()))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io(e.to_string()))?;
        }
        let mut w = std::fs::File::create(&out).map_err(|e| Error::Io(e.to_string()))?;
        // Copy at most `budget`+1 bytes: the declared size is attacker-controlled,
        // so enforce the real decompressed byte count, not the header's claim.
        let mut limited = (&mut f).take(budget + 1);
        let n = std::io::copy(&mut limited, &mut w).map_err(|e| Error::Io(e.to_string()))?;
        if n > budget {
            return Err(Error::Pack(
                "pack is too large when extracted (possible zip bomb)".into(),
            ));
        }
        budget -= n;
    }
    Ok(())
}

/// Find the pack root under `base`: `base` itself if it has `scripts/init.lua`,
/// else the first immediate subdirectory that does. Public so a caller that
/// extracted a pack with [`extract_zip`] can locate the root to load.
pub fn pack_root(base: &Path) -> Result<PathBuf> {
    find_pack_root(base)
}

/// Find the pack root under `base`: `base` itself if it has `scripts/init.lua`,
/// else the first immediate subdirectory that does.
pub(crate) fn find_pack_root(base: &Path) -> Result<PathBuf> {
    if base.join("scripts/init.lua").is_file() {
        return Ok(base.to_path_buf());
    }
    if let Ok(rd) = std::fs::read_dir(base) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && p.join("scripts/init.lua").is_file() {
                return Ok(p);
            }
        }
    }
    Err(Error::Pack("no scripts/init.lua found in zip".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_zip_enforces_byte_cap() {
        // A zip whose extracted contents exceed the cap is rejected (zip bomb).
        let tmp = tempfile::TempDir::new().unwrap();
        let zip_path = tmp.path().join("big.zip");
        {
            let mut zw = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
            zw.start_file("big.bin", zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut zw, &vec![0u8; 4096]).unwrap();
            zw.finish().unwrap();
        }
        let dest = tmp.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        // Cap below the 4 KiB payload → rejected; a generous cap → fine.
        assert!(extract_zip_capped(&zip_path, &dest, 1024).is_err());
        assert!(extract_zip_capped(&zip_path, &dest, 1 << 20).is_ok());
    }
}
