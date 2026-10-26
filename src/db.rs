//! Persistence for Bifrost slots + claims (SQLite).
//!
//! A **Bifrost slot** is one person (claimable, tied to an SSO), mapping to one
//! **AP slot** (the game). Co-op games (e.g. Don't Starve Together) have several
//! Bifrost slots pointing at the same `ap_slot`. A season is keyed by the AP
//! `seed_name`, so a new seed gets a fresh set of slots + claims.

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{FromRow, SqlitePool};

mod auth;
pub use auth::{AccountRow, LinkKind, LinkRow, RedeemError};

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

#[derive(Debug, FromRow, Clone)]
pub struct SlotRow {
    pub id: i64,
    pub name: String,
    pub ap_slot: i64,
    pub game: String,
    pub claimed_by: Option<String>,
    /// A communal "common game" slot (the shared Minecraft server): one AP slot
    /// everyone plays, no per-person claim. Rendered as a shared card.
    #[sqlx(default)]
    pub is_common: bool,
}

/// Aggregated minesweeper stats for one player in one season. Time fields are
/// over wins only and are `None` until there's a win.
#[derive(Debug, Default, FromRow, Clone)]
pub struct MineStats {
    pub wins: i64,
    pub losses: i64,
    pub avg_ms: Option<i64>,
    pub best_ms: Option<i64>,
    pub worst_ms: Option<i64>,
}

impl MineStats {
    pub fn total(&self) -> i64 {
        self.wins + self.losses
    }
    /// Success rate as a whole-number percentage (0 when nothing played).
    pub fn success_pct(&self) -> i64 {
        let t = self.total();
        if t == 0 {
            0
        } else {
            (self.wins * 100) / t
        }
    }
}

/// Result of an attempted claim under the one-slot-per-player rule.
pub enum ClaimOutcome {
    Ok,
    /// The slot is already claimed by someone else.
    SlotTaken,
    /// This player already claims another slot this season (its name).
    AlreadyHave(String),
}

/// Result of setting a Minecraft username for the whitelist.
pub enum SetMcOutcome {
    Ok,
    /// Some other player has already registered that Minecraft username.
    TakenByOther,
}

#[derive(Debug, FromRow, Clone)]
pub struct SubmissionRow {
    pub id: i64,
    pub sso: String,
    pub slot_name: String,
    pub game: String,
    pub is_custom: bool,
    pub coop_names: String,
    pub yaml_file: String,
}

/// One persisted activity-log entry, sourced from the AP server journal. `ts` is
/// epoch seconds (the journal's realtime timestamp); `kind` is one of
/// send/goal/release/collect; `text` is the rendered line. Rows are ordered by
/// `id` (journal order), which is stable even for events in the same second.
#[derive(Debug, FromRow, Clone)]
pub struct ActivityRow {
    pub ts: i64,
    pub kind: String,
    pub text: String,
    /// Sent-item importance: progression/useful/trap/filler (sends only).
    #[sqlx(rename = "item_class")]
    pub class: Option<String>,
}

/// A slot's cached Universal-Tracker in-logic result, as the game page reads it.
/// (The `inlogic` table also stores game/count/approx/sent/received for the
/// recomputer's own bookkeeping; those aren't needed for rendering.)
#[derive(Debug, FromRow, Clone, Default)]
pub struct InLogic {
    /// When this result was computed (epoch seconds).
    pub ts: i64,
    /// JSON array of in-logic location names.
    pub locations: String,
    /// Set when the last compute failed (and no good prior result was kept).
    pub err: Option<String>,
}

/// Filter for the activity log. Empty `kinds`/`classes` = unconstrained; `None`
/// fields are unconstrained. `since`/`until` are epoch-second bounds (inclusive).
#[derive(Default)]
pub struct ActFilter<'a> {
    pub kinds: &'a [&'a str],
    pub classes: &'a [&'a str],
    pub sender: Option<&'a str>,
    pub receiver: Option<&'a str>,
    pub since: Option<i64>,
    pub until: Option<i64>,
}

