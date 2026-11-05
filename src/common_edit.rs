//! Changing the catalog of common games from the console: what a sysadmin
//! could do by editing `common/<key>/` by hand, checked the same way. Every
//! change is checked by loading the setup afterwards, and undone if it doesn't
//! load. Saving rewrites `game.toml`, so comments in it don't survive.

use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use crate::common::{self, Download, Manifest};

/// One change to the catalog at a time.
static EDITS: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    EDITS.lock().unwrap_or_else(|p| p.into_inner())
}

/// A file name an upload can take: letters, digits, `.`, `_`, `-` and `+`,
/// not starting with a dot, up to 100 characters.
pub fn file_name(upload: &str) -> Result<String, String> {
    let base = upload.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let name: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() || name.starts_with('.') || name.chars().count() > 100 {
        return Err("That file needs a plain name, up to 100 characters.".into());
    }
    Ok(name)
}

/// `dir/rel`, refusing a way out of `dir` or a link anywhere along it, for a
/// file about to be written (it needn't exist yet).
fn target(dir: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel);
    if rel.is_empty() || !p.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(format!(
            "{rel}: needs to be a path inside the setup's folder"
        ));
    }
    let mut at = dir.to_path_buf();
    for c in p.components() {
        at.push(c);
        if std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!("{rel}: links aren't followed"));
        }
    }
    Ok(at)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("bifrost-tmp");
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&tmp, bytes)
        .and_then(|_| std::fs::rename(&tmp, path))
        .map_err(|e| e.to_string())
}

/// What a file held before a change, to put it back.
struct Undo(Vec<(PathBuf, Option<Vec<u8>>)>);

