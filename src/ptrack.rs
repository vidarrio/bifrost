//! Bifrost side of the PopTracker map feature: stores uploaded packs per slot
//! (extracted to disk) and computes the tracker state server-side via the
//! `poptracker` engine crate.
//!
//! The engine runs a pack's Lua (sandboxed) against the slot's live AP state and
//! returns a [`poptracker::TrackerState`] (maps + markers + items). We feed it
//! the slot's received items (resolved to names via `Meta`); checked-location
//! clearing is a later refinement (needs the per-slot checked-location ids).
//!
//! A pack's Lua is untrusted, so it only ever runs in a worker process of its
//! own ([`run_worker`]): sandboxed, and killed if it runs too long.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use crate::sandbox::{self, Job};
use crate::{ap_adapter, config};

/// Per-slot extracted-pack directory: `<data>/packs/<season>/<ap_slot>/`.
fn slot_dir(season: i64, ap_slot: i64) -> PathBuf {
    config::get()
        .packs_dir()
        .join(season.to_string())
        .join(ap_slot.to_string())
}

/// Is a usable pack installed for this slot?
pub fn has_pack(season: i64, ap_slot: i64) -> bool {
    poptracker::pack_root(&slot_dir(season, ap_slot)).is_ok()
}

/// A version stamp for the slot's pack (install dir mtime, epoch secs), used to
/// cache-bust asset URLs when a pack is (re)installed — browsers otherwise keep
/// showing cached images even across a hard refresh.
pub fn pack_version(season: i64, ap_slot: i64) -> u64 {
    std::fs::metadata(slot_dir(season, ap_slot))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Validate uploaded zip bytes: must load and expose an Archipelago autotracking
/// path (memory-only/emulator packs can't be driven server-side). Returns a
/// summary on success, else an error message suitable for showing the uploader.
pub async fn validate(zip_bytes: Vec<u8>) -> Result<poptracker::Summary, String> {
    let scratch = Scratch::new()?;
    let dir = scratch.0.clone();
    // Unpacking is the host's own code; only running the pack's Lua is isolated.
    let root = tokio::task::spawn_blocking(move || {
        let zip = dir.join("pack.zip");
        std::fs::write(&zip, &zip_bytes).map_err(|e| format!("write temp: {e}"))?;
        let unpacked = dir.join("pack");
        poptracker::extract_zip(&zip, &unpacked).map_err(|e| e.to_string())?;
        poptracker::pack_root(&unpacked).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "validate failed".to_string())??;
    let request = Request {
        pack: root.clone(),
        feed: None,
    };
    let reply = run_worker(&scratch, &root, &request).await?;
    let sum: poptracker::Summary = serde_json::from_value(reply).map_err(|e| e.to_string())?;
    if !sum.archipelago_autotracking {
        return Err(
            "this pack has no Archipelago auto-tracking — it only works with an \
                    emulator, so it can't be tracked here"
                .to_string(),
        );
    }
    Ok(sum)
}

/// Install (extract) an uploaded pack zip for a slot, replacing any existing one.
pub fn install(season: i64, ap_slot: i64, zip_bytes: &[u8]) -> Result<(), String> {
    let dir = slot_dir(season, ap_slot);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir: {e}"))?;
    let tmp = write_temp("install", zip_bytes)?;
    let r = poptracker::extract_zip(&tmp, &dir).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&tmp);
    r?;
    // Browsers can render an indexed PNG with its palette background instead of
    // honouring `tRNS` transparency (notably under `image-rendering:pixelated`),
    // leaving transparent icons with a background — convert them to RGBA.
    // (Called from `spawn_blocking`, where blocking on the runtime is fine.)
    tokio::runtime::Handle::current().block_on(ap_adapter::normalize_pngs(&dir));
    Ok(())
}

/// Remove a slot's installed pack.
pub fn remove(season: i64, ap_slot: i64) {
    let _ = std::fs::remove_dir_all(slot_dir(season, ap_slot));
}

/// What a pack learns on connect, besides the items: the slot's slot data and
/// the multiworld's seed name and players (`(slot, name, game)`).
#[derive(Default, Serialize)]
pub struct Connect<'a> {
    pub slot_data: Option<&'a serde_json::Value>,
    pub seed: Option<&'a str>,
    pub players: Vec<(i64, &'a str, &'a str)>,
}

impl<'a> Connect<'a> {
    /// The connect data for `ap_slot` from the season's metadata.
    pub fn from_meta(meta: &'a crate::meta::Meta, ap_slot: i64) -> Self {
        Connect {
            slot_data: meta.totals(ap_slot).map(|s| &s.slot_data),
            seed: meta.seed.as_deref(),
            players: (meta.slots.iter())
                .map(|(slot, s)| (*slot, s.name.as_str(), s.game.as_str()))
                .collect(),
        }
    }
}

/// Everything fed to a pack to compute its state.
#[derive(Serialize)]
pub struct Feed<'a> {
    pub items: &'a [(i64, String)],
    pub checked: &'a [i64],
    pub valid_locations: &'a [i64],
    pub player: i64,
    pub connect: Connect<'a>,
}

