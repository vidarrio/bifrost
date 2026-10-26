//! Raw season bundle: Bifrost's own job (not the host's) to package a finished
//! season's replayable artifacts — the multidata (+ spoiler), the final save,
//! the player YAMLs, the custom apworlds, the AP server journal, and a roster
//! `season.txt` — into one tarball. The host `bifrost-export-seasons` script
//! only *imports* these bundles into Nextcloud; it never re-dumps.
//!
//! Produced at "Archive & finalize season" time, alongside the Past Seasons
//! summary. Named by the seed so a re-archive overwrites in place and the export
//! can tell which seasons are already synced.

use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

use crate::config;

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
pub async fn bundle_season(seed: &str, season_txt: &str) -> Result<String, String> {
    let cfg = config::get();
    let season = cfg.season();
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

    // Metadata/roster.
    let _ = tokio::fs::write(sp.join("season.txt"), season_txt).await;

    // AP server journal (best-effort — needs the systemd-journal group; if the
    // dump is empty/failed the bundle simply omits it rather than failing).
    if let Ok(out) = Command::new("journalctl")
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
