//! `bifrost update`: replace the native single-file build with another
//! release of it. The file comes from the GitHub release and must match the
//! release's `SHA256SUMS`, whose signature (`SHA256SUMS.sig`, made by the
//! release workflow) must verify against [`RELEASE_KEY`]. The download is
//! `scripts/bifrost_ap_install.py`, the helper that already talks to GitHub.
//! The new file is written next to this one and renamed over it, so a
//! failure leaves this one in place; this one is kept as `<name>.previous`. Bifrost isn't restarted: the service
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
    // Per process: the helper creates it exclusively, and two concurrent
    // runs never share one.
    let partial = exe.with_file_name(format!(".{name}.update-{target}-{}", std::process::id()));
    let _ = std::fs::remove_file(&partial);
    println!("Downloading Bifrost {target}…");
    let res = install(&exe, &partial, &target).await;
    if res.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    let previous = res?;
    Ok(format!(
        "Updated {} from {current} to {target}. Restart Bifrost to run it (for example \
         `systemctl restart bifrost`); its first start backs up the data dir before \
         upgrading it. The previous binary is kept as {}: to go back after the new \
         version has run, restore that pre-upgrade backup (`bifrost backup restore`) \
         and put the previous binary back.",
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
    verify_release(&release_key()?, &reply, &asset, partial)?;
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
    // On disk before it takes the old binary's name (the helper synced the
    // data; this covers the permissions too).
    std::fs::File::open(partial)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("{}: {e}", partial.display()))?;
    std::fs::rename(partial, exe).map_err(|e| format!("replacing {}: {e}", exe.display()))?;
    if let Some(dir) = exe.parent() {
        let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
    }
    Ok(previous)
}

/// The public key release checksums are signed with (ed25519, hex). Its
/// private half is the release workflow's `RELEASE_SIGNING_KEY` secret.
const RELEASE_KEY: &str = "ff564625fda5723ffdefddfb226b1d0748ac658b5f46acf795caec1822b44c24";

/// The key to check releases against: [`RELEASE_KEY`], or for tests a
/// `BIFROST_UPDATE_KEY` (as `BIFROST_GITHUB_API` points at a test server),
/// which is announced so it can't go unnoticed.
fn release_key() -> Result<ed25519_dalek::VerifyingKey, String> {
    match crate::config::var("BIFROST_UPDATE_KEY") {
        Some(hex) => {
            eprintln!(
                "warning: BIFROST_UPDATE_KEY is set: releases are checked against it, \
                 not the project's release key"
            );
            parse_key(&hex)
        }
        None => parse_key(RELEASE_KEY),
    }
}

fn parse_key(hex: &str) -> Result<ed25519_dalek::VerifyingKey, String> {
    let bytes: [u8; 32] = unhex(hex)
        .and_then(|b| b.try_into().ok())
        .ok_or("this build has no release signing key")?;
    ed25519_dalek::VerifyingKey::from_bytes(&bytes).map_err(|e| format!("release key: {e}"))
}

/// The release's `SHA256SUMS` must carry a valid signature, and list the
/// downloaded file with its actual SHA-256 (checked here, not trusted from
/// the helper).
fn verify_release(
    key: &ed25519_dalek::VerifyingKey,
    reply: &serde_json::Value,
    asset: &str,
    file: &Path,
) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let sums = reply["sums"]
        .as_str()
        .and_then(unhex)
        .ok_or("the download helper didn't return SHA256SUMS")?;
    let sig: [u8; 64] = reply["sig"]
        .as_str()
        .and_then(unhex)
        .and_then(|b| b.try_into().ok())
        .ok_or("this release isn't signed (no SHA256SUMS.sig), so it can't be verified")?;
    key.verify_strict(&sums, &ed25519_dalek::Signature::from_bytes(&sig))
        .map_err(|_| "SHA256SUMS's signature doesn't verify: not a genuine release")?;
    let want = String::from_utf8_lossy(&sums)
        .lines()
        .filter_map(|l| l.split_once(char::is_whitespace))
        .find(|(_, name)| name.trim().trim_start_matches('*') == asset)
        .map(|(h, _)| h.to_ascii_lowercase())
        .ok_or_else(|| format!("SHA256SUMS doesn't list {asset}"))?;
    let mut hasher = Sha256::new();
    let mut f = std::fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut buf = vec![0; 1 << 16];
    loop {
        use std::io::Read;
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("{}: {e}", file.display())),
        }
    }
    let got: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if got != want {
        return Err(format!("{asset} doesn't match the signed SHA256SUMS"));
    }
    Ok(())
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn setup(name: &str, contents: &[u8]) -> (std::path::PathBuf, String) {
        use sha2::{Digest, Sha256};
        let dir = std::env::temp_dir().join(format!("bifrost-upd-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("asset");
        std::fs::write(&f, contents).unwrap();
        let h: String = Sha256::digest(contents)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        (f, format!("{h}  bifrost-9.9.9-x86_64-linux\n"))
    }

    fn hexs(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn the_release_key_parses() {
        assert!(parse_key(RELEASE_KEY).is_ok());
    }

    #[test]
    fn unhex_is_strict() {
        assert_eq!(unhex("0aFf"), Some(vec![0x0a, 0xff]));
        for bad in ["+1", "0g", "abc", "0 a"] {
            assert_eq!(unhex(bad), None, "{bad}");
        }
    }

    #[test]
    fn releases_must_be_signed_by_the_release_key() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let pubkey = key.verifying_key();
        let asset = "bifrost-9.9.9-x86_64-linux";
        let (file, sums) = setup("sig", b"payload");
        let reply = |sig: Option<&[u8]>, sums: &str| serde_json::json!({"sums": hexs(sums.as_bytes()), "sig": sig.map(hexs)});
        let check = |sig: Option<&[u8]>, sums: &str| {
            verify_release(&pubkey, &reply(sig, sums), asset, &file)
        };
        let good = key.sign(sums.as_bytes()).to_bytes();
        assert!(check(Some(&good), &sums).is_ok());
        let e = check(None, &sums).unwrap_err();
        assert!(e.contains("isn't signed"), "{e}");
        let forged = other.sign(sums.as_bytes()).to_bytes();
        assert!(check(Some(&forged), &sums).is_err());
        // A signature over other sums doesn't carry over.
        let (_, other_sums) = setup("other", b"other payload");
        let sig = key.sign(other_sums.as_bytes()).to_bytes();
        let e = check(Some(&sig), &other_sums).unwrap_err();
        assert!(e.contains("doesn't match"), "{e}");
        // Signed sums that list the right hash under another name, or a
        // longer one, don't vouch for the asset.
        let hash = sums.split_whitespace().next().unwrap();
        for name in [
            "bifrost-9.9.8-x86_64-linux",
            "bifrost-9.9.9-x86_64-linux.old",
            "x-bifrost-9.9.9-x86_64-linux",
        ] {
            let renamed = format!("{hash}  {name}\n");
            let sig = key.sign(renamed.as_bytes()).to_bytes();
            let e = check(Some(&sig), &renamed).unwrap_err();
            assert!(e.contains("doesn't list"), "{name}: {e}");
        }
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }
}
