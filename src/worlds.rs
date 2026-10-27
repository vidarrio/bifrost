//! How custom apworlds get into a season. Either from the
//! curated catalog — sysadmin-vetted, usable anywhere — or as an upload,
//! which is installed right away when the instance auto-approves uploads, and
//! otherwise waits in the season's `pending/` (where AP never loads it) until
//! a season moderator or a sysadmin approves it. Signups whose YAML needs a
//! pending world are held, then checked once it's approved.
//!
//! Every install goes through [`ap_adapter::install_world`], which pins the
//! hash; nothing here bypasses the pin check that guards generation.

use std::io::Read;
use std::path::PathBuf;

use crate::ap_adapter::{self, Pin};
use crate::config::{self, SeasonDirs};
use crate::db::{Approval, CatalogWorld, Db, WorldRequest};

/// File names an upload may not take: worlds Bifrost ships (Universal
/// Tracker, the common game), which are trusted by content.
const RESERVED: &[&str] = &["tracker", "minecraft"];

/// The name an uploaded apworld installs as, from the uploaded file's name.
pub fn world_file(upload_name: &str) -> Result<String, &'static str> {
    let stem = std::path::Path::new(upload_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("world");
    let base = crate::slugify(stem);
    if RESERVED.contains(&base.to_lowercase().as_str()) {
        return Err("That apworld name is reserved — rename the file and try again.");
    }
    Ok(format!("{base}.apworld"))
}

/// `.apworld`s are zips; a quick magic-byte check rejects obvious mistakes.
pub fn looks_like_apworld(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && &bytes[..2] == b"PK"
}

/// What an apworld says about itself (`<world>/archipelago.json`; worlds
/// built before AP 0.6 have none).
#[derive(Debug, Default, PartialEq)]
pub struct Manifest {
    pub game: String,
    pub version: String,
}