/// Compute the tracker state for a slot from its installed pack + what's fed
/// to it, as JSON ([`poptracker::TrackerState`]).
pub async fn compute(
    season: i64,
    ap_slot: i64,
    feed: Feed<'_>,
) -> Result<serde_json::Value, String> {
    let root = poptracker::pack_root(&slot_dir(season, ap_slot)).map_err(|e| e.to_string())?;
    let scratch = Scratch::new()?;
    let request = Request {
        pack: root.clone(),
        feed: Some(feed),
    };
    run_worker(&scratch, &root, &request).await
}

/// One slot's computes at a time, so viewers of a slot can't pile up workers.
/// `None` when one is already running; dropping the claim ends it.
pub fn claim_compute(season: i64, ap_slot: i64) -> Option<ComputeClaim> {
    COMPUTING
        .lock()
        .unwrap()
        .insert((season, ap_slot))
        .then_some(ComputeClaim(season, ap_slot))
}

static COMPUTING: Mutex<BTreeSet<(i64, i64)>> = Mutex::new(BTreeSet::new());

pub struct ComputeClaim(i64, i64);

impl Drop for ComputeClaim {
    fn drop(&mut self) {
        COMPUTING.lock().unwrap().remove(&(self.0, self.1));
    }
}

/// Pack workers running at once, across all slots and uploads.
static WORKERS: Semaphore = Semaphore::const_new(4);

/// How long one pack worker may run. Pack Lua is untrusted, and not everything
/// it can do is seen by the engine's instruction cap, so this is the real limit.
fn worker_timeout() -> Duration {
    let secs = config::var("BIFROST_PACK_TIMEOUT_SECS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    Duration::from_secs(secs)
}

/// The most a worker may send back.
const MAX_REPLY: usize = 64 << 20;

/// What a worker is asked: load the pack, and either summarise it or, given a
/// feed, compute its state.
#[derive(Serialize)]
struct Request<'a> {
    pack: PathBuf,
    feed: Option<Feed<'a>>,
}

#[derive(Deserialize)]
struct WorkerRequest {
    pack: PathBuf,
    feed: Option<WorkerFeed>,
}

#[derive(Deserialize)]
struct WorkerFeed {
    items: Vec<(i64, String)>,
    checked: Vec<i64>,
    valid_locations: Vec<i64>,
    player: i64,
    connect: WorkerConnect,
}

#[derive(Deserialize)]
struct WorkerConnect {
    slot_data: Option<serde_json::Value>,
    seed: Option<String>,
    players: Vec<(i64, String, String)>,
}

/// A worker's answer: the result, or the error to show.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Reply {
    Ok(serde_json::Value),
    Error(String),
}

/// Run a pack worker: this same program, in the sandbox, seeing only the pack
/// and the request, and killed if it runs too long.
async fn run_worker(
    scratch: &Scratch,
    pack: &Path,
    request: &Request<'_>,
) -> Result<serde_json::Value, String> {
    let exe = std::env::current_exe().map_err(|e| format!("own path: {e}"))?;
    let file = scratch.0.join("request.json");
    let json = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    tokio::fs::write(&file, json)
        .await
        .map_err(|e| format!("write temp: {e}"))?;
    let job = Job {
        program: &exe,
        args: vec![WORKER_COMMAND.into(), file.clone().into_os_string()],
        cwd: pack,
        env: Vec::new(),
        read: vec![exe.clone(), pack.to_path_buf(), file],
        write: Vec::new(),
        network: false,
    };
    let cmd = sandbox::get().command(&job);
    let _permit = WORKERS.acquire().await.map_err(|e| e.to_string())?;
    let timeout = worker_timeout();
    let out = sandbox::run(cmd, timeout)
        .await
        .map_err(|e| format!("the pack didn't finish: {e}"))?;
    if !out.status.success() {
        return Err(format!("the pack's code crashed ({})", out.status));
    }
    match serde_json::from_slice(&out.stdout) {
        Ok(Reply::Ok(v)) => Ok(v),
        Ok(Reply::Error(e)) => Err(e),
        Err(e) => Err(format!("the pack's worker answered badly: {e}")),
    }
}

/// The hidden command that runs a pack worker (see [`run_worker`]).
pub const WORKER_COMMAND: &str = "pack-worker";

/// The worker process. Only the reply goes to stdout: a pack's `print` must not
/// be able to write to it, so the real stdout moves aside and fd 1 goes nowhere.
pub fn worker_main(request: &Path) -> i32 {
    use std::io::Write;
    use std::os::fd::FromRawFd;
    // SAFETY: plain descriptor calls, before anything else runs.
    let out = unsafe {
        let out = libc::dup(1);
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        if out < 0 || null < 0 || libc::dup2(null, 1) < 0 {
            return 2;
        }
        libc::close(null);
        std::fs::File::from_raw_fd(out)
    };
    let reply = match worker_reply(request) {
        Ok(v) => match serde_json::to_vec(&Reply::Ok(v)) {
            Ok(j) if j.len() <= MAX_REPLY => j,
            _ => too_big(),
        },
        Err(e) => serde_json::to_vec(&Reply::Error(e)).unwrap_or_else(|_| too_big()),
    };
    let mut out = out;
    if out.write_all(&reply).is_err() {
        return 2;
    }
    0
}

