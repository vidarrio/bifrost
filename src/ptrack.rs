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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
    let permit = Arc::new(worker_permit().await.map_err(|Busy| BUSY.to_string())?);
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
    let request = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
    let reply = run_worker(&scratch, &root, request)
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
fn install(season: i64, ap_slot: i64, zip_bytes: &[u8]) -> Result<(), String> {
    SLOTS.lock().unwrap().remove(&(season, ap_slot));
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

/// [`install`] as a worker's equal: it can take a while, and holds a permit for it.
pub async fn install_pack(season: i64, ap_slot: i64, bytes: Arc<Vec<u8>>) -> Result<(), String> {
    let permit = worker_permit().await.map_err(|Busy| BUSY.to_string())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        install(season, ap_slot, &bytes)
    })
    .await
    .map_err(|_| "Upload failed.".to_string())?
}

/// Remove a slot's installed pack.
pub fn remove(season: i64, ap_slot: i64) {
    SLOTS.lock().unwrap().remove(&(season, ap_slot));
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

/// The tracker state for a slot from its installed pack + what's fed to it, as
/// JSON text ([`poptracker::TrackerState`]). Viewers share it: a slot runs its
/// pack once per change, in a task no request can cancel, and polls in between
/// are answered from the last result. Runs are paced by how long the last took,
/// and a pack that hit a limit is held back.
pub async fn compute(
    season: i64,
    ap_slot: i64,
    version: u64,
    feed: Feed<'_>,
) -> Result<Arc<String>, ComputeError> {
    use sha2::{Digest, Sha256};
    let key = (season, ap_slot);
    let root = poptracker::pack_root(&slot_dir(season, ap_slot))
        .map_err(|e| ComputeError::Failed(e.to_string()))?;
    let request = Request {
        pack: root.clone(),
        feed: Some(feed),
    };
    let request = serde_json::to_vec(&request).map_err(|e| ComputeError::Failed(e.to_string()))?;
    let hash: [u8; 32] = Sha256::digest(&request).into();
    let (run, done) = {
        let mut slots = SLOTS.lock().unwrap();
        let now = Instant::now();
        slots.retain(|_, s| s.running || now.duration_since(s.polled) < FORGET_AFTER);
        let slot = slots.entry(key).or_insert_with(|| Slot::new(version, now));
        if slot.version != version {
            *slot = Slot::new(version, now);
        }
        slot.polled = now;
        if let Some((until, message)) = &slot.held
            && now < *until
        {
            return Err(ComputeError::Failed(message.clone()));
        }
        if let Some((h, result)) = &slot.last
            && *h == hash
        {
            return result.clone().map_err(ComputeError::Failed);
        }
        if slot.running || now < slot.next_run {
            return match &slot.last {
                Some((_, result)) => result.clone().map_err(ComputeError::Failed),
                None => Err(ComputeError::Busy),
            };
        }
        slot.running = true;
        slot.run = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = tokio::sync::oneshot::channel();
        (
            RunOf {
                key,
                id: slot.run,
                hash,
                tx,
            },
            rx,
        )
    };
    tokio::spawn(async move {
        let started = Instant::now();
        let outcome = tokio::spawn(run_compute(root, request))
            .await
            .unwrap_or_else(|e| Err(Failure::Host(format!("worker task: {e}"))));
        let answer = run.finish(outcome, started.elapsed());
        let _ = run.tx.send(answer);
    });
    done.await
        .unwrap_or_else(|_| Err(ComputeError::Failed("compute failed".into())))
}

async fn run_compute(root: PathBuf, request: Vec<u8>) -> Result<String, Failure> {
    let _permit = worker_permit().await.map_err(|Busy| Failure::Busy)?;
    let scratch = Scratch::new().map_err(Failure::Host)?;
    let reply = run_worker(&scratch, &root, request).await?;
    Ok(reply.to_string())
}

/// One run's place in its slot.
struct RunOf {
    key: (i64, i64),
    id: u64,
    hash: [u8; 32],
    tx: tokio::sync::oneshot::Sender<Result<Arc<String>, ComputeError>>,
}

impl RunOf {
    /// Record how the run ended, unless the slot has moved on (a new pack), and
    /// say what to tell the request that started it.
    fn finish(
        &self,
        outcome: Result<String, Failure>,
        took: Duration,
    ) -> Result<Arc<String>, ComputeError> {
        let now = Instant::now();
        let mut slots = SLOTS.lock().unwrap();
        let mut slot = slots.get_mut(&self.key).filter(|s| s.run == self.id);
        if let Some(s) = slot.as_deref_mut() {
            s.running = false;
            s.next_run = now + pace_after(took);
        }
        match outcome {
            Ok(state) => {
                let state = Arc::new(state);
                if let Some(s) = slot {
                    s.strikes = 0;
                    s.held = None;
                    s.last = Some((self.hash, Ok(state.clone())));
                }
                Ok(state)
            }
            Err(Failure::Pack(message)) => {
                if let Some(s) = slot {
                    s.strikes = 0;
                    s.held = None;
                    s.last = Some((self.hash, Err(message.clone())));
                }
                Err(ComputeError::Failed(message))
            }
            Err(Failure::Limit(message)) => {
                if let Some(s) = slot {
                    s.strikes += 1;
                    s.held = Some((now + hold_for(s.strikes), message.clone()));
                }
                Err(ComputeError::Failed(message))
            }
            Err(Failure::Busy) => Err(ComputeError::Busy),
            Err(f @ Failure::Host(_)) => Err(ComputeError::Failed(f.message())),
        }
    }
}

/// Why [`compute`] gave no state.
pub enum ComputeError {
    /// Not now; the page asks again shortly.
    Busy,
    Failed(String),
}

/// A finished run's state, or the pack's error.
type Outcome = Result<Arc<String>, String>;

/// What's known of one slot's tracker.
struct Slot {
    version: u64,
    polled: Instant,
    /// The current run, which alone may record its result.
    run: u64,
    running: bool,
    next_run: Instant,
    /// The last result, and a hash of the request that gave it.
    last: Option<([u8; 32], Outcome)>,
    /// Limits hit in a row, and until when it's held back, with why.
    strikes: u32,
    held: Option<(Instant, String)>,
}

impl Slot {
    fn new(version: u64, now: Instant) -> Slot {
        Slot {
            version,
            polled: now,
            run: 0,
            running: false,
            next_run: now,
            last: None,
            strikes: 0,
            held: None,
        }
    }
}

static SLOTS: Mutex<BTreeMap<(i64, i64), Slot>> = Mutex::new(BTreeMap::new());
static NEXT_RUN: AtomicU64 = AtomicU64::new(1);

/// A slot nobody has looked at for this long gives up its result.
const FORGET_AFTER: Duration = Duration::from_secs(10 * 60);

/// How long a pack is held back after hitting a limit `strikes` times in a row:
/// briefly at first, as a load spike can do it to a good pack.
fn hold_for(strikes: u32) -> Duration {
    Duration::from_secs(60 << strikes.saturating_sub(1).min(6))
}

/// How long a slot waits before its next run, after one that took `took`: at
/// least a moment, and long enough that a pack runs at most a quarter of the
/// time, however many poll.
fn pace_after(took: Duration) -> Duration {
    (took * 3).max(Duration::from_secs(2))
}

/// One pack upload per account at a time. `None` when one is already running;
/// dropping the claim ends it.
pub fn claim_upload(account: i64) -> Option<Claim> {
    Claim::take(&UPLOADING, (account, 0))
}

type Claims = Mutex<BTreeSet<(i64, i64)>>;
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

const BUSY: &str = "the server is busy running other packs; try again shortly";

struct Busy;

async fn worker_permit() -> Result<SemaphorePermit<'static>, Busy> {
    struct Waiting;
    impl Drop for Waiting {
        fn drop(&mut self) {
            WAITING.fetch_sub(1, Ordering::Relaxed);
        }
    }
    let waiting = WAITING.fetch_add(1, Ordering::Relaxed);
    let _waiting = Waiting;
    if waiting >= MAX_WAITING {
        return Err(Busy);
    }
    match tokio::time::timeout(worker_timeout(), WORKERS.acquire()).await {
        Ok(Ok(permit)) => Ok(permit),
        _ => Err(Busy),
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
    /// No worker free.
    Busy,
}

impl Failure {
    fn message(self) -> String {
        match self {
            Failure::Pack(m) | Failure::Limit(m) => m,
            Failure::Busy => BUSY.into(),
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
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let exe = std::env::current_exe().map_err(|e| format!("own path: {e}"))?;
    Ok(
        match exe.as_os_str().as_bytes().strip_suffix(b" (deleted)") {
            Some(path) => std::ffi::OsString::from_vec(path.to_vec()).into(),
            None => exe,
        },
    )
}

/// Run a pack worker: this same program, in the sandbox, seeing only the pack
/// and the request, and killed if it runs too long or says too much. The caller
/// holds a permit.
async fn run_worker(
    scratch: &Scratch,
    pack: &Path,
    request: Vec<u8>,
) -> Result<serde_json::Value, Failure> {
    let exe = own_exe().map_err(Failure::Host)?;
    let file = scratch.0.join("request.json");
    tokio::fs::write(&file, request)
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
        let what = format!("{}: {}", out.status, capped(&stderr));
        if !brought_down(&out.status) {
            return Err(Failure::Host(what));
        }
        tracing::warn!("pack worker: {what}");
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

/// Did a worker die running (a signal, or a panic) rather than fail to start?
/// Bubblewrap passes a signal on as 128 + its number, and exits 1 when it can't
/// run the program, as a shell's 126 and 127 mean the same.
fn brought_down(status: &std::process::ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    status.signal().is_some() || matches!(status.code(), Some(101 | 128..))
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
    fn holds_grow_with_repeats_and_runs_are_paced() {
        assert_eq!(hold_for(1), Duration::from_secs(60));
        assert_eq!(hold_for(2), Duration::from_secs(120));
        assert_eq!(hold_for(100), Duration::from_secs(3840));
        assert_eq!(
            pace_after(Duration::from_millis(100)),
            Duration::from_secs(2)
        );
        assert_eq!(pace_after(Duration::from_secs(10)), Duration::from_secs(30));
    }

    #[test]
    fn only_a_running_workers_death_is_the_packs_doing() {
        use std::os::unix::process::ExitStatusExt;
        let exit = |code: i32| std::process::ExitStatus::from_raw(code << 8);
        for code in [1, 2, 126, 127] {
            assert!(!brought_down(&exit(code)), "{code}");
        }
        for code in [101, 134, 137] {
            assert!(brought_down(&exit(code)), "{code}");
        }
        assert!(brought_down(&std::process::ExitStatus::from_raw(9)));
    }

    #[test]
    fn a_claim_is_held_until_dropped() {
        let first = claim_upload(-1).expect("free");
        assert!(claim_upload(-1).is_none(), "taken");
        assert!(claim_upload(-1).is_none(), "still taken after a refusal");
        drop(first);
        assert!(claim_upload(-1).is_some(), "free again");
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
