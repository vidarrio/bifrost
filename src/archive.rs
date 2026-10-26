//! Raw season bundle: Bifrost's own job (not the host's) to package a finished
//! season's replayable artifacts — the multidata (+ spoiler), the final save,
//! the player YAMLs, the custom apworlds, the AP server journal, and a roster
//! `season.txt` — into one tarball. The host `bifrost-export-seasons` script
//! only *imports* these bundles into Nextcloud; it never re-dumps.
//!
//! Produced at "Archive & finalize season" time, alongside the Past Seasons
//! summary. Named by the seed so a re-archive overwrites in place and the export
//! can tell which seasons are already synced.
//!
//! A bundle is also how a finished season moves between instances: it carries
//! `bundle.json` (a [`Manifest`]: format + version, name, seed, dates) and
//! `summary.json` (the Past Seasons summary), and [`import`] turns one into an
//! archived season here (`bifrost season import`, or the Seasons console).

use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

use serde::{Deserialize, Serialize};

use crate::config;
use crate::db::Db;
use crate::season::SeasonSummary;

/// What `bundle.json` says.
const FORMAT: &str = "bifrost-season";
const VERSION: u32 = 1;

/// `bundle.json`: what a bundle is, enough to import it.
#[derive(Serialize, Deserialize, Debug)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub seed: String,
    pub created_at: i64,
    pub live_at: Option<i64>,
    pub archived_at: i64,
    /// The Archipelago version that generated it.
    pub ap_version: Option<String>,
    /// The Bifrost that wrote it.
    pub bifrost: String,
}

impl Manifest {
    pub fn new(row: &crate::db::SeasonRow, name: &str, seed: &str, archived_at: i64) -> Manifest {
        Manifest {
            format: FORMAT.into(),
            version: VERSION,
            name: name.into(),
            seed: seed.into(),
            created_at: row.created_at,
            live_at: row.live_at,
            archived_at,
            ap_version: row.ap_version.clone(),
            bifrost: env!("CARGO_PKG_VERSION").into(),
        }
    }
}

/// Copy every file with `ext` from `src` into `dst`. Missing dir / no matches
/// is fine (returns Ok with whatever copied).
async fn copy_ext(src: &Path, dst: &Path, ext: &str) -> std::io::Result<()> {
    let mut rd = match tokio::fs::read_dir(src).await {
        Ok(rd) => rd,
        Err(_) => return Ok(()),
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some(ext) {
            if let Some(name) = p.file_name() {
                let _ = tokio::fs::copy(&p, dst.join(name)).await;
            }
        }
    }
    Ok(())
}

/// Build (or rebuild) the raw bundle for `seed`, writing
/// `<data>/archives/<seed>.tar.gz`. `season_txt` is the human-readable
/// roster/metadata the caller composed. Returns the bundle path on success.
pub async fn bundle_season(
    season_id: i64,
    seed: &str,
    season_txt: &str,
    manifest: &Manifest,
    summary: &SeasonSummary,
) -> Result<String, String> {
    let cfg = config::get();
    let season = cfg.season(season_id);
    let archives = cfg.archives_dir();
    tokio::fs::create_dir_all(&archives)
        .await
        .map_err(|e| format!("archives dir: {e}"))?;
    let stage = archives.join(format!(".stage-{seed}"));
    let _ = tokio::fs::remove_dir_all(&stage).await;
    let sp = stage.as_path();
    for sub in ["", "players", "worlds"] {
        tokio::fs::create_dir_all(sp.join(sub))
            .await
            .map_err(|e| format!("stage dir: {e}"))?;
    }

    // Core artifacts (what makes the season replayable).
    let _ = tokio::fs::copy(season.current_zip(), sp.join("multidata.zip")).await;
    let _ = tokio::fs::copy(season.current_save(), sp.join("save.apsave")).await;
    let _ = copy_ext(&season.players, &sp.join("players"), "yaml").await;
    let _ = copy_ext(&season.worlds, &sp.join("worlds"), "apworld").await;
    let _ = tokio::fs::copy(season.worlds_lock(), sp.join("worlds").join("worlds.lock.json")).await;

    // Metadata/roster, and what an import reads.
    let _ = tokio::fs::write(sp.join("season.txt"), season_txt).await;
    for (file, json) in [
        ("bundle.json", serde_json::to_vec_pretty(manifest)),
        ("summary.json", serde_json::to_vec_pretty(summary)),
    ] {
        let json = json.map_err(|e| format!("{file}: {e}"))?;
        tokio::fs::write(sp.join(file), json).await.map_err(|e| format!("{file}: {e}"))?;
    }

    // The AP server's log: a managed server's captured output, else (external
    // server) its systemd journal — best-effort; a missing log is omitted.
    let server_log = season.logs.join("server.log");
    if cfg.ap_managed {
        let _ = tokio::fs::copy(&server_log, sp.join("archipelago.log")).await;
    } else if let Ok(out) = Command::new("journalctl")
        .args(["-u", &cfg.ap_journal_unit, "--no-pager", "-q"])
        .stdin(Stdio::null())
        .output()
        .await
    {
        if out.status.success() && !out.stdout.is_empty() {
            let _ = tokio::fs::write(sp.join("archipelago.log"), &out.stdout).await;
        }
    }

    // Package. tar from inside the stage so paths are relative (no .stage- dir).
    let tar_path = archives.join(format!("{seed}.tar.gz"));
    let status = Command::new("tar")
        .arg("-C")
        .arg(&stage)
        .arg("-czf")
        .arg(&tar_path)
        .arg(".")
        .stdin(Stdio::null())
        .status()
        .await
        .map_err(|e| format!("tar spawn: {e}"))?;
    let _ = tokio::fs::remove_dir_all(&stage).await;
    if !status.success() {
        return Err(format!("tar exited {status}"));
    }
    Ok(tar_path.to_string_lossy().into_owned())
}

