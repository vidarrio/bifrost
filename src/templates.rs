//! Starting YAMLs: Archipelago's own commented template for a game, which the
//! signup wizard offers for download once a player has picked their game.
//!
//! Writing them loads worlds, so it runs in the background, sandboxed (see
//! [`ap_adapter::write_templates`]), and the result is kept under
//! `$BIFROST_DATA/templates/<Archipelago version>/`:
//!
//! - `core/`: every core game's template, written once per version, with an
//!   `index.json` of which file is which game's;
//! - `world/<sha256>.yaml`: a custom world's (catalog or installed in a
//!   season), keyed by its file's hash. Only installed worlds get one: an
//!   upload waiting for approval never runs.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::ap_adapter;
use crate::apinstall::Install;
use crate::config;

/// Where a game's template stands.
pub enum Template {
    Ready(PathBuf),
    /// Being written (or about to be); ask again shortly.
    Preparing,
    /// It can't be had right now.
    Unavailable(String),
}

/// Jobs running, by key ("<version>/core" or "<version>/<sha256>").
static RUNNING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
/// When a job last failed: not retried sooner than [`RETRY`].
static FAILED: Mutex<BTreeMap<String, (Instant, String)>> = Mutex::new(BTreeMap::new());
const RETRY: Duration = Duration::from_secs(600);

fn root(version: &str) -> PathBuf {
    config::get().data_dir.join("templates").join(version)
}

fn is_sha(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Start `job` under `key` unless it's running or failed lately; what to tell
/// the caller meanwhile.
fn start<F>(key: String, job: F) -> Template
where
    F: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    if let Some((at, e)) = FAILED.lock().unwrap().get(&key)
        && at.elapsed() < RETRY
    {
        return Template::Unavailable(format!(
            "The template couldn't be made ({e}). Try again later."
        ));
    }
    if RUNNING.lock().unwrap().insert(key.clone()) {
        tokio::spawn(async move {
            // It loads every world, like a YAML check: queue behind those.
            let res = match crate::checker::check_gate().acquire().await {
                Ok(_permit) => job.await,
                Err(e) => Err(e.to_string()),
            };
            if let Err(e) = res {
                tracing::warn!("template {key}: {e}");
                FAILED
                    .lock()
                    .unwrap()
                    .insert(key.clone(), (Instant::now(), e));
            }
            RUNNING.lock().unwrap().remove(&key);
        });
    }
    Template::Preparing
}

/// A core game's template.
pub fn core(ap: &Install, game: &str) -> Template {
    let dir = root(&ap.version).join("core");
    let index: Option<BTreeMap<String, String>> = std::fs::read(dir.join("index.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    if let Some(index) = index {
        return match index.get(game) {
            Some(file) => Template::Ready(dir.join(file)),
            None => Template::Unavailable(format!(
                "Archipelago {} has no template for {game}.",
                ap.upstream()
            )),
        };
    }
    ensure_core(ap.clone())
}

/// Write the version's core templates in the background, unless they're there.
pub fn ensure_core(ap: Install) -> Template {
    if root(&ap.version).join("core/index.json").is_file() {
        return Template::Preparing;
    }
    let key = format!("{}/core", ap.version);
    start(key, async move {
        let (out, index) = ap_adapter::write_templates(&ap, None).await?;
        let res = keep(&out, &root(&ap.version).join("core"), &index).await;
        let _ = tokio::fs::remove_dir_all(out.parent().unwrap_or(&out)).await;
        res
    })
}

/// A custom world's template: the world at `path` (named `file`), whose
/// contents hash to `sha256`, for its game `game`.
pub fn world(ap: &Install, path: &Path, file: &str, sha256: &str, game: &str) -> Template {
    if !is_sha(sha256) {
        return Template::Unavailable("That world can't be found.".into());
    }
    let kept = root(&ap.version)
        .join("world")
        .join(format!("{sha256}.yaml"));
    if kept.is_file() {
        return Template::Ready(kept);
    }
    let key = format!("{}/{sha256}", ap.version);
    let (ap, path, file, game) = (
        ap.clone(),
        path.to_path_buf(),
        file.to_string(),
        game.to_string(),
    );
    start(key, async move {
        let (out, index) = ap_adapter::write_templates(&ap, Some((&path, &file))).await?;
        let res = match index.get(&game) {
            Some(name) => {
                let bytes = tokio::fs::read(out.join(name))
                    .await
                    .map_err(|e| e.to_string());
                match bytes {
                    Ok(b) => write(&kept, b).await,
                    Err(e) => Err(e),
                }
            }
            None => Err(format!("the world wrote no template for {game}")),
        };
        let _ = tokio::fs::remove_dir_all(out.parent().unwrap_or(&out)).await;
        res
    })
}

async fn write(path: &Path, bytes: Vec<u8>) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| e.to_string())?;
    }
    crate::atomic::write_async(path.to_path_buf(), bytes)
        .await
        .map_err(|e| e.to_string())
}

/// Copy the written templates into `dest`, the index last (it's what says
/// they're there).
async fn keep(out: &Path, dest: &Path, index: &BTreeMap<String, String>) -> Result<(), String> {
    if index.is_empty() {
        return Err("no templates were written".into());
    }
    for name in index.values() {
        let bytes = tokio::fs::read(out.join(name))
            .await
            .map_err(|e| e.to_string())?;
        write(&dest.join(name), bytes).await?;
    }
    write(
        &dest.join("index.json"),
        serde_json::to_vec(index).expect("index serializes"),
    )
    .await
}

/// Forget a version's templates, when that version is installed again or
/// removed: the next install under the same name may not be the same.
pub async fn forget(version: &str) {
    let _ = tokio::fs::remove_dir_all(root(version)).await;
    FAILED
        .lock()
        .unwrap()
        .retain(|k, _| !k.starts_with(&format!("{version}/")));
}
