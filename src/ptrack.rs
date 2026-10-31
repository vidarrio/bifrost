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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, SemaphorePermit};

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
pub async fn validate(zip_bytes: Arc<Vec<u8>>) -> Result<poptracker::Summary, String> {
    let permit = Arc::new(worker_permit().await?);
    let scratch = Arc::new(Scratch::new()?);
    // Unpacking is the host's own code; only running the pack's Lua is isolated.
    // The task owns what it uses: it runs on if this request goes away, and the
    // scratch dir and the permit last until it's done.
    let root = {
        let (scratch, permit) = (scratch.clone(), permit.clone());
        tokio::task::spawn_blocking(move || {
            let _keep = (&scratch, permit);
            let zip = scratch.0.join("pack.zip");
            std::fs::write(&zip, zip_bytes.as_slice()).map_err(|e| format!("write temp: {e}"))?;
            let unpacked = scratch.0.join("pack");
            poptracker::extract_zip(&zip, &unpacked).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(&zip);
            poptracker::pack_root(&unpacked).map_err(|e| e.to_string())
        })
        .await
        .map_err(|_| "validate failed".to_string())??
    };
    let request = Request {
        pack: root.clone(),
        feed: None,
    };
    let reply = run_worker(&scratch, &root, &request)
        .await
        .map_err(Failure::message)?;
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
    HELD_BACK.lock().unwrap().remove(&(season, ap_slot));
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
/// to it, as JSON ([`poptracker::TrackerState`]). A pack that hit a limit isn't
/// run again for a while: viewers' polls would restart it without end.
pub async fn compute(
    season: i64,
    ap_slot: i64,
    version: u64,
    feed: Feed<'_>,
) -> Result<serde_json::Value, String> {
    let key = (season, ap_slot);
    if let Some(f) = HELD_BACK.lock().unwrap().get(&key)
        && f.version == version
        && f.at.elapsed() < HOLD_BACK
    {
        return Err(f.message.clone());
    }
    let root = poptracker::pack_root(&slot_dir(season, ap_slot)).map_err(|e| e.to_string())?;
    let _permit = worker_permit().await?;
    let scratch = Scratch::new()?;
    let request = Request {
        pack: root.clone(),
        feed: Some(feed),
    };
    match run_worker(&scratch, &root, &request).await {
        Err(Failure::Limit(message)) => {
            let held = Held {
                version,
                at: Instant::now(),
                message: message.clone(),
            };
            HELD_BACK.lock().unwrap().insert(key, held);
            Err(message)
        }
        r => r.map_err(Failure::message),
    }
}

/// A pack that hit a limit, and when, for this long.
const HOLD_BACK: Duration = Duration::from_secs(5 * 60);

struct Held {
    version: u64,
    at: Instant,
    message: String,
}

static HELD_BACK: Mutex<BTreeMap<(i64, i64), Held>> = Mutex::new(BTreeMap::new());

/// One compute per slot at a time, so viewers of a slot can't pile up workers.
/// `None` when one is already running; dropping the claim ends it.
pub fn claim_compute(season: i64, ap_slot: i64) -> Option<Claim> {
    Claim::take(&COMPUTING, (season, ap_slot))
}

/// One pack upload per account at a time.
pub fn claim_upload(account: i64) -> Option<Claim> {
    Claim::take(&UPLOADING, (account, 0))
}

type Claims = Mutex<BTreeSet<(i64, i64)>>;
static COMPUTING: Claims = Mutex::new(BTreeSet::new());
static UPLOADING: Claims = Mutex::new(BTreeSet::new());

pub struct Claim(&'static Claims, (i64, i64));

impl Claim {
    fn take(set: &'static Claims, key: (i64, i64)) -> Option<Claim> {
        // Not `then_some`: it would build a claim even when refused, whose drop
        // would give up the holder's claim, and with the lock still held.
        let free = set.lock().unwrap().insert(key);
        free.then(|| Claim(set, key))
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.0.lock().unwrap().remove(&self.1);
    }
}

/// Pack workers running at once, across all slots and uploads, and how many
/// may wait for one. Past that, or after a while in line, the answer is "busy".
static WORKERS: Semaphore = Semaphore::const_new(4);
static WAITING: AtomicUsize = AtomicUsize::new(0);
const MAX_WAITING: usize = 16;

async fn worker_permit() -> Result<SemaphorePermit<'static>, String> {
    const BUSY: &str = "the server is busy running other packs; try again shortly";
    struct Waiting;
    impl Drop for Waiting {
        fn drop(&mut self) {
            WAITING.fetch_sub(1, Ordering::Relaxed);
        }
    }
    let waiting = WAITING.fetch_add(1, Ordering::Relaxed);
    let _waiting = Waiting;
    if waiting >= MAX_WAITING {
        return Err(BUSY.into());
    }
    match tokio::time::timeout(worker_timeout(), WORKERS.acquire()).await {
        Ok(Ok(permit)) => Ok(permit),
        _ => Err(BUSY.into()),
    }
}

/// How long one pack worker may run. Pack Lua is untrusted, and not everything
/// it can do is seen by the engine's instruction cap, so this is the real limit.
fn worker_timeout() -> Duration {
    let secs = config::var("BIFROST_PACK_TIMEOUT_SECS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    Duration::from_secs(secs)
}

/// The most a worker may send back, and the longest error it may report.
const MAX_REPLY: usize = 16 << 20;
const MAX_ERROR: usize = 2000;

/// The worker's exit code for a state over `MAX_REPLY`.
const EXIT_TOO_BIG: i32 = 3;

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

/// Why a worker gave no answer.
enum Failure {
    /// The pack reported an error.
    Pack(String),
    /// The pack hit a limit: too long, too large, or it brought the worker down.
    Limit(String),
    /// The worker couldn't be run; not the pack's doing.
    Host(String),
}

impl Failure {
    fn message(self) -> String {
        match self {
            Failure::Pack(m) | Failure::Limit(m) => m,
            Failure::Host(m) => {
                tracing::warn!("pack worker: {m}");
                "Bifrost couldn't run the pack; the server's log says why".into()
            }
        }
    }
}

/// The running program's file. After an update replaced it, the kernel's name
/// for it ends in " (deleted)"; the file at that path is then the new version,
/// which speaks the same request format.
fn own_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("own path: {e}"))?;
    let name = exe.as_os_str().to_string_lossy();
    Ok(match name.strip_suffix(" (deleted)") {
        Some(path) => PathBuf::from(path),
        None => exe,
    })
}