/// Read a bundle's manifest and summary (without unpacking anything else).
pub async fn read_bundle(path: &Path) -> Result<(Manifest, SeasonSummary), String> {
    let member = |name: &'static str| async move {
        let out = Command::new("tar")
            .arg("-xzOf")
            .arg(path)
            .arg(format!("./{name}"))
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|e| format!("tar: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "no {name} in it — not a season bundle from this Bifrost (bundles from before season import existed can't be imported)"
            ));
        }
        Ok(out.stdout)
    };
    let manifest: Manifest = serde_json::from_slice(&member("bundle.json").await?)
        .map_err(|e| format!("bundle.json: {e}"))?;
    if manifest.format != FORMAT {
        return Err(format!("bundle.json says it's a {:?}, not a {FORMAT}", manifest.format));
    }
    if manifest.version > VERSION {
        return Err(format!(
            "it's format version {}, newer than this Bifrost reads ({VERSION}) — upgrade Bifrost first",
            manifest.version
        ));
    }
    if !valid_seed(&manifest.seed) {
        return Err(format!("bundle.json has a bad seed name {:?}", manifest.seed));
    }
    let summary: SeasonSummary = serde_json::from_slice(&member("summary.json").await?)
        .map_err(|e| format!("summary.json: {e}"))?;
    Ok((manifest, summary))
}

/// A seed name safe to use as a file name (AP seeds are digits).
fn valid_seed(seed: &str) -> bool {
    !seed.is_empty() && seed.len() <= 64 && seed.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Import a season bundle as an archived season: its summary shows on Past
/// Seasons, and the bundle is kept in `archives/` (downloadable again). A
/// season with the same seed already here is refused. Returns its id.
pub async fn import(db: &Db, path: &Path, by: Option<i64>) -> Result<i64, String> {
    let (m, mut summary) = read_bundle(path).await?;
    if let Some(id) = db.season_by_seed(&m.seed).await {
        return Err(format!("season {id} here already has seed {} — it's imported already", m.seed));
    }
    let name = if m.name.trim().is_empty() { summary.name.clone() } else { m.name.clone() };
    summary.name = name.clone();
    let summary_json = serde_json::to_string(&summary).map_err(|e| e.to_string())?;
    let archives = config::get().archives_dir();
    tokio::fs::create_dir_all(&archives).await.map_err(|e| format!("archives dir: {e}"))?;
    let dest = archives.join(format!("{}.tar.gz", m.seed));
    if tokio::fs::canonicalize(path).await.ok() != tokio::fs::canonicalize(&dest).await.ok() {
        let tmp = archives.join(format!(".import-{}", m.seed));
        tokio::fs::copy(path, &tmp).await.map_err(|e| format!("keeping the bundle: {e}"))?;
        tokio::fs::rename(&tmp, &dest).await.map_err(|e| format!("keeping the bundle: {e}"))?;
    }
    db.import_season(&name, &m.seed, &summary_json, by, m.created_at, m.live_at, m.archived_at, m.ap_version.as_deref())
        .await
        .map_err(|e| e.to_string())
}

/// The kept bundle of an archived season, if there is one.
pub fn bundle_path(seed: &str) -> Option<std::path::PathBuf> {
    let p = config::get().archives_dir().join(format!("{seed}.tar.gz"));
    (valid_seed(seed) && p.is_file()).then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_are_file_safe() {
        assert!(valid_seed("94805671998530581428"));
        assert!(!valid_seed("../x") && !valid_seed("") && !valid_seed("a/b"));
    }
}
