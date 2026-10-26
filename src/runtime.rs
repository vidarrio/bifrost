//! Per-season runtime state (design doc §7.4). Every season has its own live
//! board and generation status; the **live** season also runs its AP server
//! (when Bifrost manages it) and the background workers — AP tracker + counts
//! refresher, in-logic recomputer, and (for an external server) the journal
//! ingester — started when it goes live and stopped when it's archived.
//!
//! Managed servers each get the port their season holds (§7.3); with an
//! external server only one season can be live.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use tokio::task::JoinHandle;

use crate::ap::{ApConfig, Board};
use crate::apserver::{ApServer, Launch};
use crate::config;
use crate::db::Db;
use crate::generate::GenState;

pub struct SeasonRt {
    pub board: Arc<RwLock<Board>>,
    pub genstate: GenState,
    /// How to reach the season's AP server (set while live).
    ap: RwLock<Option<ApConfig>>,
    server: Mutex<Option<Arc<ApServer>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl SeasonRt {
    /// The AP connection for this season, if it's live and has one.
    pub fn ap_cfg(&self) -> Option<ApConfig> {
        self.ap.read().ok().and_then(|c| c.clone())
    }
    fn server(&self) -> Option<Arc<ApServer>> {
        self.server.lock().unwrap().clone()
    }
}

pub struct Runtimes {
    db: Db,
    /// The external AP server's connection (`BIFROST_AP_MODE=external`).
    external: Option<ApConfig>,
    coop: Vec<(String, String)>,
    map: Mutex<HashMap<i64, Arc<SeasonRt>>>,
}

impl Runtimes {
    pub fn new(db: Db, external: Option<ApConfig>, coop: Vec<(String, String)>) -> Self {
        Self {
            db,
            external,
            coop,
            map: Mutex::default(),
        }
    }

    /// The season's runtime (created on first use).
    pub fn get(&self, id: i64) -> Arc<SeasonRt> {
        self.map
            .lock()
            .unwrap()
            .entry(id)
            .or_insert_with(|| {
                Arc::new(SeasonRt {
                    board: Arc::new(RwLock::new(Board::default())),
                    genstate: Arc::new(Mutex::new((false, "idle".into()))),
                    ap: RwLock::default(),
                    server: Mutex::default(),
                    workers: Mutex::default(),
                })
            })
            .clone()
    }

    /// Bring a live season up: its AP server (managed mode) and workers.
    /// Idempotent.
    pub async fn start_live(&self, id: i64) {
        let rt = self.get(id);
        if !rt.workers.lock().unwrap().is_empty() || rt.server().is_some() {
            return;
        }
        let cfg = config::get();
        let port = self
            .db
            .season(id)
            .await
            .and_then(|s| s.ap_port)
            .and_then(|p| u16::try_from(p).ok());
        let ap = if cfg.ap_managed {
            let Some(port) = port else {
                tracing::error!("season {id}: live but holds no AP port; not starting its server");
                return;
            };
            let (password, admin_password) = self.db.ap_passwords(id).await;
            let server = ApServer::spawn(
                Launch {
                    season: id,
                    port,
                    password: password.clone(),
                    admin_password: admin_password.clone(),
                },
                self.db.clone(),
                rt.board.clone(),
            );
            *rt.server.lock().unwrap() = Some(Arc::new(server));
            let host = match cfg.ap_bind.as_str() {
                "0.0.0.0" | "::" | "" => "127.0.0.1".to_string(),
                h => h.to_string(),
            };
            // Bifrost's own connections (tracker, admin console) join the
            // room as one of its slots: the configured one when this season
            // has it (BIFROST_SLOT is instance-wide), else the first.
            let slots = self.db.slots(id).await.unwrap_or_default();
            let configured = config::var("BIFROST_SLOT").filter(|n| slots.iter().any(|r| &r.name == n));
            let first_slot = slots.into_iter().min_by_key(|r| r.ap_slot).map(|r| r.name);
            Some(ApConfig {
                url: format!("ws://{host}:{port}"),
                slot: configured.or(first_slot).unwrap_or_default(),
                password,
                admin_password,
            })
        } else {
            self.external.clone()
        };
        *rt.ap.write().unwrap() = ap.clone();
        self.spawn_workers(id, &rt, ap);
    }

