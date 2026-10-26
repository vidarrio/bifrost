//! Per-season runtime state (design doc §7.4). Every season has its own live
//! board and generation status; the **live** season also runs the background
//! workers — AP tracker + counts refresher, activity ingester, in-logic
//! recomputer — started when it goes live and stopped when it's archived.
//!
//! Until Bifrost supervises an AP server per season (§7.2), the configured AP
//! endpoint serves the one live season.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use tokio::task::JoinHandle;

use crate::ap::{ApConfig, Board};
use crate::db::Db;
use crate::generate::GenState;

pub struct SeasonRt {
    pub board: Arc<RwLock<Board>>,
    pub genstate: GenState,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

pub struct Runtimes {
    db: Db,
    ap_cfg: Option<ApConfig>,
    coop: Vec<(String, String)>,
    map: Mutex<HashMap<i64, Arc<SeasonRt>>>,
}

impl Runtimes {
    pub fn new(db: Db, ap_cfg: Option<ApConfig>, coop: Vec<(String, String)>) -> Self {
        Self {
            db,
            ap_cfg,
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
                    workers: Mutex::default(),
                })
            })
            .clone()
    }

    /// Start the live season's workers (idempotent).
    pub fn start_live(&self, id: i64) {
        let rt = self.get(id);
        let mut workers = rt.workers.lock().unwrap();
        if !workers.is_empty() {
            return;
        }
        if let Some(cfg) = self.ap_cfg.clone() {
            tracing::info!(
                "season {id}: AP tracker watching {} as slot {}",
                cfg.url,
                cfg.slot
            );
            workers.extend(crate::ap::spawn(
                id,
                rt.board.clone(),
                cfg.clone(),
                self.db.clone(),
                self.coop.clone(),
            ));
            workers.extend(crate::inlogic::spawn(
                id,
                rt.board.clone(),
                self.db.clone(),
                &cfg,
            ));
        } else {
            tracing::info!(
                "season {id}: AP tracker idle (set BIFROST_SLOT to enable the live board)"
            );
        }
        // The activity log reads the AP server journal, independent of the
        // tracker connection.
        workers.push(crate::activity::spawn(
            id,
            rt.board.clone(),
            self.db.clone(),
        ));
    }

    /// Stop a season's workers and clear its live board (on archive).
    pub fn stop(&self, id: i64) {
        let rt = self.get(id);
        for w in rt.workers.lock().unwrap().drain(..) {
            w.abort();
        }
        if let Ok(mut b) = rt.board.write() {
            *b = Board::default();
        }
    }
}