fn too_big() -> Vec<u8> {
    br#"{"error":"the pack's state is too large"}"#.to_vec()
}

fn worker_reply(request: &Path) -> Result<serde_json::Value, String> {
    let text = std::fs::read(request).map_err(|e| format!("read request: {e}"))?;
    let req: WorkerRequest = serde_json::from_slice(&text).map_err(|e| e.to_string())?;
    let pack = poptracker::Pack::load(&req.pack).map_err(|e| e.to_string())?;
    let session = poptracker::Session::new(&pack).map_err(|e| e.to_string())?;
    let Some(feed) = req.feed else {
        return serde_json::to_value(session.summary()).map_err(|e| e.to_string());
    };
    session.set_player(feed.player);
    let players: Vec<(i64, &str, &str)> = feed
        .connect
        .players
        .iter()
        .map(|(slot, name, game)| (*slot, name.as_str(), game.as_str()))
        .collect();
    session
        .set_multiworld(feed.connect.seed.as_deref(), &players)
        .map_err(|e| e.to_string())?;
    // Feed the slot's valid location ids before onClear so `visibility_rules`
    // can hide markers for checks this slot's options never generated. Empty =>
    // left disabled (every marker shows), so a missing/old meta can't blank maps.
    if !feed.valid_locations.is_empty() {
        session.set_valid_locations(&feed.valid_locations);
    }
    let slot_data = clear_data(feed.connect.slot_data.as_ref());
    session.on_clear(&slot_data).map_err(|e| e.to_string())?;
    for (i, (id, name)) in feed.items.iter().enumerate() {
        session
            .on_item(i as i64, *id, name, feed.player)
            .map_err(|e| e.to_string())?;
    }
    // Feed checked locations so the pack's `onLocation` marks sections done
    // (→ grey "Done" markers). The name is unused by AP packs' handlers.
    for id in &feed.checked {
        session.on_location(*id, "").map_err(|e| e.to_string())?;
    }
    serde_json::to_value(session.state()).map_err(|e| e.to_string())
}

/// The slot data to pass to `onClear`: AP always sends an object, so anything
/// else (unknown, or metadata from before slot data was kept) becomes `{}`.
fn clear_data(slot_data: Option<&serde_json::Value>) -> serde_json::Value {
    match slot_data {
        Some(v @ serde_json::Value::Object(_)) => v.clone(),
        _ => serde_json::json!({}),
    }
}

/// A new, unguessable, owner-only temp dir, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Scratch, String> {
        use std::os::unix::fs::DirBuilderExt;
        let dir =
            std::env::temp_dir().join(format!("bifrost-pack-{}", &crate::auth::new_token()[..32]));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("temp dir: {e}"))?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Read a pack asset (e.g. a map image) for serving to the browser.
pub fn read_pack_asset(season: i64, ap_slot: i64, rel: &str) -> Option<Vec<u8>> {
    let root = poptracker::pack_root(&slot_dir(season, ap_slot)).ok()?;
    let pack = poptracker::Pack::load(&root).ok()?;
    pack.asset(rel)
}

/// Write an uploaded pack to a new, unguessable, owner-only file in the temp
/// dir (created fresh, so nothing another local user prepared is followed).
fn write_temp(tag: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = std::env::temp_dir().join(format!(
        "bifrost-pack-{tag}-{}.zip",
        &crate::auth::new_token()[..32]
    ));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("write temp: {e}"))?;
    if let Err(e) = f.write_all(bytes) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("write temp: {e}"));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(json: serde_json::Value) -> crate::meta::Meta {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn connect_passes_the_slot_data_or_an_empty_object() {
        let m = meta(serde_json::json!({
            "seed": "s1",
            "slots": {
                "1": {"name": "Ann", "game": "A", "slot_data": {"k": 1}},
                "2": {"name": "Bo", "game": "B"},
                "3": {"name": "Cy", "game": "C", "slot_data": [1, 2]}
            }
        }));
        let c = Connect::from_meta(&m, 1);
        assert_eq!(clear_data(c.slot_data), serde_json::json!({"k": 1}));
        assert_eq!(c.seed, Some("s1"));
        assert_eq!(c.players, [(1, "Ann", "A"), (2, "Bo", "B"), (3, "Cy", "C")]);
        // Metadata from before slot data was kept, a non-object, an unknown slot.
        for slot in [2, 3, 9] {
            assert_eq!(
                clear_data(Connect::from_meta(&m, slot).slot_data),
                serde_json::json!({})
            );
        }
        assert_eq!(clear_data(None), serde_json::json!({}));
    }
}
