//! Seasons as first-class records (design doc §7.1): named at creation, owned
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
             ap_admin_password TEXT NOT NULL DEFAULT ''
           )"#,
    )
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
    // A crash mid-generation must not strand a season in `generating`.
    sqlx::query(
        "UPDATE season SET state = CASE WHEN seed IS NULL THEN 'signup' ELSE 'live' END
         WHERE state = 'generating'",
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
}

/// `state` as stored (lets `FromRow` decode straight into [`State`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StateCol(pub State);

impl TryFrom<String> for StateCol {
    type Error = std::convert::Infallible;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Ok(StateCol(State::parse(&s)))
    }
}

impl SeasonRow {
    pub fn state(&self) -> State {
        self.state.0
    }
}

const COLS: &str = "id, name, state, created_by, created_at, seed, live_at, archived_at,
                    minesweeper_enabled, mc_enabled";

/// Why a transition was refused.
#[derive(Debug, PartialEq)]
pub enum TransitionError {
    NoSuchSeason,
    Illegal {
        from: State,
        to: State,
    },
    /// Only one season can be live until Bifrost runs an AP server per season.
    AnotherLive(String),
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
                "season “{name}” is live — only one season can be live at a time for now"
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
    pub async fn transition(&self, id: i64, to: State, now: i64) -> Result<(), TransitionError> {
        let db = |e: sqlx::Error| TransitionError::Db(e.to_string());
        let mut tx = self.pool.begin().await.map_err(db)?;
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
            let other: Option<String> = sqlx::query_scalar(
                "SELECT name FROM season WHERE state = 'live' AND id <> ? LIMIT 1",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if let Some(name) = other {
                return Err(TransitionError::AnotherLive(name));
            }
        }
        let sql = match to {
            State::Live => {
                "UPDATE season SET state = ?, live_at = COALESCE(live_at, ?) WHERE id = ?"
            }
            State::Archived => "UPDATE season SET state = ?, archived_at = ? WHERE id = ?",
            _ => "UPDATE season SET state = ? WHERE id = ?",
        };
        let mut q = sqlx::query(sql).bind(to.as_str());
        if matches!(to, State::Live | State::Archived) {
            q = q.bind(now);
        }
        q.bind(id).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)
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

    pub async fn set_minesweeper_enabled(&self, id: i64, on: bool) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE season SET minesweeper_enabled = ? WHERE id = ?")
            .bind(on)
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
}