pub fn manifest(bytes: &[u8]) -> Manifest {
    let read = || -> Option<Manifest> {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
        let name = zip
            .file_names()
            .filter_map(Result::ok)
            .find(|n| n.ends_with("/archipelago.json") && n.matches('/').count() == 1)?
            .into_owned();
        let mut json = String::new();
        zip.by_name(&name).ok()?.take(64 * 1024).read_to_string(&mut json).ok()?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        let s = |k: &str| match &v[k] {
            serde_json::Value::String(s) => s.chars().take(80).collect(),
            serde_json::Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        Some(Manifest { game: s("game"), version: s("world_version") })
    };
    read().unwrap_or_default()
}

pub fn catalog_file(sha256: &str) -> PathBuf {
    config::get().catalog_dir().join(format!("{sha256}.apworld"))
}

/// Where an upload ended up.
#[derive(Debug, PartialEq)]
pub enum Placed {
    /// Installed and usable now (`catalog`: it matched a catalog entry).
    Installed { catalog: bool },
    /// Waiting for approval.
    Pending,
}

/// Refuse to swap a world out from under the YAMLs that use it: a different
/// file under an installed name has to be removed by a moderator first.
fn installed_conflict(dirs: &SeasonDirs, file: &str, sha256: &str) -> Result<bool, String> {
    match ap_adapter::world_pins(dirs).get(file) {
        Some(p) if p.sha256 == sha256 => Ok(true),
        Some(_) => Err(format!(
            "A different {file} is already installed this season — ask a moderator to remove it first."
        )),
        None => Ok(false),
    }
}

async fn install(dirs: SeasonDirs, file: String, bytes: Vec<u8>, pin: Pin) -> Result<(), String> {
    tokio::task::spawn_blocking(move || ap_adapter::install_world(&dirs, &file, &bytes, pin))
        .await
        .map_err(|e| e.to_string())?
}

/// Place an uploaded apworld in `season`: installed if it's in the catalog or
/// the instance auto-approves, otherwise pending.
pub async fn place_upload(
    db: &Db,
    season: i64,
    file: &str,
    bytes: Vec<u8>,
    uploader: &str,
) -> Result<Placed, String> {
    let dirs = config::get().season(season);
    let sha = ap_adapter::sha256_hex(&bytes);
    if installed_conflict(&dirs, file, &sha)? {
        return Ok(Placed::Installed { catalog: false });
    }
    let base = Pin {
        uploaded_by: uploader.to_string(),
        uploaded_at: crate::auth::now(),
        ..Pin::default()
    };
    if let Some(cat) = db.catalog_world(&sha).await {
        let pin = Pin { catalog: true, approved_by: cat.added_by, ..base };
        install(dirs, file.to_string(), bytes, pin).await?;
        return Ok(Placed::Installed { catalog: true });
    }
    if let Some(req) = db.world_request(season, file).await {
        return if req.sha256 == sha {
            Ok(Placed::Pending)
        } else {
            Err(format!("Another version of {file} is already waiting for approval."))
        };
    }
    match db.approval().await {
        Approval::Auto => {
            install(dirs, file.to_string(), bytes, base).await?;
            Ok(Placed::Installed { catalog: false })
        }
        Approval::Manual => {
            let game = manifest(&bytes).game;
            tokio::fs::create_dir_all(&dirs.pending)
                .await
                .map_err(|e| format!("pending dir: {e}"))?;
            tokio::fs::write(dirs.pending.join(file), &bytes)
                .await
                .map_err(|e| format!("storing the upload: {e}"))?;
            let req = WorldRequest {
                file: file.to_string(),
                sha256: sha,
                game,
                uploaded_by: uploader.to_string(),
                uploaded_at: base.uploaded_at,
            };
            db.add_world_request(season, &req).await.map_err(|e| e.to_string())?;
            Ok(Placed::Pending)
        }
    }
}

/// Install a catalog world into `season`. Returns the file name it took.
pub async fn install_from_catalog(db: &Db, season: i64, sha256: &str, by: &str) -> Result<String, String> {
    let cat = db.catalog_world(sha256).await.ok_or("That world isn't in the catalog.")?;
    let bytes = tokio::fs::read(catalog_file(&cat.sha256))
        .await
        .map_err(|e| format!("reading the catalog copy: {e}"))?;
    if ap_adapter::sha256_hex(&bytes) != cat.sha256 {
        return Err("The catalog copy doesn't match its hash — ask a sysadmin to re-add it.".into());
    }
    let dirs = config::get().season(season);
    if installed_conflict(&dirs, &cat.file, &cat.sha256)? {
        return Ok(cat.file);
    }
    let pin = Pin {
        uploaded_by: by.to_string(),
        uploaded_at: crate::auth::now(),
        approved_by: cat.added_by.clone(),
        catalog: true,
        ..Pin::default()
    };
    install(dirs, cat.file.clone(), bytes, pin).await?;
    Ok(cat.file)
}

/// Approve a pending upload: install it, then check the signups held for it
/// (in the background — each check can take a while).
pub async fn approve(db: &Db, season: i64, file: &str, by: &str) -> Result<WorldRequest, String> {
    let dirs = config::get().season(season);
    let req = db.world_request(season, file).await.ok_or("Nothing is waiting under that name.")?;
    let path = dirs.pending.join(&req.file);
    let bytes = tokio::fs::read(&path).await.map_err(|e| format!("reading the upload: {e}"))?;
    if ap_adapter::sha256_hex(&bytes) != req.sha256 {
        return Err(format!("{file} changed since it was uploaded — reject it and ask for a new upload."));
    }
    if !installed_conflict(&dirs, &req.file, &req.sha256)? {
        let pin = Pin {
            uploaded_by: req.uploaded_by.clone(),
            uploaded_at: req.uploaded_at,
            approved_by: by.to_string(),
            ..Pin::default()
        };
        install(dirs, req.file.clone(), bytes, pin).await?;
    }
    let _ = tokio::fs::remove_file(&path).await;
    db.take_world_request(season, file).await;
    let (db, file) = (db.clone(), req.file.clone());
    tokio::spawn(async move { recheck_held(&db, season, &file).await });
    Ok(req)
}

/// Reject a pending upload; its held signups say so until they're removed.
pub async fn reject(db: &Db, season: i64, file: &str, by: &str) -> Result<(), String> {
    let req = db.take_world_request(season, file).await.ok_or("Nothing is waiting under that name.")?;
    let _ = tokio::fs::remove_file(config::get().season(season).pending.join(&req.file)).await;
    let why = format!("{} was rejected by {by} — remove this signup", req.file);
    for (id, _) in db.held_submissions(season, &req.file).await {
        let _ = db.set_held_err(id, &why).await;
    }
    Ok(())
}

/// Check every signup held for `file` now that it's installed: those that
/// pass go ahead (with the name and game the check read); the rest keep
/// the reason.
pub async fn recheck_held(db: &Db, season: i64, file: &str) {
    let dirs = config::get().season(season);
    for (id, yaml_path) in db.held_submissions(season, file).await {
        let outcome = match tokio::fs::read_to_string(&yaml_path).await {
            Err(e) => Err(format!("the stored YAML is gone ({e})")),
            Ok(yaml) => match check_with_retries(&dirs, &yaml).await {
                // No verdict: `stale_holds` picks it up again.
                Err(e) => Err(format!("couldn't be checked ({e}) — it's tried again before generating")),
                Ok(r) if !r.ok => Err(format!("its YAML didn't pass: {}", r.errors.join("; "))),
                Ok(r) => r
                    .players
                    .first()
                    .map(|p| (p.name.clone(), p.game.clone().unwrap_or_else(|| "?".into())))
                    .ok_or_else(|| "couldn't read a player from its YAML".to_string()),
            },
        };
        let res = match outcome {
            Ok((name, game)) => db
                .release_submission(id, &name, &game)
                .await
                .map_err(|_| format!("the slot name {name} is already signed up")),
            Err(e) => Err(e),
        };
        if let Err(e) = res {
            tracing::info!("held signup {id} in season {season} still held: {e}");
            let _ = db.set_held_err(id, &e).await;
        }
    }
}

/// A YAML check, retried a few times when the checker couldn't run it (busy
/// queue, timeout) — those aren't verdicts on the YAML.
async fn check_with_retries(dirs: &config::SeasonDirs, yaml: &str) -> Result<crate::checker::CheckResult, String> {
    let mut wait = std::time::Duration::from_secs(15);
    for _ in 0..3 {
        match crate::checker::check_yaml(dirs, yaml).await {
            Ok(r) => return Ok(r),
            Err(e) => tracing::info!("re-checking a held signup: {e}; retrying in {}s", wait.as_secs()),
        }
        tokio::time::sleep(wait).await;
        wait *= 2;
    }
    crate::checker::check_yaml(dirs, yaml).await
}

/// Finish re-checks that were cut short (a restart mid-loop, a check that
/// couldn't run): held signups whose world is installed and no longer waits
/// for approval. Run at startup and before generating.
pub async fn recheck_stale(db: &Db, season: i64) {
    let dirs = config::get().season(season);
    for file in db.stale_holds(season).await {
        if dirs.worlds.join(&file).is_file() {
            recheck_held(db, season, &file).await;
        }
    }
}

/// Add an apworld to the catalog (or re-describe it). `game`/`version`
/// default to what its manifest says.
pub async fn catalog_add(
    db: &Db,
    file: &str,
    bytes: &[u8],
    game: &str,
    version: &str,
    note: &str,
    by: &str,
) -> Result<CatalogWorld, String> {
    let sha = ap_adapter::sha256_hex(bytes);
    let m = manifest(bytes);
    let pick = |typed: &str, read: String| {
        let t = typed.trim();
        if t.is_empty() { read } else { t.chars().take(80).collect() }
    };
    let w = CatalogWorld {
        file: file.to_string(),
        game: pick(game, m.game),
        version: pick(version, m.version),
        note: note.trim().chars().take(200).collect(),
        added_by: by.to_string(),
        added_at: crate::auth::now(),
        sha256: sha,
    };
    let path = catalog_file(&w.sha256);
    let tmp = path.with_extension("apworld.tmp");
    let store = async {
        tokio::fs::create_dir_all(config::get().catalog_dir()).await?;
        tokio::fs::write(&tmp, bytes).await?;
        tokio::fs::rename(&tmp, &path).await
    };
    if let Err(e) = store.await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(format!("storing the catalog copy: {e}"));
    }
    db.catalog_add(&w).await.map_err(|e| e.to_string())?;
    Ok(w)
}

