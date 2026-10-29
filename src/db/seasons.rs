//! Seasons as first-class records: named at creation, owned
//! by their creator, moving through a Bifrost-managed lifecycle
//!
//! ```text
//! draft ⇄ signup → generating → live → archived
//!                       ↑________/
//! ```
//!
//! `generating` is transient: success lands in `live`, failure returns to where
//! it came from; a re-roll goes `live → generating → live`. Everything that
//! belongs to a season — slots, claims, submissions, activity, hints, credits,
//! in-logic cache, minesweeper results — references `season(id)` and cascades
//! with it. Per-season toggles and the journal cursor live on the row.

use sqlx::{FromRow, SqlitePool};

use super::Db;

pub(super) async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS season (
             id INTEGER PRIMARY KEY,
             name TEXT NOT NULL,
             state TEXT NOT NULL DEFAULT 'draft'
                 CHECK (state IN ('draft', 'signup', 'generating', 'live', 'archived')),
             created_by INTEGER REFERENCES account(id) ON DELETE SET NULL,
             created_at INTEGER NOT NULL,
             -- The AP seed name of the deployed world (set by generation).
             seed TEXT,
             live_at INTEGER,
             archived_at INTEGER,
             minesweeper_enabled INTEGER NOT NULL DEFAULT 0,
             mc_enabled INTEGER NOT NULL DEFAULT 0,
             -- Where this season's AP-journal ingest resumes.
             journal_cursor TEXT,
             -- Frozen final standings (JSON), captured when archived.
             summary TEXT,
             -- The season's AP server passwords: connect (shown to players, may
             -- be empty) and admin (generated; only Bifrost uses it).
             ap_password TEXT NOT NULL DEFAULT '',
             ap_admin_password TEXT NOT NULL DEFAULT '',
             -- The port its managed AP server listens on: pinned at go-live,
             -- held through restarts and re-rolls, released on archive.
             ap_port INTEGER UNIQUE,
             -- The Archipelago version that generated the current seed.
             ap_version TEXT,
             -- While generating: the state it started from (crash recovery).
             gen_from TEXT
           )"#,
    )
    .execute(pool)
    .await?;
    // A seed names one season (and its archive bundle); imports rely on it.
    sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS season_seed ON season(seed)")
        .execute(pool)
        .await?;
    // Co-moderators: besides its creator, who may run a season.
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS season_mod (
             season INTEGER NOT NULL REFERENCES season(id) ON DELETE CASCADE,
             account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
             PRIMARY KEY (season, account_id)
           )"#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Draft,
    Signup,
    Generating,
    Live,
    Archived,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Draft => "draft",
            State::Signup => "signup",
            State::Generating => "generating",
            State::Live => "live",
            State::Archived => "archived",
        }
    }
    pub fn parse(s: &str) -> State {
        match s {
            "signup" => State::Signup,
            "generating" => State::Generating,
            "live" => State::Live,
            "archived" => State::Archived,
            _ => State::Draft,
        }
    }
    /// Is `self → to` a legal lifecycle step?
    pub fn can_go(self, to: State) -> bool {
        use State::*;
        matches!(
            (self, to),
            (Draft, Signup)
                | (Signup, Draft)
                | (Draft | Signup | Live, Generating)
                | (Generating, Live | Draft | Signup)
                | (Live, Archived)
        )
    }
    /// Signups are taken only in `signup`.
    pub fn accepts_signups(self) -> bool {
        self == State::Signup
    }
    /// Players can play (claim, hint, track) only while live.
    pub fn is_live(self) -> bool {
        self == State::Live
    }
}

/// The highest cap on minesweeper bonus hints a moderator can set (0 is no
/// cap).
pub const MAX_DAILY_HINTS: i64 = 1000;