impl Undo {
    fn new() -> Self {
        Undo(Vec::new())
    }
    /// Remember `path` as it is now, the first time only.
    fn keep(&mut self, path: &Path) {
        if !self.0.iter().any(|(p, _)| p == path) {
            self.0.push((path.to_path_buf(), std::fs::read(path).ok()));
        }
    }
    fn write(&mut self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        self.keep(path);
        write_atomic(path, bytes)
    }
    fn restore(self) {
        for (path, was) in self.0.into_iter().rev() {
            match was {
                Some(b) => {
                    let _ = write_atomic(&path, &b);
                }
                None => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
}

/// A setup's folder: a valid key, and a folder of its own, not a link.
fn folder(root: &Path, key: &str) -> Result<PathBuf, String> {
    let dir = root.join(key);
    if !common::valid_key(key) || !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
        return Err("not in the catalog".into());
    }
    Ok(dir)
}

fn manifest_text(m: &Manifest) -> Result<String, String> {
    toml::to_string(m).map_err(|e| format!("writing game.toml: {e}"))
}

/// Keep a change if the setup loads after it; undo it otherwise.
fn settle(root: &Path, key: &str, undo: Undo) -> Result<(), String> {
    match common::load_from(root, key) {
        Ok(_) => Ok(()),
        Err(e) => {
            undo.restore();
            Err(e)
        }
    }
}

/// Make a new setup from its name, slot and YAML.
pub fn create(root: &Path, key: &str, name: &str, slot: &str, yaml: &[u8]) -> Result<(), String> {
    let _g = lock();
    if !common::valid_key(key) {
        return Err(
            "A key is lower-case letters, digits, '.', '_' or '-', starting with a letter or digit, up to 40 characters."
                .into(),
        );
    }
    if key == "new" {
        return Err("That key is taken by the console's own page.".into());
    }
    let dir = root.join(key);
    if std::fs::symlink_metadata(&dir).is_ok() {
        return Err(format!("The catalog already has {key}."));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let m = Manifest {
        name: name.trim().to_string(),
        slot: slot.trim().to_string(),
        ..Manifest::default()
    };
    let made = manifest_text(&m)
        .and_then(|t| write_atomic(&dir.join("game.toml"), t.as_bytes()))
        .and_then(|_| write_atomic(&dir.join(m.yaml_file()), yaml))
        .and_then(|_| common::load_from(root, key).map(|_| ()));
    if made.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    made
}

/// What the console's form sets: everything but the files.
#[derive(Debug, Default, Clone)]
pub struct Fields {
    pub name: String,
    pub slot: String,
    pub address: String,
    /// Kind, label, pattern and hint; no identifier when the kind is empty.
    pub ident: (String, String, String, String),
    pub guide: String,
    pub yaml: String,
}

/// Save a setup's fields, its guide and its YAML.
pub fn save(root: &Path, key: &str, f: &Fields) -> Result<(), String> {
    let _g = lock();
    let dir = folder(root, key)?;
    let mut m = common::manifest_from(root, key).unwrap_or_default();
    m.name = f.name.trim().to_string();
    m.slot = f.slot.trim().to_string();
    m.address = f.address.trim().to_string();
    let (kind, label, pattern, hint) = &f.ident;
    m.identifier = (!kind.trim().is_empty()).then(|| common::Identifier {
        kind: kind.trim().to_string(),
        label: label.trim().to_string(),
        pattern: pattern.trim().to_string(),
        hint: hint.trim().to_string(),
    });
    let mut undo = Undo::new();
    let guide = target(&dir, m.guide_file())?;
    let yaml = target(&dir, m.yaml_file())?;
    if f.guide.trim().is_empty() {
        undo.keep(&guide);
        let _ = std::fs::remove_file(&guide);
        m.guide = None;
    } else {
        undo.write(&guide, f.guide.replace("\r\n", "\n").as_bytes())?;
    }
    let written = undo
        .write(&yaml, f.yaml.replace("\r\n", "\n").as_bytes())
        .and_then(|_| manifest_text(&m))
        .and_then(|t| undo.write(&dir.join("game.toml"), t.as_bytes()));
    if let Err(e) = written {
        undo.restore();
        return Err(e);
    }
    settle(root, key, undo)
}

/// Put an uploaded file (already on disk at `upload`, in the setup's folder)
/// in place as the setup's apworld (`label` `None`) or as a download. An
/// apworld put here is marked as uploaded (see [`common::UPLOADED`]): it's
/// never trusted like one the host's operator put in the folder.
pub fn put(
    root: &Path,
    key: &str,
    upload: &Path,
    file: &str,
    label: Option<&str>,
    by: &str,
) -> Result<(), String> {
    let _g = lock();
    let dir = folder(root, key)?;
    let mut m = common::manifest_from(root, key)?;
    let rel = match label {
        None => {
            let lower = file.to_ascii_lowercase();
            if !lower.ends_with(".apworld") || lower == "tracker.apworld" {
                return Err("An apworld is an .apworld file, and not tracker.apworld.".into());
            }
            let mut head = [0u8; 4];
            let zip = std::fs::File::open(upload)
                .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head))
                .is_ok_and(|_| crate::worlds::looks_like_apworld(&head));
            if !zip {
                return Err("That doesn't look like an .apworld (zip) file.".into());
            }
            format!("{}.apworld", &file[..file.len() - ".apworld".len()])
        }
        Some(_) => format!("client/{file}"),
    };
    let dst = target(&dir, &rel)?;
    if label.is_some() && m.download.iter().any(|d| d.file == rel) {
        return Err(format!("{file} is a download already: remove it first."));
    }
    let mut undo = Undo::new();
    undo.keep(&dst);
    let old_world = m.apworld.clone();
    match label {
        None => m.apworld = Some(rel.clone()),
        Some(l) => m.download.push(Download {
            file: rel.clone(),
            label: l.trim().to_string(),
        }),
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if let Err(e) = std::fs::rename(upload, &dst) {
        undo.restore();
        return Err(e.to_string());
    }
    let marked = if label.is_none() {
        let by: String = by.chars().filter(|c| !c.is_control()).collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        undo.write(
            &dir.join(common::UPLOADED),
            format!("{rel}\n{by}\n{now}\n").as_bytes(),
        )
    } else {
        Ok(())
    };
    if let Err(e) = marked
        .and_then(|_| manifest_text(&m))
        .and_then(|t| undo.write(&dir.join("game.toml"), t.as_bytes()))
    {
        undo.restore();
        return Err(e);
    }
    settle(root, key, undo)?;
    // A replaced apworld of another name goes.
    if label.is_none()
        && let Some(old) = old_world.filter(|o| *o != rel)
        && let Ok(p) = target(&dir, &old)
    {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

/// Drop the setup's apworld, or the download of that file, with its file.
pub fn remove(root: &Path, key: &str, download: Option<&str>) -> Result<(), String> {
    let _g = lock();
    let dir = folder(root, key)?;
    let mut m = common::manifest_from(root, key)?;
    let rel = match download {
        None => m.apworld.take(),
        Some(file) => m
            .download
            .iter()
            .position(|d| d.file == file)
            .map(|n| m.download.remove(n).file),
    }
    .ok_or("That isn't there to remove: reload the page.")?;
    let mut undo = Undo::new();
    manifest_text(&m).and_then(|t| undo.write(&dir.join("game.toml"), t.as_bytes()))?;
    settle(root, key, undo)?;
    if let Ok(p) = target(&dir, &rel)
        && !m.download.iter().any(|d| d.file == rel)
    {
        let _ = std::fs::remove_file(p);
    }
    if download.is_none() {
        let _ = std::fs::remove_file(dir.join(common::UPLOADED));
    }
    Ok(())
}

/// Delete a setup's folder. The caller checks no season still uses it.
pub fn delete(root: &Path, key: &str) -> Result<(), String> {
    let _g = lock();
    if !common::valid_key(key) {
        return Err("not in the catalog".into());
    }
    let dir = root.join(key);
    if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
        return Err("not in the catalog".into());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())
}

/// A temporary file in the setup's folder for an upload to stream into, so
/// putting it in place is a rename.
/// Uploads a crash or a dropped connection left behind are cleared after a day.
pub fn upload_path(root: &Path, key: &str) -> Result<PathBuf, String> {
    let dir = folder(root, key)?;
    let day = std::time::Duration::from_secs(24 * 3600);
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let stale = e.file_name().to_string_lossy().starts_with(".upload-")
            && e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|a| a > day));
        if stale {
            let _ = std::fs::remove_file(e.path());
        }
    }
    Ok(dir.join(format!(".upload-{}", crate::auth::new_token())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bifrost-cedit-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn fields() -> Fields {
        Fields {
            name: "Minecraft".into(),
            slot: "Vanilla".into(),
            address: "mc.example:25565".into(),
            ident: (
                "minecraft".into(),
                "Minecraft username".into(),
                "[A-Za-z0-9_]{3,16}".into(),
                String::new(),
            ),
            guide: "Join {address}\r\n".into(),
            yaml: "name: Vanilla\ngame: Minecraft\n".into(),
        }
    }

    #[test]
    fn a_setup_is_made_and_edited() {
        let r = root("edit");
        create(&r, "vanilla", "Minecraft", "Vanilla", b"name: Vanilla\n").unwrap();
        assert!(
            create(&r, "vanilla", "X", "Y", b"name: Y\n").is_err(),
            "taken"
        );
        save(&r, "vanilla", &fields()).unwrap();
        let s = common::load_from(&r, "vanilla").unwrap();
        assert_eq!(s.address, "mc.example:25565");
        assert_eq!(s.guide, "Join {address}\n");
        assert_eq!(s.identifier.unwrap().kind, "minecraft");

        let up = upload_path(&r, "vanilla").unwrap();
        std::fs::write(&up, "not a zip").unwrap();
        assert!(put(&r, "vanilla", &up, "minecraft.apworld", None, "root").is_err());
        let up = upload_path(&r, "vanilla").unwrap();
        std::fs::write(&up, "PK\x03\x04old").unwrap();
        put(&r, "vanilla", &up, "old.apworld", None, "root").unwrap();
        assert!(
            common::uploaded_from(&r, "vanilla", "old.apworld"),
            "marked as uploaded"
        );
        // Replacing it with another name drops the old file.
        let up = upload_path(&r, "vanilla").unwrap();
        std::fs::write(&up, "PK\x03\x04zip").unwrap();
        put(&r, "vanilla", &up, "minecraft.APWORLD", None, "root").unwrap();
        assert!(!r.join("vanilla/old.apworld").exists());
        assert!(common::uploaded_from(&r, "vanilla", "minecraft.apworld"));
        let up = upload_path(&r, "vanilla").unwrap();
        std::fs::write(&up, "jar").unwrap();
        put(&r, "vanilla", &up, "mod.jar", Some("The mod"), "root").unwrap();
        let s = common::load_from(&r, "vanilla").unwrap();
        assert_eq!(s.apworld.as_deref(), Some("minecraft.apworld"));
        assert_eq!(s.downloads[0].file, "client/mod.jar");

        assert!(remove(&r, "vanilla", Some("client/other.jar")).is_err());
        remove(&r, "vanilla", Some("client/mod.jar")).unwrap();
        remove(&r, "vanilla", None).unwrap();
        assert!(!r.join("vanilla").join(common::UPLOADED).exists());
        let s = common::load_from(&r, "vanilla").unwrap();
        assert!(s.apworld.is_none() && s.downloads.is_empty());
        assert!(!r.join("vanilla/client/mod.jar").exists());
        assert!(!r.join("vanilla/minecraft.apworld").exists());
        delete(&r, "vanilla").unwrap();
        assert!(!r.join("vanilla").exists());
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn a_change_that_breaks_a_setup_is_undone() {
        let r = root("undo");
        create(&r, "vanilla", "Minecraft", "Vanilla", b"name: Vanilla\n").unwrap();
        save(&r, "vanilla", &fields()).unwrap();
        let before = std::fs::read_to_string(r.join("vanilla/game.toml")).unwrap();
        // The YAML would name another slot.
        let bad = Fields {
            yaml: "name: Other\n".into(),
            ..fields()
        };
        assert!(
            save(&r, "vanilla", &bad)
                .unwrap_err()
                .contains("not the slot")
        );
        let bad = Fields {
            ident: ("minecraft".into(), "x".into(), "(".into(), String::new()),
            ..fields()
        };
        assert!(save(&r, "vanilla", &bad).is_err());
        assert_eq!(
            std::fs::read_to_string(r.join("vanilla/game.toml")).unwrap(),
            before
        );
        assert_eq!(
            std::fs::read_to_string(r.join("vanilla/player.yaml")).unwrap(),
            "name: Vanilla\ngame: Minecraft\n"
        );
        // A setup that wouldn't load isn't made.
        assert!(create(&r, "bad", "X", "{player}", b"name: X\n").is_err());
        assert!(!r.join("bad").exists());
        let up = upload_path(&r, "vanilla").unwrap();
        std::fs::write(&up, "PK\x03\x04").unwrap();
        assert!(put(&r, "vanilla", &up, "Tracker.apworld", None, "root").is_err());
        // A put that would break the setup leaves what was there.
        let up = upload_path(&r, "vanilla").unwrap();
        std::fs::write(&up, "PK\x03\x04good").unwrap();
        put(&r, "vanilla", &up, "w.apworld", None, "root").unwrap();
        std::fs::write(r.join("vanilla/player.yaml"), "name: Other\n").unwrap();
        let up = upload_path(&r, "vanilla").unwrap();
        std::fs::write(&up, "PK\x03\x04new").unwrap();
        assert!(put(&r, "vanilla", &up, "w.apworld", None, "root").is_err());
        assert_eq!(
            std::fs::read(r.join("vanilla/w.apworld")).unwrap(),
            b"PK\x03\x04good"
        );
        // Saving to a setup that isn't there, or through a link, is refused.
        assert!(save(&r, "missing", &fields()).is_err());
        assert!(!r.join("missing").exists());
        assert!(save(&r, "Bad", &fields()).is_err());
        std::os::unix::fs::symlink(r.join("vanilla"), r.join("linked")).unwrap();
        assert!(save(&r, "linked", &fields()).is_err());
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn upload_names_are_plain() {
        assert_eq!(file_name("C:\\Users\\x\\My Mod.jar").unwrap(), "My_Mod.jar");
        assert_eq!(file_name("../../etc/passwd").unwrap(), "passwd");
        assert!(file_name(".hidden").is_err());
        assert!(file_name("").is_err());
    }
}