/// Add a world already installed in a season to the catalog.
pub async fn catalog_add_installed(db: &Db, season: i64, file: &str, by: &str) -> Result<CatalogWorld, String> {
    let dirs = config::get().season(season);
    let pin = ap_adapter::world_pins(&dirs).get(file).cloned().ok_or("That world isn't installed.")?;
    let bytes = tokio::fs::read(dirs.worlds.join(file)).await.map_err(|e| e.to_string())?;
    if ap_adapter::sha256_hex(&bytes) != pin.sha256 {
        return Err(format!("{file} changed on disk since it was installed."));
    }
    catalog_add(db, file, &bytes, "", "", "", by).await
}

/// Drop a catalog entry. Seasons that already installed it keep their copy.
pub async fn catalog_remove(db: &Db, sha256: &str) -> Result<(), String> {
    if !db.catalog_remove(sha256).await.map_err(|e| e.to_string())? {
        return Err("That world isn't in the catalog.".into());
    }
    let _ = tokio::fs::remove_file(catalog_file(sha256)).await;
    Ok(())
}

/// Drop every pending upload of a season (season reset).
pub async fn clear_pending(db: &Db, season: i64) {
    let dirs = config::get().season(season);
    let _ = tokio::fs::remove_dir_all(&dirs.pending).await;
    let _ = tokio::fs::create_dir_all(&dirs.pending).await;
    let _ = db.clear_world_requests(season).await;
}