#[derive(Debug, Clone, FromRow)]
pub struct SeasonRow {
    pub id: i64,
    pub name: String,
    #[sqlx(try_from = "String")]
    pub state: StateCol,
    /// The creator — the season's owning moderator.
    pub created_by: Option<i64>,
    pub created_at: i64,
    pub seed: Option<String>,
    pub live_at: Option<i64>,
    pub archived_at: Option<i64>,
    pub minesweeper_enabled: bool,
    pub mc_enabled: bool,
    /// Bonus hints a player can earn from minesweeper in any 24 hours; 0 is
    /// no cap.
    pub mine_daily_hints: i64,
    pub ap_port: Option<i64>,
    /// The Archipelago version that generated its current seed (it keeps
    /// running on that one; see `apinstall`).
    pub ap_version: Option<String>,
}

/// `state` as stored (lets `FromRow` decode straight into [`State`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StateCol(pub State);

impl From<String> for StateCol {
    fn from(s: String) -> Self {
        StateCol(State::parse(&s))
    }
}

impl SeasonRow {
    pub fn state(&self) -> State {
        self.state.0
    }
}

const COLS: &str = "id, name, state, created_by, created_at, seed, live_at, archived_at,
                    minesweeper_enabled, mc_enabled, mine_daily_hints, ap_port, ap_version";

/// Why a transition was refused.
#[derive(Debug, PartialEq)]
pub enum TransitionError {
    NoSuchSeason,
    Illegal {
        from: State,
        to: State,
    },
    /// External mode: one AP server, so only one season can be live.
    AnotherLive(String),
    /// Managed mode: every port in the range is held by a live season.
    NoFreePort {
        first: u16,
        last: u16,
    },
    Db(String),
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransitionError::NoSuchSeason => write!(f, "no such season"),
            TransitionError::Illegal { from, to } => {
                write!(f, "a {} season can't become {}", from.as_str(), to.as_str())
            }
            TransitionError::AnotherLive(name) => write!(
                f,
                "season “{name}” is live — with an external AP server only one season can be live"
            ),
            TransitionError::NoFreePort { first, last } => write!(
                f,
                "every AP port ({first}–{last}) is held by a live season — archive one, \
                 or have the sysadmin widen BIFROST_AP_PORTS"
            ),
            TransitionError::Db(e) => write!(f, "{e}"),
        }
    }
}

impl Db {
    pub async fn create_season(
        &self,
        name: &str,
        created_by: Option<i64>,
        now: i64,
    ) -> Result<i64, sqlx::Error> {
        let admin_pw: String = crate::auth::new_token().chars().take(24).collect();
        let r = sqlx::query(
            "INSERT INTO season (name, created_by, created_at, ap_admin_password) VALUES (?, ?, ?, ?)",
        )
            .bind(name)
            .bind(created_by)
            .bind(now)
            .bind(admin_pw)
            .execute(&self.pool)
            .await?;
        Ok(r.last_insert_rowid())
    }

