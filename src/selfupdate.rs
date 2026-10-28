//! `bifrost update`: replace the native single-file build with another
//! release of it. The file comes from the GitHub release and must match the
//! release's `SHA256SUMS` (the download is `scripts/bifrost_ap_install.py`,
//! the helper that already talks to GitHub). The new file is written next to
//! this one and renamed over it, so a failure leaves this one in place; this
//! one is kept as `<name>.previous`. Bifrost isn't restarted: the service
//! manager does that, and the first start of the new version backs up the
//! data dir before upgrading it (see `main::upgrade`).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::{apupdate, bundle, release_check};

/// `bifrost update [<version>]`. Returns the exit code.
pub async fn run(version: Option<&str>) -> i32 {
    match update(version).await {
        Ok(msg) => {
            println!("{msg}");
            0
        }
        Err(e) => {
            eprintln!("update failed: {e}");
            1
        }
    }
}

async fn update(version: Option<&str>) -> Result<String, String> {
    if !bundle::present() {
        return Err(
            "only the native single-file build updates itself; for the container \
             image, pull the new tag and recreate the container"
                .into(),
        );
    }
    if std::env::consts::ARCH != "x86_64" {
        return Err(format!(
            "releases are built for x86_64 only, not {}",
            std::env::consts::ARCH
        ));
    }
    let current = release_check::current();
    let target = match version {
        Some(v) => v.trim().trim_start_matches('v').to_string(),
        None => {
            let releases = apupdate::releases(release_check::REPO).await?;
            match release_check::pick(&releases, current) {
                Some(n) => n.version,
                None => return Ok(format!("Bifrost {current} is the newest release")),
            }
        }
    };
    if target == current {
        return Ok(format!("this is already Bifrost {current}"));
    }
    if target.is_empty()
        || !target
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c))
    {
        return Err(format!("{target:?} isn't a version"));
    }
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("where this binary is: {e}"))?;
    let name = exe
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("this binary's name isn't UTF-8")?
        .to_string();
    let partial = exe.with_file_name(format!(".{name}.update-{target}"));
    let _ = std::fs::remove_file(&partial);
    println!("Downloading Bifrost {target}…");
    let res = install(&exe, &partial, &target).await;
    if res.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    let previous = res?;
    Ok(format!(
        "Updated {} from {current} to {target}. Restart Bifrost to run it (for example \
         `systemctl restart bifrost`); it backs up the data dir before upgrading it. \
         The previous binary is kept as {}.",
        exe.display(),
        previous.display()
    ))
}

/// Download `target` to `partial`, check it, and swap it in for `exe`.
/// Returns where the old binary went.
async fn install(exe: &Path, partial: &Path, target: &str) -> Result<PathBuf, String> {
    let tag = format!("v{target}");
    let asset = format!("bifrost-{target}-x86_64-linux");
    let out = apupdate::installer()
        .arg("download")
        .arg(release_check::REPO)
        .arg(&tag)
        .arg(&asset)
        .arg(partial)
        .output()
        .await
        .map_err(|e| format!("couldn't run the download helper: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    let reply: serde_json::Value = serde_json::from_str(last).map_err(|_| {
        format!(
            "unexpected reply: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
    })?;
    if reply["ok"] != true {
        return Err(reply["error"]
            .as_str()
            .unwrap_or("download failed")
            .to_string());
    }
    if !bundle::is_bundle(partial) {
        return Err(format!("{asset} isn't a Bifrost single-file build"));
    }
    let mode = std::fs::metadata(exe)
        .map(|m| m.permissions().mode())
        .unwrap_or(0o755);
    std::fs::set_permissions(partial, std::fs::Permissions::from_mode(mode | 0o500))
        .map_err(|e| format!("{}: {e}", partial.display()))?;
    let previous = exe.with_file_name(format!(
        "{}.previous",
        exe.file_name().unwrap_or_default().to_string_lossy()
    ));
    let _ = std::fs::remove_file(&previous);
    std::fs::hard_link(exe, &previous)
        .or_else(|_| std::fs::copy(exe, &previous).map(|_| ()))
        .map_err(|e| format!("keeping the previous binary as {}: {e}", previous.display()))?;
    std::fs::rename(partial, exe).map_err(|e| format!("replacing {}: {e}", exe.display()))?;
    Ok(previous)
}
