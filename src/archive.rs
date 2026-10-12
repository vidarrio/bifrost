//! Raw season bundle: Bifrost's own job (not the host's) to package a finished
//! season's replayable artifacts — the multidata (+ spoiler), the final save,
//! the player YAMLs, the custom apworlds, the AP server journal, and a roster
//! `season.txt` — into one tarball. The host `bifrost-export-seasons` script
//! only *imports* these bundles into Nextcloud; it never re-dumps.
//!
//! Produced at "Archive & finalize season" time, alongside the Past Seasons
//! summary. Named by the seed so a re-archive overwrites in place and the export
//! can tell which seasons are already synced.

use std::process::Stdio;
use tokio::process::Command;

const AP_OUTPUT: &str = "/srv/archipelago/output";
const AP_PLAYERS: &str = "/srv/archipelago/Players";
const AP_WORLDS: &str = "/srv/archipelago/custom_worlds";
const SEASONS_DIR: &str = "/var/lib/bifrost/seasons";

/// Copy every file with `ext` from `src` into `dst`. Missing dir / no matches
/// is fine (returns Ok with whatever copied).
async fn copy_ext(src: &str, dst: &std::path::Path, ext: &str) -> std::io::Result<()> {
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
/// `/var/lib/bifrost/seasons/<seed>.tar.gz`. `season_txt` is the human-readable
/// roster/metadata the caller composed. Returns the bundle path on success.
pub async fn bundle_season(seed: &str, season_txt: &str) -> Result<String, String> {
    tokio::fs::create_dir_all(SEASONS_DIR)
        .await
        .map_err(|e| format!("seasons dir: {e}"))?;
    let stage = format!("{SEASONS_DIR}/.stage-{seed}");
    let _ = tokio::fs::remove_dir_all(&stage).await;
    let sp = std::path::Path::new(&stage);
    for sub in ["", "players", "worlds"] {
        tokio::fs::create_dir_all(sp.join(sub))
            .await
            .map_err(|e| format!("stage dir: {e}"))?;
    }

    // Core artifacts (what makes the season replayable).
    let _ = tokio::fs::copy(
        format!("{AP_OUTPUT}/current.zip"),
        sp.join("multidata.zip"),
    )
    .await;
    let _ = tokio::fs::copy(
        format!("{AP_OUTPUT}/current.apsave"),
        sp.join("save.apsave"),
    )
    .await;
    let _ = copy_ext(AP_PLAYERS, &sp.join("players"), "yaml").await;
    let _ = copy_ext(AP_WORLDS, &sp.join("worlds"), "apworld").await;

    // Metadata/roster.
    let _ = tokio::fs::write(sp.join("season.txt"), season_txt).await;

    // AP server journal (best-effort — needs the systemd-journal group; if the
    // dump is empty/failed the bundle simply omits it rather than failing).
    if let Ok(out) = Command::new("journalctl")
        .args(["-u", "archipelago", "--no-pager", "-q"])
        .stdin(Stdio::null())
        .output()
        .await
    {
        if out.status.success() && !out.stdout.is_empty() {
            let _ = tokio::fs::write(sp.join("archipelago.log"), &out.stdout).await;
        }
    }

    // Package. tar from inside the stage so paths are relative (no .stage- dir).
    let tar_path = format!("{SEASONS_DIR}/{seed}.tar.gz");
    let status = Command::new("tar")
        .args(["-C", &stage, "-czf", &tar_path, "."])
        .stdin(Stdio::null())
        .status()
        .await
        .map_err(|e| format!("tar spawn: {e}"))?;
    let _ = tokio::fs::remove_dir_all(&stage).await;
    if !status.success() {
        return Err(format!("tar exited {status}"));
    }
    Ok(tar_path)
}