    /// The season holding `seed`, if any.
    pub async fn season_by_seed(&self, seed: &str) -> Option<i64> {
        sqlx::query_scalar("SELECT id FROM season WHERE seed = ?")
            .bind(seed)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    /// An archived season brought in from another instance's bundle
    /// (`archive::import`): no slots or files, just its record and summary.
    #[allow(clippy::too_many_arguments)]
    pub async fn import_season(
        &self,
        name: &str,
        seed: &str,
        summary: &str,
        created_by: Option<i64>,
        created_at: i64,
        live_at: Option<i64>,
        archived_at: i64,
        ap_version: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        let r = sqlx::query(
            "INSERT INTO season (name, state, created_by, created_at, seed, live_at, archived_at, summary, ap_version)
             VALUES (?, 'archived', ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(created_by)
        .bind(created_at)
        .bind(seed)
        .bind(live_at)
        .bind(archived_at)
        .bind(summary)
        .bind(ap_version)
        .execute(&self.pool)
        .await?;
        Ok(r.last_insert_rowid())
    }

    pub async fn season(&self, id: i64) -> Option<SeasonRow> {
        sqlx::query_as(&format!("SELECT {COLS} FROM season WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    /// Every season, newest first.
    pub async fn seasons(&self) -> Vec<SeasonRow> {
        sqlx::query_as(&format!(
            "SELECT {COLS} FROM season ORDER BY created_at DESC, id DESC"
        ))
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// The season unscoped URLs mean: the live one, else the most recent
    /// in-progress one (generating, signup, draft — in that order), else the
    /// most recently archived.
    pub async fn default_season(&self) -> Option<SeasonRow> {
        sqlx::query_as(&format!(
            "SELECT {COLS} FROM season ORDER BY
               CASE state WHEN 'live' THEN 0 WHEN 'generating' THEN 1 WHEN 'signup' THEN 2
                          WHEN 'draft' THEN 3 ELSE 4 END,
               COALESCE(live_at, archived_at, created_at) DESC, id DESC
             LIMIT 1"
        ))
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    /// Everyone who may run the season besides sysadmins: its creator and its
    /// co-moderators.
    pub async fn season_mods(&self, id: i64) -> Vec<i64> {
        sqlx::query_scalar(
            "SELECT created_by FROM season WHERE id = ? AND created_by IS NOT NULL
             UNION SELECT account_id FROM season_mod WHERE season = ?",
        )
        .bind(id)
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// Co-moderators (not the creator) as (account id, username).
    pub async fn co_mods(&self, id: i64) -> Vec<(i64, String)> {
        sqlx::query_as(
            "SELECT a.id, a.username FROM season_mod m JOIN account a ON a.id = m.account_id
             WHERE m.season = ? ORDER BY a.username COLLATE NOCASE",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    pub async fn add_co_mod(&self, id: i64, account: i64) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT OR IGNORE INTO season_mod (season, account_id) VALUES (?, ?)")
            .bind(id)
            .bind(account)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn remove_co_mod(&self, id: i64, account: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM season_mod WHERE season = ? AND account_id = ?")
            .bind(id)
            .bind(account)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn rename_season(&self, id: i64, name: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE season SET name = ? WHERE id = ?")
            .bind(name)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Move a season along its lifecycle, enforcing the legal steps and the
    /// one-live-season rule. Stamps `live_at` / `archived_at`.
    /// A crash or restart mid-generation must not strand a season in
    /// `generating`: put it back where the generation started from. Server
    /// startup only — a CLI command (e.g. `backup now`) may run beside a live
    /// server whose generation is still going.
    pub async fn recover_interrupted_generations(&self) -> Result<u64, sqlx::Error> {
        let r = sqlx::query(
            "UPDATE season SET state = COALESCE(gen_from, 'signup'), gen_from = NULL
             WHERE state = 'generating'",
        )
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    pub async fn transition(&self, id: i64, to: State, now: i64) -> Result<(), TransitionError> {
        let db = |e: sqlx::Error| TransitionError::Db(e.to_string());
        // IMMEDIATE: this reads then writes; a deferred transaction would
        // fail outright (not wait) if another writer got in between.
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await.map_err(db)?;
        let from: Option<String> = sqlx::query_scalar("SELECT state FROM season WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
        let from = State::parse(&from.ok_or(TransitionError::NoSuchSeason)?);
        if !from.can_go(to) {
            return Err(TransitionError::Illegal { from, to });
        }
        if to == State::Live {
            let port = self.live_slot(&mut tx, id).await?;
            sqlx::query("UPDATE season SET ap_port = ? WHERE id = ?")
                .bind(port.map(i64::from))
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        let sql = match to {
            State::Live => {
                "UPDATE season SET state = ?, live_at = COALESCE(live_at, ?) WHERE id = ?"
            }
            State::Archived => {
                "UPDATE season SET state = ?, archived_at = ?, ap_port = NULL WHERE id = ?"
            }
            _ => "UPDATE season SET state = ? WHERE id = ?",
        };
        let mut q = sqlx::query(sql).bind(to.as_str());
        if matches!(to, State::Live | State::Archived) {
            q = q.bind(now);
        }
        q.bind(id).execute(&mut *tx).await.map_err(db)?;
        // Remember where a generation started, for crash recovery.
        sqlx::query("UPDATE season SET gen_from = ? WHERE id = ?")
            .bind((to == State::Generating).then_some(from.as_str()))
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)
    }

    /// Could the season go live right now (is there room)? Lets generation
    /// refuse before doing the work.
    pub async fn can_go_live(&self, id: i64) -> Result<(), TransitionError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| TransitionError::Db(e.to_string()))?;
        self.live_slot(&mut tx, id).await.map(|_| ())
    }

    /// Room for `id` among the live seasons: in managed mode its AP port —
    /// the one it already holds if still in range (stable across re-rolls and
    /// range edits that keep it), else the lowest free one; with an external
    /// server, `None` once no other season is live.
    async fn live_slot(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        id: i64,
    ) -> Result<Option<u16>, TransitionError> {
        let db = |e: sqlx::Error| TransitionError::Db(e.to_string());
        let Some(range) = self.ap_ports.clone() else {
            let other: Option<String> = sqlx::query_scalar(
                "SELECT name FROM season WHERE state = 'live' AND id <> ? LIMIT 1",
            )
            .bind(id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(db)?;
            return match other {
                Some(name) => Err(TransitionError::AnotherLive(name)),
                None => Ok(None),
            };
        };
        let held: Option<i64> = sqlx::query_scalar("SELECT ap_port FROM season WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(db)?
            .flatten();
        if let Some(p) = held.and_then(|p| u16::try_from(p).ok())
            && range.contains(&p)
        {
            return Ok(Some(p));
        }
        let taken: std::collections::HashSet<i64> =
            sqlx::query_scalar("SELECT ap_port FROM season WHERE ap_port IS NOT NULL AND id <> ?")
                .bind(id)
                .fetch_all(&mut **tx)
                .await
                .map_err(db)?
                .into_iter()
                .collect();
        range
            .clone()
            .find(|p| !taken.contains(&i64::from(*p)))
            .map(Some)
            .ok_or(TransitionError::NoFreePort {
                first: *range.start(),
                last: *range.end(),
            })
    }

    /// Boot-time port reconciliation (managed mode): only live seasons hold
    /// ports; a live season whose port fell outside the configured range gets
    /// a new one. Returns the live seasons left without a port (range full).
    pub async fn reconcile_ap_ports(&self) -> Vec<String> {
        let _ = sqlx::query("UPDATE season SET ap_port = NULL WHERE state <> 'live'")
            .execute(&self.pool)
            .await;
        let mut stranded = Vec::new();
        for s in self.seasons().await {
            if s.state() != State::Live {
                continue;
            }
            let ok = async {
                let mut tx = self
                    .pool
                    .begin_with("BEGIN IMMEDIATE")
                    .await
                    .map_err(|e| TransitionError::Db(e.to_string()))?;
                let port = self.live_slot(&mut tx, s.id).await?;
                sqlx::query("UPDATE season SET ap_port = ? WHERE id = ?")
                    .bind(port.map(i64::from))
                    .bind(s.id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| TransitionError::Db(e.to_string()))?;
                tx.commit()
                    .await
                    .map_err(|e| TransitionError::Db(e.to_string()))
            }
            .await;
            if let Err(e) = ok {
                tracing::error!("season {} ({}): no AP port — {e}", s.id, s.name);
                let _ = sqlx::query("UPDATE season SET ap_port = NULL WHERE id = ?")
                    .bind(s.id)
                    .execute(&self.pool)
                    .await;
                stranded.push(s.name);
            }
        }
        stranded
    }

    /// The season's AP server (connect, admin) passwords.
    pub async fn ap_passwords(&self, id: i64) -> (String, String) {
        sqlx::query_as("SELECT ap_password, ap_admin_password FROM season WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    pub async fn set_ap_password(&self, id: i64, password: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE season SET ap_password = ? WHERE id = ?")
            .bind(password)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_season_seed(&self, id: i64, seed: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE season SET seed = ? WHERE id = ?")
            .bind(seed)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_season_ap_version(
        &self,
        id: i64,
        version: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE season SET ap_version = ? WHERE id = ?")
            .bind(version)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_minesweeper_enabled(&self, id: i64, on: bool) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE season SET minesweeper_enabled = ? WHERE id = ?")
            .bind(on)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_mine_daily_hints(&self, id: i64, cap: i64) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE season SET mine_daily_hints = ? WHERE id = ?")
            .bind(cap)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_mc_enabled(&self, id: i64, on: bool) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE season SET mc_enabled = ? WHERE id = ?")
            .bind(on)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn journal_cursor(&self, id: i64) -> Option<String> {
        sqlx::query_scalar("SELECT journal_cursor FROM season WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .flatten()
    }

    pub async fn set_journal_cursor(&self, id: i64, cursor: Option<&str>) {
        let _ = sqlx::query("UPDATE season SET journal_cursor = ? WHERE id = ?")
            .bind(cursor)
            .bind(id)
            .execute(&self.pool)
            .await;
    }

    /// Store a season's frozen final standings (JSON).
    pub async fn set_season_summary(&self, id: i64, summary: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE season SET summary = ? WHERE id = ?")
            .bind(summary)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn season_summary(&self, id: i64) -> Option<String> {
        sqlx::query_scalar("SELECT summary FROM season WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .flatten()
    }

    /// When the most recent *other* season was archived (0 if none) — the lower
    /// bound for a season's one-time journal backfill.
    pub async fn last_archived_before(&self, id: i64) -> i64 {
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT MAX(archived_at) FROM season WHERE id <> ? AND archived_at IS NOT NULL",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(0)
    }

    /// Forget everything derived from the season's previous AP seed (before a
    /// re-roll deploys a new one): activity log + cursor, hint priorities and
    /// the in-logic cache. Slots/claims are remapped separately; minesweeper
    /// results and hint credits belong to the season and are kept.
    pub async fn clear_seed_data(&self, id: i64) -> Result<(), sqlx::Error> {
        for table in ["activity", "hint_priority", "inlogic"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE season = ?"))
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        self.set_journal_cursor(id, None).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_rules() {
        use State::*;
        assert!(Draft.can_go(Signup) && Signup.can_go(Draft));
        assert!(Signup.can_go(Generating) && Draft.can_go(Generating));
        assert!(Generating.can_go(Live) && Generating.can_go(Signup));
        assert!(Live.can_go(Generating), "re-roll");
        assert!(Live.can_go(Archived));
        assert!(!Draft.can_go(Live), "live only via generation");
        assert!(!Signup.can_go(Archived), "only a played season is archived");
        assert!(
            !Archived.can_go(Live) && !Archived.can_go(Draft),
            "archive is final"
        );
        assert!(Signup.accepts_signups() && !Live.accepts_signups());
    }

    async fn db() -> Db {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("bifrost-seasontest-{}-{n}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Db::open(&path.to_string_lossy()).await.unwrap()
    }

    #[tokio::test]
    async fn creator_and_co_mods_run_a_season() {
        let db = db().await;
        let owner = db
            .create_account("owner", "h", crate::auth::Role::Moderator, 0)
            .await
            .unwrap();
        let co = db
            .create_account("co", "h", crate::auth::Role::Moderator, 0)
            .await
            .unwrap();
        let s = db.create_season("S", Some(owner), 0).await.unwrap();
        assert_eq!(db.season_mods(s).await, vec![owner]);
        db.add_co_mod(s, co).await.unwrap();
        db.add_co_mod(s, co).await.unwrap();
        let mut mods = db.season_mods(s).await;
        mods.sort();
        assert_eq!(mods, vec![owner, co]);
        assert_eq!(db.co_mods(s).await, vec![(co, "co".to_string())]);
        db.remove_co_mod(s, co).await.unwrap();
        assert_eq!(db.season_mods(s).await, vec![owner]);
    }

    #[tokio::test]
    async fn transitions_enforce_rules_and_one_live_season() {
        let db = db().await;
        let a = db.create_season("Spring", None, 10).await.unwrap();
        let b = db.create_season("Summer", None, 20).await.unwrap();
        assert_eq!(db.default_season().await.unwrap().id, b, "newest draft");
        assert!(matches!(
            db.transition(a, State::Live, 30).await,
            Err(TransitionError::Illegal { .. })
        ));
        db.transition(a, State::Generating, 30).await.unwrap();
        db.transition(a, State::Live, 31).await.unwrap();
        assert_eq!(db.season(a).await.unwrap().live_at, Some(31));
        assert_eq!(db.default_season().await.unwrap().id, a, "live wins");
        db.transition(b, State::Generating, 40).await.unwrap();
        assert!(matches!(
            db.transition(b, State::Live, 41).await,
            Err(TransitionError::AnotherLive(n)) if n == "Spring"
        ));
        db.transition(b, State::Signup, 41).await.unwrap();
        db.transition(a, State::Archived, 50).await.unwrap();
        assert_eq!(db.season(a).await.unwrap().archived_at, Some(50));
        assert_eq!(db.default_season().await.unwrap().id, b);
        assert_eq!(db.last_archived_before(b).await, 50);
        assert_eq!(db.last_archived_before(a).await, 0);
    }

    #[tokio::test]
    async fn interrupted_generations_go_back_where_they_started() {
        let db = db().await.with_ap_ports(5000..=5001);
        let first = db.create_season("First", None, 0).await.unwrap();
        db.transition(first, State::Signup, 1).await.unwrap();
        db.transition(first, State::Generating, 2).await.unwrap();
        // Generation writes the seed before the world is deployed.
        db.set_season_seed(first, "seed-1").await.unwrap();
        let reroll = db.create_season("Reroll", None, 0).await.unwrap();
        db.transition(reroll, State::Generating, 1).await.unwrap();
        db.transition(reroll, State::Live, 2).await.unwrap();
        db.transition(reroll, State::Generating, 3).await.unwrap();
        assert_eq!(db.recover_interrupted_generations().await.unwrap(), 2);
        assert_eq!(
            db.season(first).await.unwrap().state(),
            State::Signup,
            "not live with an undeployed seed"
        );
        assert_eq!(db.season(reroll).await.unwrap().state(), State::Live);
        assert_eq!(db.recover_interrupted_generations().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn live_seasons_hold_stable_ports_from_the_range() {
        let db = db().await.with_ap_ports(5000..=5001);
        let mut ids = Vec::new();
        for (i, name) in ["A", "B", "C"].iter().enumerate() {
            let id = db.create_season(name, None, i as i64).await.unwrap();
            db.transition(id, State::Generating, 10).await.unwrap();
            ids.push(id);
        }
        let (a, b, c) = (ids[0], ids[1], ids[2]);
        let port = |id| {
            let db = db.clone();
            async move { db.season(id).await.unwrap().ap_port }
        };
        db.transition(a, State::Live, 11).await.unwrap();
        db.transition(b, State::Live, 12).await.unwrap();
        assert_eq!((port(a).await, port(b).await), (Some(5000), Some(5001)));
        // Range full: C can't go live (and generation can tell up front).
        assert_eq!(
            db.can_go_live(c).await,
            Err(TransitionError::NoFreePort {
                first: 5000,
                last: 5001
            })
        );
        assert!(db.transition(c, State::Live, 13).await.is_err());
        // A re-roll keeps the port.
        db.transition(a, State::Generating, 14).await.unwrap();
        assert!(db.can_go_live(c).await.is_err(), "still held mid re-roll");
        db.transition(a, State::Live, 15).await.unwrap();
        assert_eq!(port(a).await, Some(5000));
        // Archiving releases it; C takes it.
        db.transition(a, State::Archived, 16).await.unwrap();
        assert_eq!(port(a).await, None);
        db.transition(c, State::Live, 17).await.unwrap();
        assert_eq!(port(c).await, Some(5000));
        // Boot with a moved range: live seasons are re-pinned inside it,
        // keeping ports that still fit.
        let moved = db.clone().with_ap_ports(5001..=5003);
        assert!(moved.reconcile_ap_ports().await.is_empty());
        assert_eq!(port(b).await, Some(5001), "still in range: kept");
        assert_eq!(port(c).await, Some(5002), "out of range: re-pinned");
        // A shrunken range strands the overflow (logged, not crashed).
        let tiny = db.clone().with_ap_ports(5002..=5002);
        assert_eq!(tiny.reconcile_ap_ports().await, vec!["B".to_string()]);
    }
}