impl Db {
    pub async fn open(path: &str) -> Result<Self, sqlx::Error> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS bifrost_slot (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 season TEXT NOT NULL,
                 name TEXT NOT NULL,
                 ap_slot INTEGER NOT NULL,
                 game TEXT NOT NULL,
                 UNIQUE(season, name)
               )"#,
        )
        .execute(&pool)
        .await?;
        // `is_common` marks the shared "common game" slot (added after launch;
        // ignore the error when the column already exists).
        let _ = sqlx::query(
            "ALTER TABLE bifrost_slot ADD COLUMN is_common INTEGER NOT NULL DEFAULT 0",
        )
        .execute(&pool)
        .await;
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS claim (
                 bifrost_slot_id INTEGER PRIMARY KEY REFERENCES bifrost_slot(id) ON DELETE CASCADE,
                 sso TEXT NOT NULL,
                 claimed_at INTEGER NOT NULL
               )"#,
        )
        .execute(&pool)
        .await?;
        // Signup submissions for the *next* season (before any seed exists).
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS submission (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 sso TEXT NOT NULL,
                 slot_name TEXT NOT NULL UNIQUE,
                 game TEXT NOT NULL,
                 is_custom INTEGER NOT NULL DEFAULT 0,
                 coop_names TEXT NOT NULL DEFAULT '',
                 yaml_file TEXT NOT NULL,
                 apworld_file TEXT,
                 created_at INTEGER NOT NULL
               )"#,
        )
        .execute(&pool)
        .await?;
        // Simple key/value settings (e.g. whether signups are open).
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS setting (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
               )"#,
        )
        .execute(&pool)
        .await?;
        // Bonus hint credits earned via the minesweeper minigame (per season).
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS hint_credit (
                 season TEXT NOT NULL,
                 sso TEXT NOT NULL,
                 credits INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (season, sso)
               )"#,
        )
        .execute(&pool)
        .await?;
        // Hints a player has flagged to surface on the board.
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS hint_priority (
                 season TEXT NOT NULL,
                 finding INTEGER NOT NULL,
                 location INTEGER NOT NULL,
                 item INTEGER NOT NULL,
                 PRIMARY KEY (season, finding, location, item)
               )"#,
        )
        .execute(&pool)
        .await?;
        // One row per finished minesweeper game, for per-slot/per-season stats.
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS mine_result (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 season TEXT NOT NULL,
                 sso TEXT NOT NULL,
                 won INTEGER NOT NULL,
                 duration_ms INTEGER NOT NULL,
                 ts INTEGER NOT NULL,
                 kind TEXT NOT NULL DEFAULT 'standard'
               )"#,
        )
        .execute(&pool)
        .await?;
        // Add `kind` to databases created before board types existed (rows from
        // back then were all the standard board). Ignore the error if present.
        let _ = sqlx::query("ALTER TABLE mine_result ADD COLUMN kind TEXT NOT NULL DEFAULT 'standard'")
            .execute(&pool)
            .await;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_mine_result ON mine_result (season, sso, kind)",
        )
        .execute(&pool)
        .await?;
        // Frozen per-season summaries for the Past Seasons page. Keyed by the AP
        // seed so each season is captured once; deliberately NOT touched by the
        // admin reset, so finished seasons survive the clean slate.
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS season_archive (
                 seed TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 finalized_at INTEGER NOT NULL,
                 summary TEXT NOT NULL
               )"#,
        )
        .execute(&pool)
        .await?;
        // A player's Minecraft username, for the common-game server's whitelist.
        // Keyed by SSO identity (persists across seasons — a returning player
        // keeps their name); the case-insensitive unique index stops two people
        // claiming the same Minecraft account. The *whitelist* itself is scoped
        // to the current season's claimants (see `mc_whitelist`).
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS mc_account (
                 sso TEXT PRIMARY KEY,
                 mc_username TEXT NOT NULL,
                 updated_at INTEGER NOT NULL
               )"#,
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_mc_account_name ON mc_account (mc_username COLLATE NOCASE)",
        )
        .execute(&pool)
        .await?;
        // Persistent activity log, one row per kept AP journal event. Scoped by
        // season (AP seed) so the archived/past-season view can replay it; like
        // `season_archive`, deliberately NOT cleared by the admin reset. Ordered
        // by `id` = journal order. De-duplication across runs is handled by the
        // journal cursor (setting `journal_cursor`), not a UNIQUE constraint, so
        // genuine identical events in the same second are never dropped.
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS activity (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 season TEXT NOT NULL,
                 ts INTEGER NOT NULL,
                 kind TEXT NOT NULL,
                 text TEXT NOT NULL,
                 sender TEXT,
                 receiver TEXT,
                 item_class TEXT
               )"#,
        )
        .execute(&pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_activity ON activity (season, id)")
            .execute(&pool)
            .await?;
        // Migrate older DBs: add sender/receiver/item_class. If any ADD succeeds
        // the column was absent (old rows lack it) — and since the whole log is
        // derived from the AP journal, clear it + the cursor to re-backfill with
        // the new fields populated. A fresh DB already has the columns (CREATE
        // above), so the ADDs error and nothing is wiped.
        let mut added = false;
        for col in ["sender TEXT", "receiver TEXT", "item_class TEXT"] {
            added |= sqlx::query(&format!("ALTER TABLE activity ADD COLUMN {col}"))
                .execute(&pool)
                .await
                .is_ok();
        }
        if added {
            let _ = sqlx::query("DELETE FROM activity").execute(&pool).await;
            let _ = sqlx::query("DELETE FROM setting WHERE key = 'journal_cursor'")
                .execute(&pool)
                .await;
        }
        // Universal-Tracker in-logic cache: one row per (season, ap_slot), holding
        // that slot's currently-reachable location names (computed out-of-band by
        // the `inlogic` recomputer shelling out to UT). `sent`/`received` are the
        // slot state the row was computed at, so the recomputer re-runs only when
        // the slot's state moves.
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS inlogic (
                 season TEXT NOT NULL,
                 ap_slot INTEGER NOT NULL,
                 ts INTEGER NOT NULL,
                 game TEXT NOT NULL DEFAULT '',
                 sent INTEGER NOT NULL DEFAULT 0,
                 received INTEGER NOT NULL DEFAULT 0,
                 count INTEGER NOT NULL DEFAULT 0,
                 locations TEXT NOT NULL DEFAULT '[]',
                 approx INTEGER NOT NULL DEFAULT 0,
                 err TEXT,
                 PRIMARY KEY (season, ap_slot)
               )"#,
        )
        .execute(&pool)
        .await?;
        auth::migrate(&pool).await?;
        Ok(Self { pool })
    }

    async fn get_setting(&self, key: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT value FROM setting WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
    }

    async fn set_setting(&self, key: &str, value: &str) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT OR REPLACE INTO setting (key, value) VALUES (?, ?)")
            .bind(key)
            .bind(value)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Whether the season signup form is open (defaults to open if unset).
    pub async fn signups_open(&self) -> bool {
        !matches!(self.get_setting("signups_open").await, Ok(Some(v)) if v == "0")
    }

    pub async fn set_signups_open(&self, open: bool) -> Result<(), sqlx::Error> {
        self.set_setting("signups_open", if open { "1" } else { "0" })
            .await
    }

    /// Whether the minesweeper minigame is enabled (defaults to off).
    pub async fn minesweeper_enabled(&self) -> bool {
        matches!(self.get_setting("minesweeper_enabled").await, Ok(Some(v)) if v == "1")
    }

    pub async fn set_minesweeper_enabled(&self, on: bool) -> Result<(), sqlx::Error> {
        self.set_setting("minesweeper_enabled", if on { "1" } else { "0" })
            .await
    }

    /// Whether the shared Minecraft common game is enabled for the season (read
    /// at generation time to decide whether the common slot is included;
    /// defaults to off).
    pub async fn mc_enabled(&self) -> bool {
        matches!(self.get_setting("mc_enabled").await, Ok(Some(v)) if v == "1")
    }

    pub async fn set_mc_enabled(&self, on: bool) -> Result<(), sqlx::Error> {
        self.set_setting("mc_enabled", if on { "1" } else { "0" })
            .await
    }

    /// A player's bonus-hint credits for a season.
    pub async fn credits(&self, season: &str, sso: &str) -> i64 {
        sqlx::query_scalar("SELECT credits FROM hint_credit WHERE season = ? AND sso = ?")
            .bind(season)
            .bind(sso)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(0)
    }

    /// True if this SSO user has claimed a regular (non-common) slot this season.
    /// Communal-hint gate: only players with a game of their own in the season may
    /// spend the common game's shared hint points.
    pub async fn has_claimed_slot(&self, season: &str, sso: &str) -> bool {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM claim c JOIN bifrost_slot s ON s.id = c.bifrost_slot_id
             WHERE s.season = ? AND c.sso = ? AND s.is_common = 0",
        )
        .bind(season)
        .bind(sso)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        n > 0
    }

    /// This player's registered Minecraft username, if any.
    pub async fn mc_username(&self, sso: &str) -> Option<String> {
        sqlx::query_scalar("SELECT mc_username FROM mc_account WHERE sso = ?")
            .bind(sso)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    /// Set (or update) this player's Minecraft username. Fails if another player
    /// already registered it (case-insensitive). The caller is expected to have
    /// already validated the name's shape.
    pub async fn set_mc_username(
        &self,
        sso: &str,
        name: &str,
    ) -> Result<SetMcOutcome, sqlx::Error> {
        // Taken by someone else? (Same user re-saving their own name is fine.)
        let owner: Option<String> =
            sqlx::query_scalar("SELECT sso FROM mc_account WHERE mc_username = ? COLLATE NOCASE")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        if matches!(owner, Some(o) if o != sso) {
            return Ok(SetMcOutcome::TakenByOther);
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        sqlx::query(
            "INSERT INTO mc_account (sso, mc_username, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(sso) DO UPDATE SET mc_username = excluded.mc_username, updated_at = excluded.updated_at",
        )
        .bind(sso)
        .bind(name)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(SetMcOutcome::Ok)
    }

    /// The Minecraft whitelist for a season: the registered usernames of everyone
    /// holding a regular (non-common) slot this season. Exported for the
    /// host-side `mc-whitelist-sync` to apply to the Minecraft server.
    pub async fn mc_whitelist(&self, season: &str) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT a.mc_username FROM mc_account a
             WHERE EXISTS (
                 SELECT 1 FROM claim c JOIN bifrost_slot s ON s.id = c.bifrost_slot_id
                 WHERE s.season = ? AND c.sso = a.sso AND s.is_common = 0
             )
             ORDER BY a.mc_username COLLATE NOCASE",
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// Grant `n` bonus-hint credits, returning the new total.
    pub async fn add_credits(&self, season: &str, sso: &str, n: i64) -> Result<i64, sqlx::Error> {
        sqlx::query(
            "INSERT INTO hint_credit (season, sso, credits) VALUES (?, ?, ?)
             ON CONFLICT(season, sso) DO UPDATE SET credits = credits + excluded.credits",
        )
        .bind(season)
        .bind(sso)
        .bind(n)
        .execute(&self.pool)
        .await?;
        Ok(self.credits(season, sso).await)
    }

    /// Spend one bonus-hint credit if available; returns true if one was spent.
    pub async fn spend_credit(&self, season: &str, sso: &str) -> bool {
        let res = sqlx::query(
            "UPDATE hint_credit SET credits = credits - 1
             WHERE season = ? AND sso = ? AND credits > 0",
        )
        .bind(season)
        .bind(sso)
        .execute(&self.pool)
        .await;
        matches!(res, Ok(r) if r.rows_affected() > 0)
    }

    /// Record a finished minesweeper game (win or loss, with its duration) for a
    /// given board kind ("standard" / "expert").
    pub async fn record_mine_result(
        &self,
        season: &str,
        sso: &str,
        kind: &str,
        won: bool,
        duration_ms: i64,
    ) {
        let _ = sqlx::query(
            "INSERT INTO mine_result (season, sso, kind, won, duration_ms, ts)
             VALUES (?, ?, ?, ?, ?, strftime('%s','now'))",
        )
        .bind(season)
        .bind(sso)
        .bind(kind)
        .bind(won as i64)
        .bind(duration_ms)
        .execute(&self.pool)
        .await;
    }

    /// Per-kind minesweeper stats for a player. `season = Some(s)` scopes to one
    /// season; `None` is all-time (across every season + freeplay) — the
    /// "permanent" view. Time stats are over wins only; `None` = no wins yet.
    pub async fn mine_stats(&self, season: Option<&str>, sso: &str, kind: &str) -> MineStats {
        let mut sql = String::from(
            "SELECT COALESCE(SUM(won),0) AS wins, COALESCE(SUM(1-won),0) AS losses, \
             CAST(AVG(CASE WHEN won=1 THEN duration_ms END) AS INTEGER) AS avg_ms, \
             MIN(CASE WHEN won=1 THEN duration_ms END) AS best_ms, \
             MAX(CASE WHEN won=1 THEN duration_ms END) AS worst_ms \
             FROM mine_result WHERE sso=? AND kind=?",
        );
        if season.is_some() {
            sql.push_str(" AND season=?");
        }
        let mut q = sqlx::query_as::<_, MineStats>(&sql).bind(sso).bind(kind);
        if let Some(s) = season {
            q = q.bind(s);
        }
        q.fetch_one(&self.pool).await.unwrap_or_default()
    }

    /// Best-time leaderboard: (player, best_ms) over won games, fastest first,
    /// top `limit`. `season = None` is the all-time (permanent) board.
    pub async fn mine_leaders_time(
        &self,
        season: Option<&str>,
        kind: &str,
        limit: i64,
    ) -> Vec<(String, i64)> {
        let mut sql =
            String::from("SELECT sso, MIN(duration_ms) AS best FROM mine_result WHERE kind=? AND won=1");
        if season.is_some() {
            sql.push_str(" AND season=?");
        }
        sql.push_str(" GROUP BY sso ORDER BY best ASC LIMIT ?");
        let mut q = sqlx::query_as::<_, (String, i64)>(&sql).bind(kind);
        if let Some(s) = season {
            q = q.bind(s);
        }
        q.bind(limit).fetch_all(&self.pool).await.unwrap_or_default()
    }

    /// Best completion-rate leaderboard: (player, wins, total) for players with
    /// at least `min_games`, highest rate first (more games breaks ties), top
    /// `limit`. `season = None` is the all-time (permanent) board.
    pub async fn mine_leaders_rate(
        &self,
        season: Option<&str>,
        kind: &str,
        min_games: i64,
        limit: i64,
    ) -> Vec<(String, i64, i64)> {
        let mut sql =
            String::from("SELECT sso, SUM(won) AS wins, COUNT(*) AS total FROM mine_result WHERE kind=?");
        if season.is_some() {
            sql.push_str(" AND season=?");
        }
        sql.push_str(
            " GROUP BY sso HAVING total>=? ORDER BY (CAST(SUM(won) AS REAL)/COUNT(*)) DESC, total DESC LIMIT ?",
        );
        let mut q = sqlx::query_as::<_, (String, i64, i64)>(&sql).bind(kind);
        if let Some(s) = season {
            q = q.bind(s);
        }
        q.bind(min_games)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default()
    }

    /// Delete every signup submission, returning their stored file paths so the
    /// caller can remove the uploads.
    pub async fn clear_submissions(&self) -> Result<Vec<String>, sqlx::Error> {
        let files: Vec<String> = sqlx::query_scalar("SELECT yaml_file FROM submission")
            .fetch_all(&self.pool)
            .await?;
        sqlx::query("DELETE FROM submission")
            .execute(&self.pool)
            .await?;
        Ok(files)
    }

    /// Delete every Bifrost slot (claims cascade). Used by the admin reset.
    pub async fn clear_all_slots(&self) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM bifrost_slot")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Store (or overwrite) a season's frozen summary for the Past Seasons page.
    pub async fn archive_season(
        &self,
        seed: &str,
        name: &str,
        summary: &str,
    ) -> Result<(), sqlx::Error> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default();
        sqlx::query(
            "INSERT INTO season_archive (seed, name, finalized_at, summary) VALUES (?, ?, ?, ?)
             ON CONFLICT(seed) DO UPDATE SET
                 name = excluded.name,
                 finalized_at = excluded.finalized_at,
                 summary = excluded.summary",
        )
        .bind(seed)
        .bind(name)
        .bind(now)
        .bind(summary)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Archived seasons as (seed, name, finalized_at), newest first.
    pub async fn past_seasons(&self) -> Vec<(String, String, i64)> {
        sqlx::query_as::<_, (String, String, i64)>(
            "SELECT seed, name, finalized_at FROM season_archive ORDER BY finalized_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// One archived season's stored summary JSON, if present.
    pub async fn past_season(&self, seed: &str) -> Option<String> {
        sqlx::query_scalar::<_, String>("SELECT summary FROM season_archive WHERE seed = ?")
            .bind(seed)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    /// The most recent archived season's finalization time (0 if none). Used as
    /// the lower bound for the current season's one-time retroactive journal
    /// backfill: the current season began when the previous one ended.
    pub async fn last_finalized_at(&self) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT MAX(finalized_at) FROM season_archive")
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(0)
    }

    /// The saved journal cursor (where incremental ingest resumes), if set.
    pub async fn journal_cursor(&self) -> Option<String> {
        self.get_setting("journal_cursor").await.ok().flatten()
    }

    pub async fn set_journal_cursor(&self, cursor: &str) {
        let _ = self.set_setting("journal_cursor", cursor).await;
    }

    /// Append an activity row (ingest path; dedup is via the cursor). `sender`
    /// is the actor (sender of an item, or the player who goaled/released/
    /// collected); `receiver` is the item's recipient (sends only, else None).
    pub async fn activity_add(
        &self,
        season: &str,
        ts: i64,
        kind: &str,
        text: &str,
        sender: Option<&str>,
        receiver: Option<&str>,
        item_class: Option<&str>,
    ) {
        let _ = sqlx::query(
            "INSERT INTO activity (season, ts, kind, text, sender, receiver, item_class)
             VALUES (?,?,?,?,?,?,?)",
        )
        .bind(season)
        .bind(ts)
        .bind(kind)
        .bind(text)
        .bind(sender)
        .bind(receiver)
        .bind(item_class)
        .execute(&self.pool)
        .await;
    }

    /// Whether any activity rows exist for a season (guards the one-time backfill).
    pub async fn activity_any(&self, season: &str) -> bool {
        sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM activity WHERE season = ?)")
            .bind(season)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0)
            == 1
    }

    /// Cached in-logic result for one slot, if computed this season.
    pub async fn inlogic_get(&self, season: &str, ap_slot: i64) -> Option<InLogic> {
        sqlx::query_as::<_, InLogic>(
            "SELECT ts, locations, err FROM inlogic WHERE season = ? AND ap_slot = ?",
        )
        .bind(season)
        .bind(ap_slot)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    /// Per-slot in-logic ("available now") counts for a season, for the board
    /// cards. Only slots whose last compute succeeded (no error) are included, so
    /// a UT-disabled or not-yet-computed slot simply omits the figure.
    pub async fn inlogic_counts(&self, season: &str) -> std::collections::HashMap<i64, i64> {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT ap_slot, count FROM inlogic WHERE season = ? AND err IS NULL",
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await
        .map(|rows| rows.into_iter().collect())
        .unwrap_or_default()
    }

    /// The `(sent, received)` state a slot's cached in-logic was computed at, so
    /// the recomputer can skip slots whose state hasn't moved. `None` = no row.
    pub async fn inlogic_state(&self, season: &str, ap_slot: i64) -> Option<(i64, i64)> {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT sent, received FROM inlogic WHERE season = ? AND ap_slot = ?",
        )
        .bind(season)
        .bind(ap_slot)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    /// Upsert a slot's computed in-logic result.
    #[allow(clippy::too_many_arguments)]
    pub async fn inlogic_set(
        &self,
        season: &str,
        ap_slot: i64,
        ts: i64,
        game: &str,
        sent: i64,
        received: i64,
        count: i64,
        locations: &str,
        approx: bool,
        err: Option<&str>,
    ) {
        let _ = sqlx::query(
            "INSERT INTO inlogic (season, ap_slot, ts, game, sent, received, count, locations, approx, err)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(season, ap_slot) DO UPDATE SET
               ts = excluded.ts, game = excluded.game, sent = excluded.sent,
               received = excluded.received, count = excluded.count,
               locations = excluded.locations, approx = excluded.approx, err = excluded.err",
        )
        .bind(season)
        .bind(ap_slot)
        .bind(ts)
        .bind(game)
        .bind(sent)
        .bind(received)
        .bind(count)
        .bind(locations)
        .bind(approx as i64)
        .bind(err)
        .execute(&self.pool)
        .await;
    }

    /// The newest `limit` activity rows for a season (board preview).
    pub async fn activity_latest(&self, season: &str, limit: i64) -> Vec<ActivityRow> {
        sqlx::query_as::<_, ActivityRow>(
            "SELECT ts, kind, text, item_class FROM activity
             WHERE season = ? ORDER BY id DESC LIMIT ?",
        )
        .bind(season)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// Append the active WHERE conditions for a filter onto a query builder that
    /// already has `WHERE season = <bound>`.
    fn apply_activity_filter<'a>(
        qb: &mut sqlx::QueryBuilder<'a, sqlx::Sqlite>,
        f: &'a ActFilter<'a>,
    ) {
        if !f.kinds.is_empty() {
            qb.push(" AND kind IN (");
            let mut sep = qb.separated(", ");
            for k in f.kinds {
                sep.push_bind(*k);
            }
            qb.push(")");
        }
        if !f.classes.is_empty() {
            qb.push(" AND item_class IN (");
            let mut sep = qb.separated(", ");
            for c in f.classes {
                sep.push_bind(*c);
            }
            qb.push(")");
        }
        if let Some(s) = f.sender {
            qb.push(" AND sender = ").push_bind(s).push(" COLLATE NOCASE");
        }
        if let Some(r) = f.receiver {
            qb.push(" AND receiver = ").push_bind(r).push(" COLLATE NOCASE");
        }
        if let Some(t) = f.since {
            qb.push(" AND ts >= ").push_bind(t);
        }
        if let Some(t) = f.until {
            qb.push(" AND ts <= ").push_bind(t);
        }
    }

    /// Total activity rows for a season matching the filter (for pagination).
    pub async fn activity_count(&self, season: &str, f: &ActFilter<'_>) -> i64 {
        let mut qb = sqlx::QueryBuilder::new("SELECT COUNT(*) FROM activity WHERE season = ");
        qb.push_bind(season);
        Self::apply_activity_filter(&mut qb, f);
        qb.build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0)
    }

    /// A page of a season's filtered activity, newest first, by offset
    /// (page p → offset `(p-1)*limit`). Returns up to `limit` rows.
    pub async fn activity_page(
        &self,
        season: &str,
        f: &ActFilter<'_>,
        offset: i64,
        limit: i64,
    ) -> Vec<ActivityRow> {
        let mut qb =
            sqlx::QueryBuilder::new("SELECT ts, kind, text, item_class FROM activity WHERE season = ");
        qb.push_bind(season);
        Self::apply_activity_filter(&mut qb, f);
        qb.push(" ORDER BY id DESC LIMIT ")
            .push_bind(limit)
            .push(" OFFSET ")
            .push_bind(offset.max(0));
        qb.build_query_as::<ActivityRow>()
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default()
    }

    /// A page of activity involving one participant — as sender OR receiver —
    /// newest first, optionally narrowed to rows whose text contains `q`
    /// (case-insensitive substring; empty `q` matches all). Powers a game page's
    /// own paginated, searchable "Log" tab.
    pub async fn activity_for_participant(
        &self,
        season: &str,
        name: &str,
        q: &str,
        offset: i64,
        limit: i64,
    ) -> Vec<ActivityRow> {
        sqlx::query_as::<_, ActivityRow>(
            "SELECT ts, kind, text, item_class FROM activity
             WHERE season = ?1
               AND (sender = ?2 COLLATE NOCASE OR receiver = ?2 COLLATE NOCASE)
               AND (?3 = '' OR instr(lower(text), lower(?3)) > 0)
             ORDER BY id DESC LIMIT ?4 OFFSET ?5",
        )
        .bind(season)
        .bind(name)
        .bind(q)
        .bind(limit)
        .bind(offset.max(0))
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// Total activity rows involving one participant matching `q` (empty = all),
    /// for the Log tab's pager.
    pub async fn activity_count_for_participant(&self, season: &str, name: &str, q: &str) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM activity
             WHERE season = ?1
               AND (sender = ?2 COLLATE NOCASE OR receiver = ?2 COLLATE NOCASE)
               AND (?3 = '' OR instr(lower(text), lower(?3)) > 0)",
        )
        .bind(season)
        .bind(name)
        .bind(q)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0)
    }

    /// Distinct participant names (senders ∪ receivers) in a season, for the
    /// filter's sender/receiver autocomplete.
    pub async fn activity_participants(&self, season: &str) -> Vec<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT name FROM (
                 SELECT sender AS name FROM activity WHERE season = ?1 AND sender IS NOT NULL
                 UNION SELECT receiver FROM activity WHERE season = ?1 AND receiver IS NOT NULL
             ) ORDER BY name COLLATE NOCASE",
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// The set of prioritized hint keys (finding, location, item) for a season.
    pub async fn priorities(
        &self,
        season: &str,
    ) -> Result<std::collections::HashSet<(i64, i64, i64)>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (i64, i64, i64)>(
            "SELECT finding, location, item FROM hint_priority WHERE season = ?",
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().collect())
    }

    /// Toggle a hint's priority. Returns true if it is now prioritized.
    pub async fn toggle_priority(
        &self,
        season: &str,
        finding: i64,
        location: i64,
        item: i64,
    ) -> Result<bool, sqlx::Error> {
        let del = sqlx::query(
            "DELETE FROM hint_priority WHERE season=? AND finding=? AND location=? AND item=?",
        )
        .bind(season)
        .bind(finding)
        .bind(location)
        .bind(item)
        .execute(&self.pool)
        .await?;
        if del.rows_affected() > 0 {
            return Ok(false);
        }
        sqlx::query(
            "INSERT OR IGNORE INTO hint_priority (season, finding, location, item) VALUES (?,?,?,?)",
        )
        .bind(season)
        .bind(finding)
        .bind(location)
        .bind(item)
        .execute(&self.pool)
        .await?;
        Ok(true)
    }

    /// Remove priority rows for the given (finding, location, item) hints — used
    /// to drop the star from hints whose item has since been found.
    pub async fn unprioritize_many(&self, season: &str, hints: &[(i64, i64, i64)]) {
        for (finding, location, item) in hints {
            let _ = sqlx::query(
                "DELETE FROM hint_priority WHERE season=? AND finding=? AND location=? AND item=?",
            )
            .bind(season)
            .bind(finding)
            .bind(location)
            .bind(item)
            .execute(&self.pool)
            .await;
        }
    }

    /// Seed a season's Bifrost slots from the AP roster (1:1), then add any
    /// co-op extras (`ap_slot_name -> extra_bifrost_name`). Idempotent.
    pub async fn seed_season(
        &self,
        season: &str,
        ap_slots: &[(i64, String, String)],
        coop: &[(String, String)],
    ) -> Result<(), sqlx::Error> {
        for (num, name, game) in ap_slots {
            sqlx::query(
                "INSERT OR IGNORE INTO bifrost_slot (season, name, ap_slot, game) VALUES (?, ?, ?, ?)",
            )
            .bind(season)
            .bind(name)
            .bind(*num)
            .bind(game)
            .execute(&self.pool)
            .await?;
        }
        for (ap_name, extra) in coop {
            if let Some((num, _, game)) = ap_slots.iter().find(|(_, n, _)| n == ap_name) {
                sqlx::query(
                    "INSERT OR IGNORE INTO bifrost_slot (season, name, ap_slot, game) VALUES (?, ?, ?, ?)",
                )
                .bind(season)
                .bind(extra)
                .bind(*num)
                .bind(game)
                .execute(&self.pool)
                .await?;
            }
        }
        Ok(())
    }

    /// Seed a freshly-generated season from submissions: 1:1 Bifrost slots from
    /// the AP roster, each matching submission's uploader auto-claimed, and a
    /// co-op Bifrost slot minted (unclaimed) per co-op name.
    pub async fn seed_from_submissions(
        &self,
        season: &str,
        ap_slots: &[(i64, String, String)],
        subs: &[SubmissionRow],
        common_slot: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        for (num, name, game) in ap_slots {
            sqlx::query(
                "INSERT OR IGNORE INTO bifrost_slot (season, name, ap_slot, game) VALUES (?, ?, ?, ?)",
            )
            .bind(season)
            .bind(name)
            .bind(num)
            .bind(game)
            .execute(&self.pool)
            .await?;
            let id: i64 =
                sqlx::query_scalar("SELECT id FROM bifrost_slot WHERE season = ? AND name = ?")
                    .bind(season)
                    .bind(name)
                    .fetch_one(&self.pool)
                    .await?;
            if let Some(sub) = subs.iter().find(|s| &s.slot_name == name) {
                sqlx::query(
                    "INSERT OR IGNORE INTO claim (bifrost_slot_id, sso, claimed_at) VALUES (?, ?, ?)",
                )
                .bind(id)
                .bind(&sub.sso)
                .bind(now)
                .execute(&self.pool)
                .await?;
                for coop in sub
                    .coop_names
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    sqlx::query(
                        "INSERT OR IGNORE INTO bifrost_slot (season, name, ap_slot, game) VALUES (?, ?, ?, ?)",
                    )
                    .bind(season)
                    .bind(coop)
                    .bind(num)
                    .bind(game)
                    .execute(&self.pool)
                    .await?;
                }
            }
        }
        // Mark the communal common-game slot (no claim; shared by everyone).
        if let Some(common) = common_slot {
            sqlx::query("UPDATE bifrost_slot SET is_common = 1 WHERE season = ? AND name = ?")
                .bind(season)
                .bind(common)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    /// Re-seed a freshly regenerated season from the previous one: recreate every
    /// Bifrost slot (1:1 AP slots + co-op seats) mapped onto the new AP slot
    /// numbers by name, and carry each claim over. Player names are stable across
    /// a re-roll, so claims and co-op grouping survive.
    pub async fn carry_over_season(
        &self,
        old_season: &str,
        new_season: &str,
        new_ap_slots: &[(i64, String, String)],
    ) -> Result<(), sqlx::Error> {
        use std::collections::{HashMap, HashSet};
        let old = self.slots(old_season).await?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let roster: HashSet<&str> = new_ap_slots.iter().map(|(_, n, _)| n.as_str()).collect();
        // Names that were communal common-game slots last season stay communal.
        let common_names: HashSet<&str> =
            old.iter().filter(|s| s.is_common).map(|s| s.name.as_str()).collect();
        let new_ap_by_name: HashMap<&str, i64> = new_ap_slots
            .iter()
            .map(|(n, name, _)| (name.as_str(), *n))
            .collect();
        // For each old AP slot number, the roster name occupying it (its "base").
        let mut base_by_old_ap: HashMap<i64, String> = HashMap::new();
        for s in &old {
            if roster.contains(s.name.as_str()) {
                base_by_old_ap
                    .entry(s.ap_slot)
                    .or_insert_with(|| s.name.clone());
            }
        }
        // 1:1 AP slots.
        for (num, name, game) in new_ap_slots {
            sqlx::query(
                "INSERT OR IGNORE INTO bifrost_slot (season, name, ap_slot, game, is_common) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(new_season)
            .bind(name)
            .bind(num)
            .bind(game)
            .bind(common_names.contains(name.as_str()))
            .execute(&self.pool)
            .await?;
        }
        // Co-op seats (names not in the AP roster) → their base's new AP slot.
        for s in &old {
            if roster.contains(s.name.as_str()) {
                continue;
            }
            let Some(base) = base_by_old_ap.get(&s.ap_slot) else {
                continue;
            };
            let Some(new_ap) = new_ap_by_name.get(base.as_str()) else {
                continue;
            };
            sqlx::query(
                "INSERT OR IGNORE INTO bifrost_slot (season, name, ap_slot, game) VALUES (?, ?, ?, ?)",
            )
            .bind(new_season)
            .bind(&s.name)
            .bind(new_ap)
            .bind(&s.game)
            .execute(&self.pool)
            .await?;
        }
        // Carry claims over by name.
        for s in &old {
            let Some(sso) = &s.claimed_by else { continue };
            let id: Option<i64> =
                sqlx::query_scalar("SELECT id FROM bifrost_slot WHERE season = ? AND name = ?")
                    .bind(new_season)
                    .bind(&s.name)
                    .fetch_optional(&self.pool)
                    .await?;
            if let Some(id) = id {
                sqlx::query(
                    "INSERT OR IGNORE INTO claim (bifrost_slot_id, sso, claimed_at) VALUES (?, ?, ?)",
                )
                .bind(id)
                .bind(sso)
                .bind(now)
                .execute(&self.pool)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn slots(&self, season: &str) -> Result<Vec<SlotRow>, sqlx::Error> {
        sqlx::query_as::<_, SlotRow>(
            r#"SELECT s.id, s.name, s.ap_slot, s.game, c.sso AS claimed_by, s.is_common
               FROM bifrost_slot s LEFT JOIN claim c ON c.bifrost_slot_id = s.id
               WHERE s.season = ?
               ORDER BY s.ap_slot, s.name"#,
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await
    }

    /// Claim a slot, enforcing one claim per player per season.
    pub async fn claim_one(&self, slot_id: i64, sso: &str) -> Result<ClaimOutcome, sqlx::Error> {
        let season: Option<String> =
            sqlx::query_scalar("SELECT season FROM bifrost_slot WHERE id = ?")
                .bind(slot_id)
                .fetch_optional(&self.pool)
                .await?;
        let Some(season) = season else {
            return Ok(ClaimOutcome::SlotTaken);
        };
        // Already holding a *different* slot this season? One per player.
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT s.name FROM claim c JOIN bifrost_slot s ON s.id = c.bifrost_slot_id
             WHERE s.season = ? AND c.sso = ? AND s.id <> ?",
        )
        .bind(&season)
        .bind(sso)
        .bind(slot_id)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(name) = existing {
            return Ok(ClaimOutcome::AlreadyHave(name));
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let res = sqlx::query(
            "INSERT OR IGNORE INTO claim (bifrost_slot_id, sso, claimed_at) VALUES (?, ?, ?)",
        )
        .bind(slot_id)
        .bind(sso)
        .bind(now)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() > 0 {
            return Ok(ClaimOutcome::Ok);
        }
        // Slot was already claimed — fine if it's this same user, else taken.
        let owner: Option<String> =
            sqlx::query_scalar("SELECT sso FROM claim WHERE bifrost_slot_id = ?")
                .bind(slot_id)
                .fetch_optional(&self.pool)
                .await?;
        if owner.as_deref() == Some(sso) {
            Ok(ClaimOutcome::Ok)
        } else {
            Ok(ClaimOutcome::SlotTaken)
        }
    }

    /// Record a signup submission. Errors (e.g. duplicate slot name) bubble up.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_submission(
        &self,
        sso: &str,
        slot_name: &str,
        game: &str,
        is_custom: bool,
        coop_names: &str,
        yaml_file: &str,
        apworld_file: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let res = sqlx::query(
            "INSERT INTO submission (sso, slot_name, game, is_custom, coop_names, yaml_file, apworld_file, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(sso)
        .bind(slot_name)
        .bind(game)
        .bind(is_custom as i64)
        .bind(coop_names)
        .bind(yaml_file)
        .bind(apworld_file)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    pub async fn submissions(&self) -> Result<Vec<SubmissionRow>, sqlx::Error> {
        sqlx::query_as::<_, SubmissionRow>(
            "SELECT id, sso, slot_name, game, is_custom, coop_names, yaml_file
             FROM submission ORDER BY created_at",
        )
        .fetch_all(&self.pool)
        .await
    }

    /// Delete a submission (its own submitter, or an admin). Returns the deleted
    /// row's file paths so the caller can clean them up.
    pub async fn delete_submission(
        &self,
        id: i64,
        sso: &str,
        is_admin: bool,
    ) -> Result<Option<(String, Option<String>)>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, String, Option<String>)>(
            "SELECT sso, yaml_file, apworld_file FROM submission WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((owner, yaml_file, apworld_file)) = row else {
            return Ok(None);
        };
        if !is_admin && owner != sso {
            return Ok(None);
        }
        sqlx::query("DELETE FROM submission WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(Some((yaml_file, apworld_file)))
    }

    /// Whether `sso` claims any Bifrost slot mapping to `ap_slot` this season.
    pub async fn claims_ap_slot(&self, season: &str, sso: &str, ap_slot: i64) -> bool {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM bifrost_slot s JOIN claim c ON c.bifrost_slot_id = s.id
             WHERE s.season = ? AND s.ap_slot = ? AND c.sso = ?",
        )
        .bind(season)
        .bind(ap_slot)
        .bind(sso)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        n > 0
    }

    /// (ap_slot, current claimant) for a Bifrost slot, or None if it doesn't exist.
    pub async fn slot_detail(
        &self,
        id: i64,
    ) -> Result<Option<(i64, Option<String>, bool)>, sqlx::Error> {
        sqlx::query_as::<_, (i64, Option<String>, bool)>(
            "SELECT s.ap_slot, c.sso, s.is_common FROM bifrost_slot s
             LEFT JOIN claim c ON c.bifrost_slot_id = s.id WHERE s.id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    /// Release a claim. A non-admin may only release their own; an admin any.
    pub async fn unclaim(
        &self,
        slot_id: i64,
        sso: &str,
        is_admin: bool,
    ) -> Result<bool, sqlx::Error> {
        let res = if is_admin {
            sqlx::query("DELETE FROM claim WHERE bifrost_slot_id = ?")
                .bind(slot_id)
                .execute(&self.pool)
                .await?
        } else {
            sqlx::query("DELETE FROM claim WHERE bifrost_slot_id = ? AND sso = ?")
                .bind(slot_id)
                .bind(sso)
                .execute(&self.pool)
                .await?
        };
        Ok(res.rows_affected() > 0)
    }
}
