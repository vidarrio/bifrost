//! A live season's Archipelago server, run by Bifrost (design doc §7.2–7.3):
//! one per live season, each on the port the season holds.
//!
//! A supervisor task owns the MultiServer process: it starts it (sandboxed,
//! with network) on the season's `output/current.zip`, restarts it with
//! backoff if it dies, captures its output into `logs/server.log` and the
//! activity feed, and handles redeploys and shutdowns in the right order —
//! the server is stopped (saving) *before* its save is wiped and the new seed
//! copied in, so a dying server can never write the old seed's hints/checks
//! into the new world's save.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{mpsc, oneshot};

use crate::ap::Board;
use crate::config::{self, SeasonDirs};
use crate::db::Db;
use crate::sandbox::{self, Job};

/// How long a server gets to save and exit after `/exit` before it's killed.
const STOP_GRACE: Duration = Duration::from_secs(15);

enum Cmd {
    /// Stop, swap in this seed zip (and a fresh save), start.
    Redeploy(PathBuf, oneshot::Sender<Result<(), String>>),
    /// A server console command (e.g. `/option password …`).
    Console(String),
    /// Stop for good (saving), then end the supervisor.
    Stop(oneshot::Sender<()>),
}

/// Handle to a season's supervised AP server.
pub struct ApServer {
    tx: mpsc::Sender<Cmd>,
}

/// How to launch the season's server.
#[derive(Clone)]
pub struct Launch {
    pub season: i64,
    /// The season's pinned port (from `BIFROST_AP_PORTS`).
    pub port: u16,
    pub password: String,
    pub admin_password: String,
}

impl ApServer {
    pub fn spawn(launch: Launch, db: Db, board: Arc<RwLock<Board>>) -> ApServer {
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(supervise(launch, db, board, rx));
        ApServer { tx }
    }

    /// Replace the deployed seed (stop → swap zip, wipe save → start).
    pub async fn redeploy(&self, seed_zip: PathBuf) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Redeploy(seed_zip, tx))
            .await
            .map_err(|_| "AP server supervisor is gone".to_string())?;
        rx.await
            .map_err(|_| "AP server supervisor is gone".to_string())?
    }

    pub async fn console(&self, cmd: &str) {
        let _ = self.tx.send(Cmd::Console(cmd.to_string())).await;
    }

    /// Stop the server (it saves on the way out) and the supervisor.
    pub async fn stop(&self) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Cmd::Stop(tx)).await.is_ok() {
            let _ = rx.await;
        }
    }
}

struct Running {
    child: Child,
    stdin: ChildStdin,
}

async fn supervise(launch: Launch, db: Db, board: Arc<RwLock<Board>>, mut rx: mpsc::Receiver<Cmd>) {
    let season = launch.season;
    let dirs = config::get().season(season);
    let mut running: Option<Running> = None;
    let mut backoff = Duration::from_secs(2);
    // The last start attempt failed (e.g. the port is still held): retry on
    // the backoff rather than wait for a redeploy that may never come.
    let mut failed = false;
    loop {
        if running.is_none() && dirs.current_zip().exists() {
            match start(&launch, &dirs, &db, &board) {
                Ok(r) => {
                    running = Some(r);
                    failed = false;
                }
                Err(e) => {
                    tracing::warn!(
                        "season {season}: AP server didn't start: {e}; retrying in {}s",
                        backoff.as_secs()
                    );
                    failed = true;
                }
            }
        }
        let cmd = match running.as_mut() {
            Some(r) => tokio::select! {
                cmd = rx.recv() => cmd,
                status = r.child.wait() => {
                    tracing::warn!("season {season}: AP server exited ({status:?}); restarting in {}s", backoff.as_secs());
                    running = None;
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                    continue;
                }
            },
            None if failed => tokio::select! {
                cmd = rx.recv() => cmd,
                _ = tokio::time::sleep(backoff) => {
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                    continue;
                }
            },
            // Nothing deployed yet: wait for a redeploy (or stop).
            None => rx.recv().await,
        };
        match cmd {
            Some(Cmd::Redeploy(zip, reply)) => {
                if let Some(r) = running.take() {
                    stop(season, r).await;
                }
                let r = swap_seed(&dirs, &zip).await;
                backoff = Duration::from_secs(2);
                failed = false;
                let _ = reply.send(r);
            }
            Some(Cmd::Console(line)) => {
                if let Some(r) = running.as_mut() {
                    let _ = r.stdin.write_all(format!("{line}\n").as_bytes()).await;
                    let _ = r.stdin.flush().await;
                }
            }
            Some(Cmd::Stop(reply)) => {
                if let Some(r) = running.take() {
                    stop(season, r).await;
                }
                let _ = reply.send(());
                return;
            }
            None => {
                // Every handle dropped: shut down.
                if let Some(r) = running.take() {
                    stop(season, r).await;
                }
                return;
            }
        }
    }
}

