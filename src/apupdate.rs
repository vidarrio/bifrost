//! Installing Archipelago releases from the settings page:
//! check GitHub for releases, install one in the background, and only call
//! it ready once a test generation passed with it. Switching new seasons to
//! it, and removing old installs, are separate buttons (see `apinstall` for
//! what "active" and "pinned" mean).
//!
//! The download/venv work is `scripts/bifrost_ap_install.py`, run under
//! `BIFROST_PYTHON` outside the sandbox (it needs the network and builds the
//! interpreter the sandbox later runs). The test generation runs sandboxed,
//! like any other.

use std::path::Path;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::apinstall::{self, Install};
use crate::{ap_adapter, config};

const RELEASES_TIMEOUT: Duration = Duration::from_secs(60);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(45 * 60);
/// Progress lines kept for the page.
const LOG_LINES: usize = 60;

#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub tag: String,
    pub name: String,
    pub prerelease: bool,
    pub published: String,
}

#[derive(Clone, Debug)]
pub struct Job {
    pub version: String,
    pub started: i64,
    pub log: Vec<String>,
    /// `None` while running.
    pub outcome: Option<Result<String, String>>,
}

#[derive(Clone, Default)]
pub struct Updates {
    /// The last release check: when, and what it found.
    pub releases: Option<(i64, Result<Vec<Release>, String>)>,
    /// The current or last install.
    pub job: Option<Job>,
}

static UPDATES: Mutex<Updates> = Mutex::new(Updates {
    releases: None,
    job: None,
});

pub fn snapshot() -> Updates {
    UPDATES.lock().unwrap().clone()
}