/// The slot name and game a YAML declares at top level, read without AP
/// (for signups held before their world can be checked). `None` where it
/// isn't a plain value (e.g. a weighted `game:` map).
pub fn yaml_name_game(yaml: &str) -> (Option<String>, Option<String>) {
    let value = |key: &str| {
        yaml.lines().find_map(|l| {
            let v = l.strip_prefix(key)?.strip_prefix(':')?;
            let v = match v.find(" #") {
                Some(i) => &v[..i],
                None => v,
            }
            .trim();
            let v = v
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(v);
            (!v.is_empty()).then(|| v.chars().take(80).collect::<String>())
        })
    };
    (value("name"), value("game"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn apworld(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            for (name, body) in entries {
                z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
                z.write_all(body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn manifest_is_read_from_the_world_folder() {
        let w = apworld(&[
            ("foo/__init__.py", ""),
            ("foo/archipelago.json", r#"{"game": "Foo Quest", "world_version": "1.4.0", "version": 7}"#),
        ]);
        assert!(looks_like_apworld(&w));
        assert_eq!(manifest(&w), Manifest { game: "Foo Quest".into(), version: "1.4.0".into() });
        assert_eq!(manifest(&apworld(&[("foo/__init__.py", "")])), Manifest::default());
        assert_eq!(manifest(b"not a zip"), Manifest::default());
    }

    #[test]
    fn upload_names_are_slugged_and_reserved_names_refused() {
        assert_eq!(world_file("Pokemon Crystal.apworld").unwrap(), "Pokemon_Crystal.apworld");
        assert!(world_file("tracker.apworld").is_err());
        assert!(world_file("Minecraft.apworld").is_err());
    }

    #[test]
    fn yaml_name_and_game_are_read_at_top_level() {
        let y = "# comment\nname: \"Robin\"  # me\ngame: Foo Quest\nFoo Quest:\n  name: nested\n";
        assert_eq!(yaml_name_game(y), (Some("Robin".into()), Some("Foo Quest".into())));
        let y = "name: Sam\ngame:\n  A: 1\n  B: 1\n";
        assert_eq!(yaml_name_game(y), (Some("Sam".into()), None));
    }
}
