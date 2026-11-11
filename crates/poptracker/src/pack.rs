//! A pack on disk: loading, its files and its manifest's variants.

use crate::archive::{extract_zip, find_pack_root};
use crate::json::parse_pack_json;
use crate::rules::leading_int;
use crate::{Error, Result};
use std::path::{Path, PathBuf};

/// A loaded pack. Either a directory on disk or a `.zip` extracted to a temp dir
/// (kept alive by `_tmp` for the pack's lifetime).
pub struct Pack {
    pub(crate) dir: PathBuf,
    /// The variant in use (see [`pick_variant`]); empty if the pack has none.
    pub(crate) variant: String,
    /// Whether a location that's both glitched and inspectable reads as glitched
    /// unless it says otherwise: the old default, for packs that target it.
    pub(crate) scoutable_glitched: bool,
    _tmp: Option<tempfile::TempDir>,
}
impl Pack {
    /// Load a pack from a directory **or** a `.zip` file (auto-detected).
    pub fn load(path: impl AsRef<Path>) -> Result<Pack> {
        let path = path.as_ref();
        if path.is_dir() {
            Self::from_dir(path.to_path_buf(), None)
        } else {
            Self::load_zip(path)
        }
    }

    /// Extract an untrusted pack zip to a temp dir (with a zip-slip guard) and
    /// locate the pack root (the dir containing `scripts/init.lua`, at the zip
    /// root or one level down — packs are often zipped inside a top folder).
    pub fn load_zip(path: &Path) -> Result<Pack> {
        let tmp = tempfile::TempDir::new().map_err(|e| Error::Io(e.to_string()))?;
        extract_zip(path, tmp.path())?;
        let root = find_pack_root(tmp.path())?;
        Self::from_dir(root, Some(tmp))
    }

    fn from_dir(dir: PathBuf, tmp: Option<tempfile::TempDir>) -> Result<Pack> {
        if !dir.join("scripts/init.lua").is_file() {
            return Err(Error::Pack("missing scripts/init.lua".into()));
        }
        let manifest = read_manifest(&dir);
        let variant = pick_variant(&dir, &manifest);
        let scoutable_glitched = targets_old_scoutable_default(&manifest);
        Ok(Pack {
            dir,
            variant,
            scoutable_glitched,
            _tmp: tmp,
        })
    }

    /// The pack's root directory on disk.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The variant in use; empty if the pack has none.
    pub fn variant(&self) -> &str {
        &self.variant
    }

    /// Read a file from inside the pack (pack-relative), e.g. a map image.
    /// Guards against path traversal.
    pub fn asset(&self, rel: &str) -> Option<Vec<u8>> {
        std::fs::read(find_in_pack(&self.dir, &self.variant, rel)?).ok()
    }
}

/// Resolve a pack-relative path, rejecting traversal (`..`), absolute paths, and
/// Windows separators/roots. This is the guard that keeps every pack-controlled
/// filename — asset requests AND the pack's own Lua (`ScriptHost:LoadScript`,
/// `Tracker:AddItems`/`AddLocations`/`AddMaps`/`AddLayouts`) — inside the pack
/// directory, so a malicious pack can't read arbitrary host files.
fn pack_join(dir: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty()
        || rel.contains("..")
        || rel.starts_with('/')
        || rel.starts_with('\\')
        || rel.contains(':')
    {
        return None;
    }
    Some(dir.join(rel))
}

/// Find a pack file as PopTracker does: in the variant's folder, then the pack's
/// root (a path may start with `/`, meaning the same).
pub(crate) fn find_in_pack(dir: &Path, variant: &str, rel: &str) -> Option<PathBuf> {
    let rel = rel.trim_start_matches('/');
    if !variant.is_empty()
        && let Some(p) = pack_join(dir, &format!("{variant}/{rel}"))
        && p.is_file()
    {
        return Some(p);
    }
    pack_join(dir, rel)
}

/// Read a file of an unpacked pack without loading it, given its variant. Serving
/// images takes this path: it parses nothing a pack wrote, so it's safe in the
/// server's own process, unlike [`Pack::load`], which belongs in a worker.
pub fn read_asset(dir: &Path, variant: &str, rel: &str) -> Option<Vec<u8>> {
    std::fs::read(find_in_pack(dir, variant, rel)?).ok()
}