/// Deploy `zip` as the season's `current.zip` with a fresh save. Only called
/// while no server is running, so nothing can re-write the old save.
async fn swap_seed(dirs: &SeasonDirs, zip: &std::path::Path) -> Result<(), String> {
    let _ = tokio::fs::remove_file(dirs.current_save()).await;
    let dst = dirs.current_zip();
    let _ = tokio::fs::remove_file(&dst).await;
    tokio::fs::copy(zip, &dst)
        .await
        .map(|_| ())
        .map_err(|e| format!("deploy: {e}"))
}

/// Ask the server to save and exit (`/exit` on its console); kill it if it
/// doesn't within the grace period.
async fn stop(season: i64, mut r: Running) {
    let _ = r.stdin.write_all(b"/exit\n").await;
    let _ = r.stdin.flush().await;
    if tokio::time::timeout(STOP_GRACE, r.child.wait())
        .await
        .is_err()
    {
        tracing::warn!("season {season}: AP server didn't exit in time; killing it");
        let _ = r.child.kill().await;
    }
}

fn start(
    launch: &Launch,
    dirs: &SeasonDirs,
    db: &Db,
    board: &Arc<RwLock<Board>>,
) -> Result<Running, String> {
    let cfg = config::get();
    // A server left over from a crashed Bifrost (or anything else) still on
    // the port would make MultiServer die on bind; say so plainly instead.
    if let Err(e) = std::net::TcpListener::bind((cfg.ap_bind.as_str(), launch.port)) {
        return Err(format!("port {} isn't free ({e})", launch.port));
    }
    let args: Vec<OsString> = vec![
        cfg.script("bifrost_server.py").into(),
        dirs.current_zip().into(),
        "--host".into(),
        cfg.ap_bind.clone().into(),
        "--port".into(),
        launch.port.to_string().into(),
        "--password".into(),
        launch.password.clone().into(),
        "--server_password".into(),
        launch.admin_password.clone().into(),
    ];
    let job = Job {
        program: &cfg.ap_python,
        args,
        cwd: &dirs.root,
        env: vec![
            ("BIFROST_AP_DIR", cfg.ap_dir.clone().into_os_string()),
            ("BIFROST_SEASON_DIR", dirs.root.clone().into_os_string()),
            ("PYTHONDONTWRITEBYTECODE", "1".into()),
            ("PYTHONUNBUFFERED", "1".into()),
        ],
        read: vec![
            cfg.ap_dir.clone(),
            cfg.runtime_dir.join("scripts"),
            dirs.root.clone(),
        ],
        // The save lives next to the multidata; AP writes its own logs too.
        write: vec![dirs.output.clone(), dirs.logs.clone()],
        // It's a server: it listens for players.
        network: true,
    };
    let mut cmd = sandbox::get().service(&job);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let log = dirs.logs.join("server.log");
    for stream in [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
    ]
    .into_iter()
    .flatten()
    {
        tokio::spawn(capture(
            launch.season,
            stream,
            log.clone(),
            db.clone(),
            board.clone(),
        ));
    }
    tracing::info!(
        "season {}: AP server started on {}:{}",
        launch.season,
        cfg.ap_bind,
        launch.port
    );
    Ok(Running { child, stdin })
}

/// Append the server's output to `logs/server.log` and feed gameplay lines to
/// the activity log.
async fn capture(
    season: i64,
    stream: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
    log: PathBuf,
    db: Db,
    board: Arc<RwLock<Board>>,
) {
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .await
        .ok();
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let ts = crate::auth::now();
        if let Some(f) = file.as_mut() {
            let _ = f.write_all(format!("{ts} {line}\n").as_bytes()).await;
        }
        crate::activity::ingest_line(season, &board, &db, line.trim(), ts).await;
    }
}