    /// The season's tracker, in-logic recomputer and (external mode) activity
    /// ingester, connecting with `ap`.
    fn spawn_workers(&self, id: i64, rt: &SeasonRt, ap: Option<ApConfig>) {
        let cfg = config::get();
        let mut workers = rt.workers.lock().unwrap();
        if let Some(ap) = ap {
            tracing::info!("season {id}: AP tracker watching {}", ap.url);
            workers.extend(crate::ap::spawn(
                id,
                rt.board.clone(),
                ap.clone(),
                self.db.clone(),
                self.coop.clone(),
            ));
            workers.extend(crate::inlogic::spawn(
                id,
                rt.board.clone(),
                self.db.clone(),
                &ap,
            ));
        } else {
            tracing::info!(
                "season {id}: no AP server configured (BIFROST_AP_MODE=external needs BIFROST_SLOT)"
            );
        }
        // A managed server's output feeds the activity log directly; an
        // external one's comes from its systemd journal.
        if !cfg.ap_managed {
            workers.push(crate::activity::spawn(
                id,
                rt.board.clone(),
                self.db.clone(),
            ));
        }
    }

    /// Where players point their AP client for this season: the instance's
    /// public host plus the season's port (managed), or the external server's
    /// port. `None` until both are known. (A future subdomain/`:443` overlay
    /// for browser clients slots in here.)
    pub fn connect_address(&self, row: &crate::db::SeasonRow, domain: &str) -> Option<String> {
        if row.state() != crate::db::State::Live || domain.trim().is_empty() {
            return None;
        }
        let port = if config::get().ap_managed {
            row.ap_port.and_then(|p| u16::try_from(p).ok())
        } else {
            self.external
                .as_ref()
                .and_then(|c| c.url.rsplit(':').next()?.trim_end_matches('/').parse().ok())
        }?;
        Some(format!("{}:{port}", domain.trim()))
    }

    /// Deploy a freshly generated seed for a season. Managed: the supervisor
    /// stops the server, swaps the seed in with a fresh save, and restarts it.
    /// External: copy it in as `current.zip`; the host's watcher restarts the
    /// server (and must wipe the save after stopping it).
    pub async fn redeploy(&self, id: i64, seed_zip: &Path) -> Result<(), String> {
        if let Some(server) = self.get(id).server() {
            return server.redeploy(seed_zip.to_path_buf()).await;
        }
        let dirs = config::get().season(id);
        let dst = dirs.current_zip();
        let _ = tokio::fs::remove_file(&dst).await;
        tokio::fs::copy(seed_zip, &dst)
            .await
            .map_err(|e| format!("deploy: {e}"))?;
        let _ = tokio::fs::remove_file(dirs.current_save()).await;
        Ok(())
    }

    /// Send a server console command (managed mode); false if there's no
    /// managed server for the season.
    pub async fn console(&self, id: i64, cmd: &str) -> bool {
        match self.get(id).server() {
            Some(server) => {
                server.console(cmd).await;
                true
            }
            None => false,
        }
    }

    /// Update the live season's connect password everywhere Bifrost uses it:
    /// the shared config, and the workers (restarted, as they hold their own
    /// copy). The AP server supervisor reads it from the database on each start.
    pub fn set_connect_password(&self, id: i64, password: &str) {
        let rt = self.get(id);
        let ap = {
            let mut ap = rt.ap.write().unwrap();
            let Some(cfg) = ap.as_mut() else { return };
            cfg.password = password.to_string();
            ap.clone()
        };
        for w in rt.workers.lock().unwrap().drain(..) {
            w.abort();
        }
        self.spawn_workers(id, &rt, ap);
    }

    /// Stop a season's AP server (saving) and workers, and clear its board
    /// (on archive).
    pub async fn stop(&self, id: i64) {
        let rt = self.get(id);
        let server = rt.server.lock().unwrap().take();
        if let Some(server) = server {
            server.stop().await;
        }
        for w in rt.workers.lock().unwrap().drain(..) {
            w.abort();
        }
        *rt.ap.write().unwrap() = None;
        if let Ok(mut b) = rt.board.write() {
            *b = Board::default();
        }
    }
}