/// The most of a manifest that's read. Parsing one recurses, so a hostile pack
/// can overflow the stack: that's why only a worker may call [`Pack::load`].
const MAX_MANIFEST: u64 = 1 << 20;

/// The pack's manifest, if it has one that parses.
fn read_manifest(dir: &Path) -> serde_json::Value {
    let path = dir.join("manifest.json");
    let txt = match std::fs::metadata(&path) {
        Ok(m) if m.len() <= MAX_MANIFEST => std::fs::read_to_string(&path).unwrap_or_default(),
        _ => String::new(),
    };
    parse_pack_json(txt.trim_start_matches('\u{feff}')).unwrap_or_default()
}

/// PopTracker's default for `inspectable_sequence_break`: on for a pack that
/// targets 0.31.0 or older.
fn targets_old_scoutable_default(manifest: &serde_json::Value) -> bool {
    let version: Vec<u64> = manifest
        .get("target_poptracker_version")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .split('.')
        .map(|p| leading_int(p).max(0) as u64)
        .chain([0, 0, 0])
        .take(3)
        .collect();
    let version = (version[0], version[1], version[2]);
    version > (0, 0, 0) && version <= (0, 31, 0)
}

/// Which of a pack's variants to run: there's no one here to choose, so an
/// Archipelago one with a map rather than items only, `standard` if that fits,
/// else the first by name. Empty if the manifest names none.
fn pick_variant(dir: &Path, manifest: &serde_json::Value) -> String {
    let Some(variants) = manifest.get("variants").and_then(|v| v.as_object()) else {
        return String::new();
    };
    let items_only = |uid: &str, v: &serde_json::Value| {
        let name = v.get("display_name").and_then(|n| n.as_str()).unwrap_or("");
        let plain = |s: &str| s.to_lowercase().replace([' ', '_', '-'], "");
        plain(uid).contains("itemsonly") || plain(name).contains("itemsonly")
    };
    let ap = |v: &serde_json::Value| {
        v.get("flags")
            .and_then(|f| f.as_array())
            .is_some_and(|f| f.iter().any(|x| x.as_str() == Some("ap")))
    };
    let usable = |uid: &&String| pack_join(dir, uid).is_some();
    let score = |uid: &String, v: &serde_json::Value| {
        (ap(v), !items_only(uid, v), uid.as_str() == "standard")
    };
    let mut best: Option<(&String, (bool, bool, bool))> = None;
    // Names in order, so an earlier one wins a tie.
    for (uid, v) in variants.iter().filter(|(uid, _)| usable(uid)) {
        let sc = score(uid, v);
        if best.as_ref().is_none_or(|(_, b)| sc > *b) {
            best = Some((uid, sc));
        }
    }
    best.map(|(uid, _)| uid.clone()).unwrap_or_default()
}

/// A `require` module name as a pack-relative path without the extension
/// ("logic.base" or "logic/base" → "logic/base"). Names with empty parts, or
/// anything `pack_join` would refuse, aren't modules.
pub(crate) fn module_path(name: &str) -> Option<String> {
    let rel = name.replace('.', "/");
    if rel.split('/').any(|p| p.is_empty()) {
        return None;
    }
    pack_join(Path::new(""), &rel).map(|_| rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_join_blocks_traversal_and_absolute() {
        let d = Path::new("/packs/slot");
        // legitimate pack-relative paths resolve inside the dir
        assert_eq!(
            pack_join(d, "images/map.png"),
            Some(d.join("images/map.png"))
        );
        assert_eq!(
            pack_join(d, "scripts/logic.lua"),
            Some(d.join("scripts/logic.lua"))
        );
        // traversal / absolute / windowsy / empty are refused
        for bad in [
            "..",
            "../etc/passwd",
            "a/../../b",
            "/etc/passwd",
            "\\x",
            "C:/x",
            "",
        ] {
            assert!(pack_join(d, bad).is_none(), "{bad:?} must be refused");
        }
    }
}
