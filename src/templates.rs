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
        /// Takes the key out of [`RUNNING`] however the job ends. If it
        /// ends without finishing (a panic), that counts as a failure, so it
        /// isn't retried sooner than [`RETRY`] either.
        struct Done(String, bool);
        impl Drop for Done {
            fn drop(&mut self) {
                if !self.1
                    && let Ok(mut f) = FAILED.lock()
                {
                    f.insert(
                        self.0.clone(),
                        (Instant::now(), "it stopped unexpectedly".into()),
                    );
                }
                if let Ok(mut r) = RUNNING.lock() {
                    r.remove(&self.0);
                }
            }
        }
        tokio::spawn(async move {
            let mut done = Done(key.clone(), false);
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
            done.1 = true;
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
                let bytes = read_output_async(&out, name).await;
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

/// The most a template may be. Archipelago's run to about 100 KiB.
const MAX_TEMPLATE: u64 = 4 << 20;

/// Read the file `name` the sandboxed run wrote into `out`. The run could
/// write anything there, and something it leaves behind could still be
/// changing it, so: `name` must be one plain file name, neither `out` nor the
/// file may be a symlink (each is opened with `O_NOFOLLOW`, the file through
/// the directory's own descriptor, so nothing can be swapped in between), and
/// it must be a regular file no bigger than [`MAX_TEMPLATE`].
fn read_output(out: &Path, name: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    let plain = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\0'])
        && name.ends_with(".yaml");
    if !plain {
        return Err(format!("unexpected template name {name:?}"));
    }
    let dir = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(out)
        .map_err(|e| format!("templates dir: {e}"))?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(format!("/proc/self/fd/{}/{name}", dir.as_raw_fd()))
        .map_err(|e| format!("template {name}: {e}"))?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err(format!("template {name} isn't a regular file"));
    }
    if meta.len() > MAX_TEMPLATE {
        return Err(format!("template {name} is too big"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_TEMPLATE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_TEMPLATE {
        return Err(format!("template {name} is too big"));
    }
    Ok(bytes)
}

/// [`read_output`], off the async runtime's threads.
async fn read_output_async(out: &Path, name: &str) -> Result<Vec<u8>, String> {
    let (out, name) = (out.to_path_buf(), name.to_string());
    tokio::task::spawn_blocking(move || read_output(&out, &name))
        .await
        .map_err(|e| e.to_string())?
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
        let bytes = read_output_async(out, name).await?;
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

/// Drop the custom worlds' templates no one can ask for any more: those of
/// files neither in the catalog nor installed in any season. Returns how many.
pub async fn prune(db: &crate::db::Db) -> usize {
    let mut keep: BTreeSet<String> = db.catalog().await.into_iter().map(|w| w.sha256).collect();
    for s in db.seasons().await {
        let dirs = config::get().season(s.id);
        keep.extend(
            crate::ap_adapter::world_pins(&dirs)
                .into_values()
                .map(|p| p.sha256),
        );
    }
    prune_keeping(&config::get().data_dir.join("templates"), &keep).await
}

async fn prune_keeping(templates: &Path, keep: &BTreeSet<String>) -> usize {
    let mut removed = 0;
    let Ok(mut versions) = tokio::fs::read_dir(templates).await else {
        return 0;
    };
    while let Ok(Some(v)) = versions.next_entry().await {
        let Ok(mut files) = tokio::fs::read_dir(v.path().join("world")).await else {
            continue;
        };
        while let Ok(Some(f)) = files.next_entry().await {
            let name = f.file_name().to_string_lossy().to_string();
            let Some(sha) = name.strip_suffix(".yaml") else {
                continue;
            };
            if !keep.contains(sha) && tokio::fs::remove_file(f.path()).await.is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// Prune now and then once a day.
pub fn spawn_pruner(db: crate::db::Db) {
    tokio::spawn(async move {
        loop {
            let n = prune(&db).await;
            if n > 0 {
                tracing::info!("templates: removed {n} no one can ask for any more");
            }
            tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);
    impl Dir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn pruning_keeps_only_the_files_asked_for() {
        let base = std::env::temp_dir().join(format!("bifrost-tpl-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let d = Dir(base);
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        for v in ["0.6.7", "0.6.8"] {
            let w = d.path().join(v).join("world");
            std::fs::create_dir_all(&w).unwrap();
            std::fs::write(w.join(format!("{a}.yaml")), "x").unwrap();
            std::fs::write(w.join(format!("{b}.yaml")), "x").unwrap();
            std::fs::create_dir_all(d.path().join(v).join("core")).unwrap();
            std::fs::write(d.path().join(v).join("core/index.json"), "{}").unwrap();
        }
        let keep = BTreeSet::from([a.clone()]);
        assert_eq!(prune_keeping(d.path(), &keep).await, 2);
        for v in ["0.6.7", "0.6.8"] {
            assert!(d.path().join(v).join(format!("world/{a}.yaml")).is_file());
            assert!(!d.path().join(v).join(format!("world/{b}.yaml")).exists());
            assert!(
                d.path().join(v).join("core/index.json").is_file(),
                "core templates stay"
            );
        }
        assert_eq!(prune_keeping(&d.path().join("missing"), &keep).await, 0);
    }

    #[test]
    fn output_reads_only_plain_files_in_the_dir() {
        let base = std::env::temp_dir().join(format!("bifrost-tpl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let d = Dir(base);
        let out = d.path().join("out");
        std::fs::create_dir(&out).unwrap();
        std::fs::write(out.join("A Game.yaml"), b"game: A Game\n").unwrap();
        assert_eq!(read_output(&out, "A Game.yaml").unwrap(), b"game: A Game\n");

        // A name reaching outside the dir, or not a template's.
        let secret = d.path().join("secret.yaml");
        std::fs::write(&secret, b"secret").unwrap();
        for name in [
            "../secret.yaml",
            "/etc/passwd",
            "..",
            "",
            "x.txt",
            "sub/x.yaml",
        ] {
            assert!(read_output(&out, name).is_err(), "{name:?}");
        }
        // A symlink in place of the file, or of the dir itself.
        std::os::unix::fs::symlink(&secret, out.join("link.yaml")).unwrap();
        assert!(read_output(&out, "link.yaml").is_err());
        let linked = d.path().join("linked");
        std::os::unix::fs::symlink(&out, &linked).unwrap();
        assert!(read_output(&linked, "A Game.yaml").is_err());
        // Not a regular file.
        std::fs::create_dir(out.join("dir.yaml")).unwrap();
        assert!(read_output(&out, "dir.yaml").is_err());
    }
}