/// Run a pack worker: this same program, in the sandbox, seeing only the pack
/// and the request, and killed if it runs too long or says too much. The caller
/// holds a permit.
async fn run_worker(
    scratch: &Scratch,
    pack: &Path,
    request: &Request<'_>,
) -> Result<serde_json::Value, Failure> {
    let exe = own_exe().map_err(Failure::Host)?;
    let file = scratch.0.join("request.json");
    let json = serde_json::to_vec(request).map_err(|e| Failure::Host(e.to_string()))?;
    tokio::fs::write(&file, json)
        .await
        .map_err(|e| Failure::Host(format!("write temp: {e}")))?;
    let job = Job {
        program: &exe,
        args: vec![WORKER_COMMAND.into(), file.clone().into_os_string()],
        cwd: pack,
        env: Vec::new(),
        read: vec![exe.clone(), pack.to_path_buf(), file],
        write: Vec::new(),
        network: false,
    };
    let cmd = sandbox::get().pack_worker(&job);
    let timeout = worker_timeout();
    let out = match sandbox::run_capped(cmd, timeout, MAX_REPLY).await {
        Ok(out) => out,
        Err(sandbox::CappedError::TimedOut) => {
            return Err(Failure::Limit(format!(
                "the pack didn't finish within {} seconds",
                timeout.as_secs()
            )));
        }
        Err(sandbox::CappedError::TooMuch) => {
            return Err(Failure::Limit("the pack's state is too large".into()));
        }
        Err(sandbox::CappedError::Run(e)) => return Err(Failure::Host(e)),
    };
    if out.status.code() == Some(EXIT_TOO_BIG) {
        return Err(Failure::Limit("the pack's state is too large".into()));
    }
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        tracing::warn!("pack worker: {}: {}", out.status, capped(&stderr));
        return Err(Failure::Limit(
            "the pack's code stopped the worker: it may have run out of memory".into(),
        ));
    }
    match serde_json::from_slice(&out.stdout) {
        Ok(Reply::Ok(v)) => Ok(v),
        Ok(Reply::Error(e)) => Err(Failure::Pack(capped(&e))),
        Err(e) => Err(Failure::Limit(format!(
            "the pack's worker answered badly: {e}"
        ))),
    }
}

/// At most `MAX_ERROR` bytes of an error, which a pack can make as long as it
/// likes.
fn capped(e: &str) -> String {
    if e.len() <= MAX_ERROR {
        return e.to_string();
    }
    let mut end = MAX_ERROR;
    while !e.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &e[..end])
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
        Ok(v) => Reply::Ok(v),
        Err(e) => Reply::Error(capped(&e)),
    };
    let reply = match serde_json::to_vec(&reply) {
        Ok(j) if j.len() <= MAX_REPLY => j,
        _ => return EXIT_TOO_BIG,
    };
    let mut out = out;
    if out.write_all(&reply).is_err() {
        return 2;
    }
    0
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

/// Where scratch dirs go: in the data dir's `tmp` (which backups skip), not the
/// system's temp dir, which is often in memory.
fn scratch_root() -> PathBuf {
    config::get().data_dir.join("tmp").join("packs")
}

/// A new, unguessable, owner-only dir for one worker, removed when dropped (off
/// the async runtime when on it, as a pack can hold many files).
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Scratch, String> {
        use std::os::unix::fs::DirBuilderExt;
        let dir = scratch_root().join(&crate::auth::new_token()[..32]);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("temp dir: {e}"))?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let dir = std::mem::take(&mut self.0);
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn_blocking(move || std::fs::remove_dir_all(dir));
            }
            Err(_) => {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }
}

/// Remove scratch dirs a previous run left behind. At startup.
pub fn sweep_scratch() {
    let _ = std::fs::remove_dir_all(scratch_root());
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
    fn a_claim_is_held_until_dropped() {
        let first = claim_compute(-1, -1).expect("free");
        assert!(claim_compute(-1, -1).is_none(), "taken");
        assert!(
            claim_compute(-1, -1).is_none(),
            "still taken after a refusal"
        );
        assert!(claim_upload(-1).is_some(), "uploads are a separate set");
        drop(first);
        assert!(claim_compute(-1, -1).is_some(), "free again");
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