fn installer() -> tokio::process::Command {
    let cfg = config::get();
    let mut cmd = tokio::process::Command::new(&cfg.base_python);
    cmd.arg(cfg.script("bifrost_ap_install.py"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    cmd
}

/// Ask GitHub for recent Archipelago releases (a few seconds).
pub async fn check_releases() {
    let res = releases(&config::get().ap_repo).await;
    if let Err(e) = &res {
        tracing::warn!("checking Archipelago releases: {e}");
    }
    UPDATES.lock().unwrap().releases = Some((crate::auth::now(), res));
}

/// The recent releases (drafts left out) of GitHub repo `owner/name`.
pub async fn releases(repo: &str) -> Result<Vec<Release>, String> {
    let cfg = config::get();
    let out = tokio::time::timeout(
        RELEASES_TIMEOUT,
        installer().arg("releases").arg(repo).output(),
    )
    .await
    .map_err(|_| "GitHub didn't answer in time".to_string())?
    .map_err(|e| format!("couldn't run {}: {e}", cfg.base_python.display()))?;
    #[derive(Deserialize)]
    struct Reply {
        ok: bool,
        #[serde(default)]
        releases: Vec<Release>,
        #[serde(default)]
        error: String,
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    let r: Reply = serde_json::from_str(line).map_err(|_| {
        format!(
            "unexpected reply: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
    })?;
    if r.ok { Ok(r.releases) } else { Err(r.error) }
}

fn log(line: impl Into<String>) {
    let line = line.into();
    tracing::info!("AP install: {line}");
    if let Some(j) = UPDATES.lock().unwrap().job.as_mut() {
        j.log.push(line);
        let n = j.log.len();
        if n > LOG_LINES {
            j.log.drain(..n - LOG_LINES);
        }
    }
}

/// Install release `tag` and wait for it — at startup, for a live season whose
/// version went missing (an image upgrade replaced the bundled install), so it
/// keeps running the version that generated it. Waits out a running install.
pub async fn ensure(tag: &str) -> Result<(), String> {
    static ONE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one = ONE.lock().await;
    loop {
        if apinstall::find(tag).is_some() {
            return Ok(());
        }
        let ours = match start_install(tag) {
            Ok(()) => true,
            Err(e) if e.contains("already running") => false,
            Err(e) => return Err(e),
        };
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let u = UPDATES.lock().unwrap();
            match &u.job {
                Some(j) if j.outcome.is_none() => continue,
                Some(j) if ours && j.version == tag => {
                    return j.outcome.clone().unwrap_or(Ok(String::new())).map(|_| ());
                }
                _ => break,
            }
        }
    }
}

/// Start installing release `tag` in the background.
pub fn start_install(tag: &str) -> Result<(), String> {
    if !apinstall::valid_version(tag) {
        return Err("That isn't a release tag.".into());
    }
    if apinstall::find(tag).is_some() {
        return Err(format!("Archipelago {tag} is already installed."));
    }
    {
        let mut u = UPDATES.lock().unwrap();
        if matches!(&u.job, Some(j) if j.outcome.is_none()) {
            return Err("An install is already running.".into());
        }
        u.job = Some(Job {
            version: tag.to_string(),
            started: crate::auth::now(),
            log: vec![],
            outcome: None,
        });
    }
    let tag = tag.to_string();
    tokio::spawn(async move {
        let outcome = install(&tag).await;
        match &outcome {
            Ok(m) => tracing::info!("AP install {tag}: {m}"),
            Err(e) => tracing::warn!("AP install {tag} failed: {e}"),
        }
        if let Some(j) = UPDATES.lock().unwrap().job.as_mut() {
            j.outcome = Some(outcome);
        }
    });
    Ok(())
}

#[derive(Deserialize, Default)]
struct InstallReply {
    ok: bool,
    #[serde(default)]
    failed_worlds: Vec<String>,
    #[serde(default)]
    speedups: bool,
    #[serde(default)]
    error: String,
}

async fn install(tag: &str) -> Result<String, String> {
    let root = apinstall::root();
    let partial = root.join(format!(".partial-{tag}"));
    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|e| format!("{}: {e}", root.display()))?;
    let res = install_into(tag, &partial).await;
    if res.is_err() {
        let _ = tokio::fs::remove_dir_all(&partial).await;
    }
    res
}

async fn install_into(tag: &str, partial: &Path) -> Result<String, String> {
    let cfg = config::get();
    let mut child = installer()
        .arg("install")
        .arg(&cfg.ap_repo)
        .arg(tag)
        .arg(partial)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't run {}: {e}", cfg.base_python.display()))?;
    let mut lines = BufReader::new(child.stdout.take().expect("piped")).lines();
    let mut last = String::new();
    let read = async {
        while let Ok(Some(l)) = lines.next_line().await {
            if l.trim_start().starts_with('{') {
                last = l;
            } else if !l.trim().is_empty() {
                log(l);
            }
        }
        child.wait().await
    };
    tokio::time::timeout(INSTALL_TIMEOUT, read)
        .await
        .map_err(|_| "the install took too long".to_string())?
        .map_err(|e| e.to_string())?;
    let reply: InstallReply = serde_json::from_str(&last).unwrap_or_default();
    if !reply.ok {
        return Err(if reply.error.is_empty() {
            "the installer failed".into()
        } else {
            reply.error
        });
    }

    let version = apinstall::read_version(partial).ok_or("couldn't read the installed version")?;
    if apinstall::find(&version).is_some() {
        return Err(format!(
            "that release is Archipelago {version}, which is already installed"
        ));
    }
    let install = Install {
        version: version.clone(),
        dir: partial.to_path_buf(),
        python: partial.join("venv/bin/python"),
        bundled: false,
    };
    log("Running a test generation…");
    smoke_test(&install).await?;

    let dest = apinstall::root().join(&version);
    let _ = tokio::fs::remove_dir_all(&dest).await;
    tokio::fs::rename(partial, &dest)
        .await
        .map_err(|e| format!("moving into place: {e}"))?;
    apinstall::mark_ready(&dest).map_err(|e| format!("marking it ready: {e}"))?;

    let mut msg = format!("Archipelago {version} is installed and passed a test generation.");
    if !reply.failed_worlds.is_empty() {
        msg.push_str(&format!(
            " These worlds' extra requirements didn't install, so they may not work: {}.",
            reply.failed_worlds.join(", ")
        ));
    }
    if !reply.speedups {
        msg.push_str(
            " AP's speedups didn't build (that needs a C compiler and the Python headers), so it uses its slower pure-Python fallback.",
        );
    }
    Ok(msg)
}

/// A core game every recent Archipelago ships, for the test run.
fn smoke_game(install: &Install) -> Option<(&'static str, &'static str)> {
    [
        ("checksfinder", "ChecksFinder"),
        ("clique", "Clique"),
        ("apquest", "APQuest"),
    ]
    .into_iter()
    .find(|(dir, _)| install.dir.join("worlds").join(dir).is_dir())
}

/// Check a YAML and roll a one-player seed with `install`, sandboxed, in a
/// scratch user dir.
async fn smoke_test(install: &Install) -> Result<(), String> {
    let (_, game) = smoke_game(install).ok_or("none of the test games are in this release")?;
    let dirs = config::get()
        .scratch_season(&format!("ap-test-{}", install.version))
        .with_install(install.clone());
    let _ = tokio::fs::remove_dir_all(&dirs.root).await;
    dirs.ensure().map_err(|e| format!("scratch dir: {e}"))?;
    let yaml = format!("name: Tester\ngame: {game}\n{game}: {{}}\n");
    let checked = ap_adapter::check_yaml(&dirs, &yaml)
        .await
        .map_err(|e| format!("test check: {e}"))?;
    if !checked.ok {
        return Err(format!("test check failed: {}", checked.errors.join("; ")));
    }
    tokio::fs::write(dirs.players.join("tester.yaml"), &yaml)
        .await
        .map_err(|e| e.to_string())?;
    ap_adapter::generate(&dirs)
        .await
        .map_err(|e| format!("test generation: {e}"))?;
    let rolled = std::fs::read_dir(&dirs.output)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().ends_with(".zip"));
    let _ = tokio::fs::remove_dir_all(&dirs.root).await;
    if rolled {
        Ok(())
    } else {
        Err("the test generation produced no seed".into())
    }
}

/// Delete a managed install nothing uses.
pub async fn remove(version: &str) -> Result<(), String> {
    let i = apinstall::find(version).ok_or("That version isn't installed.")?;
    if i.bundled {
        return Err("The bundled Archipelago can't be removed.".into());
    }
    if apinstall::active().version == version {
        return Err("That's the active version — switch new seasons to another one first.".into());
    }
    if !apinstall::pinned_to(version).is_empty() {
        return Err("A season still runs on that version — archive or re-roll it first.".into());
    }
    tokio::fs::remove_dir_all(&i.dir)
        .await
        .map_err(|e| format!("removing it: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_reply_parses() {
        let r: InstallReply =
            serde_json::from_str(r#"{"ok": true, "failed_worlds": ["sc2"], "speedups": false}"#)
                .unwrap();
        assert!(r.ok && r.failed_worlds == ["sc2"] && !r.speedups);
        let r: InstallReply =
            serde_json::from_str(r#"{"ok": false, "error": "HTTPError: 404"}"#).unwrap();
        assert!(!r.ok && r.error.contains("404"));
    }
}
