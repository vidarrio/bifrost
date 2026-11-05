//! Persistence for Bifrost slots + claims (SQLite).
//!
//! A **Bifrost slot** is one person (claimable, tied to an account), mapping to one
//! **AP slot** (the game). Co-op games (e.g. Don't Starve Together) have several
//! Bifrost slots pointing at the same `ap_slot`. A season is keyed by the AP
//! `seed_name`, so a new seed gets a fresh set of slots + claims.

pub use crate::id::Id;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{FromRow, SqlitePool};

mod auth;
mod common;
mod migrate;
pub use migrate::SCHEMA_VERSION;
mod seasons;
mod seats;
mod settings;
mod worlds;
pub use auth::{AccountRow, LinkKind, LinkRow, RedeemError};
pub use common::{SeasonCommon, SetIdentOutcome};
pub use migrate::PRE_0_3;
pub use seasons::{MAX_DAILY_HINTS, MAX_GAMES_LIMIT, SeasonRow, State};
pub use seats::{JoinOutcome, SeatLink, SeatRow, name_clash, seat_name_error};
pub use settings::{Approval, Discord, Instance};
pub use worlds::{CatalogWorld, WorldRequest};

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
    /// The AP port range live seasons draw from (managed mode); `None` means
    /// one external server, so only one season can be live.
    ap_ports: Option<std::ops::RangeInclusive<u16>>,
}

#[derive(Debug, FromRow, Clone)]
pub struct SlotRow {
    pub id: Id,
    /// The game's id in URLs, shared by the slots of a co-op game.
    pub game_id: Id,
    /// A co-op seat, whose name Archipelago doesn't know.
    pub is_seat: bool,
    pub name: String,
    pub ap_slot: i64,
    pub game: String,
    /// The claimant's current username (display) and account (authorization).
    pub claimed_by: Option<String>,
    pub claimed_by_id: Option<i64>,
    /// A common game's slot: one AP slot everyone in the season plays, with no
    /// claim. Its setup's key in the catalog.
    #[sqlx(default)]
    pub common_key: Option<String>,
}

impl SlotRow {
    pub fn is_common(&self) -> bool {
        self.common_key.is_some()
    }
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
        if t == 0 { 0 } else { (self.wins * 100) / t }
    }
}

/// Result of an attempted claim under the one-slot-per-player rule.
pub enum ClaimOutcome {
    Ok,
    /// The slot is already claimed by someone else.
    SlotTaken,
}

#[derive(Debug, FromRow, Clone)]
pub struct SubmissionRow {
    pub id: Id,
    pub account_id: i64,
    /// The submitter's current username.
    pub username: String,
    pub slot_name: String,
    pub game: String,
    pub is_custom: bool,
    pub yaml_file: String,
    /// Held until this apworld (a pending upload's file name) is approved;
    /// its YAML can't be checked before then.
    pub held_for: Option<String>,
    /// Why a held submission can't go ahead (its check failed after approval,
    /// or the apworld was rejected).
    pub held_err: Option<String>,
    /// Shared notes for the season's other players.
    pub notes: String,
    /// Partners' seat names, comma-separated.
    pub seat_names: String,
}

/// The slot Bifrost's own connections join as: the configured one if it's a real
/// slot of this season, else the lowest. Never a co-op seat, which the server
/// doesn't know.
pub fn connect_slot(slots: &[SlotRow], configured: Option<&str>) -> Option<String> {
    let real = || slots.iter().filter(|r| !r.is_seat);
    configured
        .filter(|n| real().any(|r| r.name == *n))
        .map(str::to_string)
        .or_else(|| real().min_by_key(|r| r.ap_slot).map(|r| r.name.clone()))
}

impl SubmissionRow {
    pub fn held(&self) -> bool {
        self.held_for.is_some()
    }
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
    /// Give each live season its own AP port from `range` (managed mode).
    pub fn with_ap_ports(mut self, range: std::ops::RangeInclusive<u16>) -> Self {
        self.ap_ports = Some(range);
        self
    }

    /// Open the database and bring its schema up to date (tests; startup
    /// goes through `main::upgrade`).
    #[cfg(test)]
    pub async fn open(path: &str) -> Result<Self, sqlx::Error> {
        let db = Self::connect(path).await?;
        db.migrate().await?;
        Ok(db)
    }

    /// Open the database without touching its schema, so startup can look at
    /// its [`Stamp`] (and back it up) before [`Db::migrate`] changes it.
    /// Close the database, which folds the write-ahead log back into its file.
    pub async fn close(&self) {
        if let Err(e) = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.pool)
            .await
        {
            tracing::warn!("folding the database's log into it: {e}");
        }
        self.pool.close().await;
    }

    pub async fn connect(path: &str) -> Result<Self, sqlx::Error> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            // Readers don't wait for writers. Fully synced, so that a revoked session or a
            // spent link stays that way through a power loss.
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;
        Ok(Self {
            pool,
            ap_ports: None,
        })
    }
}

impl Db {
    /// A player's bonus-hint credits for a season.
    pub async fn credits(&self, season: Id, account: i64) -> i64 {
        sqlx::query_scalar("SELECT credits FROM hint_credit WHERE season_id = ? AND account_id = ?")
            .bind(season)
            .bind(account)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(0)
    }

    /// True if this account has claimed a regular (non-common) slot this season.
    /// Communal-hint gate: only players with a game of their own in the season may
    /// spend the common game's shared hint points.
    pub async fn has_claimed_slot(&self, season: Id, account: i64) -> bool {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM claim c JOIN slot s ON s.id = c.slot_id
             WHERE s.season_id = ? AND c.account_id = ? AND s.common_key IS NULL",
        )
        .bind(season)
        .bind(account)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        n > 0
    }

    /// Grant `n` bonus-hint credits, returning the new total.
    pub async fn add_credits(&self, season: Id, account: i64, n: i64) -> Result<i64, sqlx::Error> {
        sqlx::query(
            "INSERT INTO hint_credit (season_id, account_id, credits) VALUES (?, ?, ?)
             ON CONFLICT(season_id, account_id) DO UPDATE SET credits = credits + excluded.credits",
        )
        .bind(season)
        .bind(account)
        .bind(n)
        .execute(&self.pool)
        .await?;
        Ok(self.credits(season, account).await)
    }

    /// Spend one bonus-hint credit if available; returns true if one was spent.
    pub async fn spend_credit(&self, season: Id, account: i64) -> bool {
        let res = sqlx::query(
            "UPDATE hint_credit SET credits = credits - 1
             WHERE season_id = ? AND account_id = ? AND credits > 0",
        )
        .bind(season)
        .bind(account)
        .execute(&self.pool)
        .await;
        matches!(res, Ok(r) if r.rows_affected() > 0)
    }

    /// Record a finished minesweeper game (win or loss, with its duration) for a
    /// given board kind ("standard" / "expert"). False if it couldn't be stored.
    pub async fn record_mine_result(
        &self,
        season: Option<Id>,
        account: i64,
        kind: &str,
        won: bool,
        duration_ms: i64,
    ) -> Option<i64> {
        sqlx::query(
            "INSERT INTO mine_result (season_id, account_id, kind, won, duration_ms, ts)
             VALUES (?, ?, ?, ?, ?, strftime('%s','now'))",
        )
        .bind(season)
        .bind(account)
        .bind(kind)
        .bind(won as i64)
        .bind(duration_ms)
        .execute(&self.pool)
        .await
        .ok()
        .map(|r| r.last_insert_rowid())
    }

    /// Award a bonus hint for the seasonal win recorded as `result`, unless
    /// the account has already earned `cap` of them in the last `secs`
    /// seconds (`cap` 0: no limit). Marking the win and adding the credit
    /// happen in one transaction, and the count is taken under the write
    /// lock, so parallel wins can't overshoot the cap. Returns the new credit
    /// total, or `None` when the cap was reached.
    pub async fn award_mine_hint(
        &self,
        result: i64,
        season: Id,
        account: i64,
        cap: i64,
        secs: i64,
    ) -> Result<Option<i64>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let marked = sqlx::query(
            "UPDATE mine_result SET earned = 1
             WHERE id = ?1 AND season_id = ?2 AND account_id = ?3 AND won = 1 AND earned = 0
               AND (?4 = 0 OR (SELECT COUNT(*) FROM mine_result
                               WHERE season_id = ?2 AND account_id = ?3 AND earned = 1
                                 AND ts > strftime('%s','now') - ?5) < ?4)",
        )
        .bind(result)
        .bind(season)
        .bind(account)
        .bind(cap)
        .bind(secs)
        .execute(&mut *tx)
        .await?;
        if marked.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(None);
        }
        sqlx::query(
            "INSERT INTO hint_credit (season_id, account_id, credits) VALUES (?, ?, 1)
             ON CONFLICT(season_id, account_id) DO UPDATE SET credits = credits + 1",
        )
        .bind(season)
        .bind(account)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(self.credits(season, account).await))
    }

    /// Per-kind minesweeper stats for a player. `season = Some(s)` scopes to one
    /// season; `None` is all-time (across every season + freeplay) — the
    /// "permanent" view. Time stats are over wins only; `None` = no wins yet.
    pub async fn mine_stats(&self, season: Option<Id>, account: i64, kind: &str) -> MineStats {
        let mut sql = String::from(
            "SELECT COALESCE(SUM(won),0) AS wins, COALESCE(SUM(1-won),0) AS losses, \
             CAST(AVG(CASE WHEN won=1 THEN duration_ms END) AS INTEGER) AS avg_ms, \
             MIN(CASE WHEN won=1 THEN duration_ms END) AS best_ms, \
             MAX(CASE WHEN won=1 THEN duration_ms END) AS worst_ms \
             FROM mine_result WHERE account_id=? AND kind=?",
        );
        if season.is_some() {
            sql.push_str(" AND season_id=?");
        }
        let mut q = sqlx::query_as::<_, MineStats>(sqlx::AssertSqlSafe(sql))
            .bind(account)
            .bind(kind);
        if let Some(s) = season {
            q = q.bind(s);
        }
        q.fetch_one(&self.pool).await.unwrap_or_default()
    }

    /// Best-time leaderboard: (player, best_ms) over won games, fastest first,
    /// top `limit`. `season = None` is the all-time (permanent) board.
    pub async fn mine_leaders_time(
        &self,
        season: Option<Id>,
        kind: &str,
        limit: i64,
    ) -> Vec<(String, i64)> {
        let mut sql = String::from(
            "SELECT a.username, MIN(m.duration_ms) AS best FROM mine_result m \
             JOIN account a ON a.id = m.account_id WHERE m.kind=? AND m.won=1",
        );
        if season.is_some() {
            sql.push_str(" AND m.season_id=?");
        }
        sql.push_str(" GROUP BY m.account_id ORDER BY best ASC LIMIT ?");
        let mut q = sqlx::query_as::<_, (String, i64)>(sqlx::AssertSqlSafe(sql)).bind(kind);
        if let Some(s) = season {
            q = q.bind(s);
        }
        q.bind(limit)
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default()
    }

    /// Best completion-rate leaderboard: (player, wins, total) for players with
    /// at least `min_games`, highest rate first (more games breaks ties), top
    /// `limit`. `season = None` is the all-time (permanent) board.
    pub async fn mine_leaders_rate(
        &self,
        season: Option<Id>,
        kind: &str,
        min_games: i64,
        limit: i64,
    ) -> Vec<(String, i64, i64)> {
        let mut sql = String::from(
            "SELECT a.username, SUM(m.won) AS wins, COUNT(*) AS total FROM mine_result m \
             JOIN account a ON a.id = m.account_id WHERE m.kind=?",
        );
        if season.is_some() {
            sql.push_str(" AND m.season_id=?");
        }
        sql.push_str(
            " GROUP BY m.account_id HAVING total>=? ORDER BY (CAST(SUM(m.won) AS REAL)/COUNT(*)) DESC, total DESC LIMIT ?",
        );
        let mut q = sqlx::query_as::<_, (String, i64, i64)>(sqlx::AssertSqlSafe(sql)).bind(kind);
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
    pub async fn clear_submissions(&self, season: Id) -> Result<Vec<String>, sqlx::Error> {
        let files: Vec<String> =
            sqlx::query_scalar("SELECT yaml_file FROM submission WHERE season_id = ?")
                .bind(season)
                .fetch_all(&self.pool)
                .await?;
        sqlx::query("DELETE FROM submission WHERE season_id = ?")
            .bind(season)
            .execute(&self.pool)
            .await?;
        Ok(files)
    }

    /// Delete a season's Bifrost slots (claims cascade), before a re-roll rebuilds them.
    pub async fn clear_all_slots(&self, season: Id) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM slot WHERE season_id = ?")
            .bind(season)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Append an activity row (ingest path; dedup is via the cursor). `sender`
    /// is the actor (sender of an item, or the player who goaled/released/
    /// collected); `receiver` is the item's recipient (sends only, else None).
    #[allow(clippy::too_many_arguments)]
    pub async fn activity_add(
        &self,
        season: Id,
        ts: i64,
        kind: &str,
        text: &str,
        sender: Option<&str>,
        receiver: Option<&str>,
        item_class: Option<&str>,
    ) {
        let _ = sqlx::query(
            "INSERT INTO activity (season_id, ts, kind, text, sender, receiver, item_class)
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
    pub async fn activity_any(&self, season: Id) -> bool {
        sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM activity WHERE season_id = ?)")
            .bind(season)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0)
            == 1
    }

    /// Cached in-logic result for one slot, if computed this season.
    pub async fn inlogic_get(&self, season: Id, ap_slot: i64) -> Option<InLogic> {
        sqlx::query_as::<_, InLogic>(
            "SELECT ts, locations, err FROM inlogic WHERE season_id = ? AND ap_slot = ?",
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
    pub async fn inlogic_counts(&self, season: Id) -> std::collections::HashMap<i64, i64> {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT ap_slot, count FROM inlogic WHERE season_id = ? AND err IS NULL",
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await
        .map(|rows| rows.into_iter().collect())
        .unwrap_or_default()
    }

    /// The `(sent, received)` state a slot's cached in-logic was computed at, so
    /// the recomputer can skip slots whose state hasn't moved. `None` = no row.
    pub async fn inlogic_state(&self, season: Id, ap_slot: i64) -> Option<(i64, i64)> {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT sent, received FROM inlogic WHERE season_id = ? AND ap_slot = ?",
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
        season: Id,
        ap_slot: i64,
        ts: i64,
        sent: i64,
        received: i64,
        count: i64,
        locations: &str,
        err: Option<&str>,
    ) {
        let _ = sqlx::query(
            "INSERT INTO inlogic (season_id, ap_slot, ts, sent, received, count, locations, err)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(season_id, ap_slot) DO UPDATE SET
               ts = excluded.ts, sent = excluded.sent,
               received = excluded.received, count = excluded.count,
               locations = excluded.locations, err = excluded.err",
        )
        .bind(season)
        .bind(ap_slot)
        .bind(ts)
        .bind(sent)
        .bind(received)
        .bind(count)
        .bind(locations)
        .bind(err)
        .execute(&self.pool)
        .await;
    }

    /// The newest `limit` activity rows for a season (board preview).
    pub async fn activity_latest(&self, season: Id, limit: i64) -> Vec<ActivityRow> {
        sqlx::query_as::<_, ActivityRow>(
            "SELECT ts, kind, text, item_class FROM activity
             WHERE season_id = ? ORDER BY id DESC LIMIT ?",
        )
        .bind(season)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// Append the active WHERE conditions for a filter onto a query builder that
    /// already has `WHERE season = <bound>`.
    fn apply_activity_filter(qb: &mut sqlx::QueryBuilder<sqlx::Sqlite>, f: &ActFilter<'_>) {
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
            qb.push(" AND sender = ")
                .push_bind(s)
                .push(" COLLATE NOCASE");
        }
        if let Some(r) = f.receiver {
            qb.push(" AND receiver = ")
                .push_bind(r)
                .push(" COLLATE NOCASE");
        }
        if let Some(t) = f.since {
            qb.push(" AND ts >= ").push_bind(t);
        }
        if let Some(t) = f.until {
            qb.push(" AND ts <= ").push_bind(t);
        }
    }

    /// Total activity rows for a season matching the filter (for pagination).
    pub async fn activity_count(&self, season: Id, f: &ActFilter<'_>) -> i64 {
        let mut qb = sqlx::QueryBuilder::new("SELECT COUNT(*) FROM activity WHERE season_id = ");
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
        season: Id,
        f: &ActFilter<'_>,
        offset: i64,
        limit: i64,
    ) -> Vec<ActivityRow> {
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT ts, kind, text, item_class FROM activity WHERE season_id = ",
        );
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
        season: Id,
        name: &str,
        q: &str,
        offset: i64,
        limit: i64,
    ) -> Vec<ActivityRow> {
        sqlx::query_as::<_, ActivityRow>(
            "SELECT ts, kind, text, item_class FROM activity
             WHERE season_id = ?1
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
    pub async fn activity_count_for_participant(&self, season: Id, name: &str, q: &str) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM activity
             WHERE season_id = ?1
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
    pub async fn activity_participants(&self, season: Id) -> Vec<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT name FROM (
                 SELECT sender AS name FROM activity WHERE season_id = ?1 AND sender IS NOT NULL
                 UNION SELECT receiver FROM activity WHERE season_id = ?1 AND receiver IS NOT NULL
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
        season: Id,
    ) -> Result<std::collections::HashSet<(i64, i64, i64)>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (i64, i64, i64)>(
            "SELECT finding, location, item FROM hint_priority WHERE season_id = ?",
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().collect())
    }

    /// Toggle a hint's priority. Returns true if it is now prioritized.
    pub async fn toggle_priority(
        &self,
        season: Id,
        finding: i64,
        location: i64,
        item: i64,
    ) -> Result<bool, sqlx::Error> {
        let del = sqlx::query(
            "DELETE FROM hint_priority WHERE season_id=? AND finding=? AND location=? AND item=?",
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
            "INSERT OR IGNORE INTO hint_priority (season_id, finding, location, item) VALUES (?,?,?,?)",
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
    pub async fn unprioritize_many(&self, season: Id, hints: &[(i64, i64, i64)]) {
        for (finding, location, item) in hints {
            let _ = sqlx::query(
                "DELETE FROM hint_priority WHERE season_id=? AND finding=? AND location=? AND item=?",
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
        season: Id,
        ap_slots: &[(i64, String, String)],
        coop: &[(String, String)],
    ) -> Result<(), sqlx::Error> {
        let add = |name: String, num: i64, game: String, is_seat: bool| async move {
            // Slots on one AP slot are one game, so they share its id.
            sqlx::query(
                "INSERT OR IGNORE INTO slot (id, season_id, game_id, name, ap_slot, game, is_seat)
                 VALUES (?1, ?2,
                         COALESCE((SELECT game_id FROM slot WHERE season_id = ?2 AND ap_slot = ?5 LIMIT 1), ?3),
                         ?4, ?5, ?6, ?7)",
            )
            .bind(Id::new())
            .bind(season)
            .bind(Id::new())
            .bind(name)
            .bind(num)
            .bind(game)
            .bind(is_seat)
            .execute(&self.pool)
            .await
        };
        for (num, name, game) in ap_slots {
            add(name.clone(), *num, game.clone(), false).await?;
        }
        for (ap_name, extra) in coop {
            if let Some((num, _, game)) = ap_slots.iter().find(|(_, n, _)| n == ap_name) {
                add(extra.clone(), *num, game.clone(), true).await?;
            }
        }
        Ok(())
    }

    /// Seed a freshly-generated season from submissions: 1:1 Bifrost slots from
    /// the AP roster, each matching submission's uploader auto-claimed, and a
    /// co-op Bifrost slot minted per seat, also claimed.
    pub async fn seed_from_submissions(
        &self,
        season: Id,
        ap_slots: &[(i64, String, String)],
        subs: &[SubmissionRow],
        commons: &[(&str, &str)],
    ) -> Result<(), sqlx::Error> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        // A seat's name was free when it was picked, so an AP slot with that name
        // is a clash: fail rather than seat them on another game.
        for sub in subs {
            for seat in self.try_seats_of(sub.id).await? {
                if ap_slots.iter().any(|(_, name, _)| *name == seat.name) {
                    return Err(sqlx::Error::Protocol(format!(
                        "seat name {} is already another slot's",
                        seat.name
                    )));
                }
            }
        }
        for (num, name, game) in ap_slots {
            // A signup's game keeps its id; any other gets its own.
            let sub = subs.iter().find(|s| &s.slot_name == name);
            let game_id = sub.map_or_else(Id::new, |s| s.id);
            sqlx::query(
                "INSERT OR IGNORE INTO slot (id, season_id, game_id, name, ap_slot, game)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(Id::new())
            .bind(season)
            .bind(game_id)
            .bind(name)
            .bind(num)
            .bind(game)
            .execute(&self.pool)
            .await?;
            let id: Id = sqlx::query_scalar("SELECT id FROM slot WHERE season_id = ? AND name = ?")
                .bind(season)
                .bind(name)
                .fetch_one(&self.pool)
                .await?;
            let Some(sub) = sub else { continue };
            sqlx::query(
                "INSERT OR IGNORE INTO claim (slot_id, account_id, claimed_at) VALUES (?, ?, ?)",
            )
            .bind(id)
            .bind(sub.account_id)
            .bind(now)
            .execute(&self.pool)
            .await?;
            for seat in self.try_seats_of(sub.id).await? {
                sqlx::query(
                    "INSERT OR IGNORE INTO slot (id, season_id, game_id, name, ap_slot, game, is_seat)
                     VALUES (?, ?, ?, ?, ?, ?, 1)",
                )
                .bind(Id::new())
                .bind(season)
                .bind(game_id)
                .bind(&seat.name)
                .bind(num)
                .bind(game)
                .execute(&self.pool)
                .await?;
                sqlx::query(
                    "INSERT OR IGNORE INTO claim (slot_id, account_id, claimed_at)
                     SELECT id, ?, ? FROM slot WHERE season_id = ? AND name = ?",
                )
                .bind(seat.account_id)
                .bind(now)
                .bind(season)
                .bind(&seat.name)
                .execute(&self.pool)
                .await?;
            }
        }
        // Mark the common games' slots (no claim; shared by everyone), by
        // (slot name, setup key).
        for (name, key) in commons {
            sqlx::query("UPDATE slot SET common_key = ? WHERE season_id = ? AND name = ?")
                .bind(key)
                .bind(season)
                .bind(name)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    /// After a re-roll: rebuild the season's Bifrost slots (1:1 AP slots +
    /// co-op seats) on the new seed's AP slot numbers by name, carrying each
    /// claim over. Player names are stable across a re-roll, so claims and
    /// co-op grouping survive.
    pub async fn remap_season(
        &self,
        season: Id,
        new_ap_slots: &[(i64, String, String)],
    ) -> Result<(), sqlx::Error> {
        use std::collections::{HashMap, HashSet};
        let old = self.slots(season).await?;
        self.clear_all_slots(season).await?;
        let new_season = season;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let roster: HashSet<&str> = new_ap_slots.iter().map(|(_, n, _)| n.as_str()).collect();
        // Common games' slots stay theirs, by name.
        let common_keys: HashMap<&str, &str> = old
            .iter()
            .filter_map(|s| Some((s.name.as_str(), s.common_key.as_deref()?)))
            .collect();
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
            // The same game keeps its id on the new seed.
            let game_id = old
                .iter()
                .find(|s| &s.name == name)
                .map_or_else(Id::new, |s| s.game_id);
            sqlx::query(
                "INSERT OR IGNORE INTO slot (id, season_id, game_id, name, ap_slot, game, common_key)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(Id::new())
            .bind(new_season)
            .bind(game_id)
            .bind(name)
            .bind(num)
            .bind(game)
            .bind(common_keys.get(name.as_str()).copied())
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
                "INSERT OR IGNORE INTO slot (id, season_id, game_id, name, ap_slot, game, is_seat)
                 VALUES (?, ?, ?, ?, ?, ?, 1)",
            )
            .bind(s.id)
            .bind(new_season)
            .bind(s.game_id)
            .bind(&s.name)
            .bind(new_ap)
            .bind(&s.game)
            .execute(&self.pool)
            .await?;
        }
        // Carry claims over by name.
        for s in &old {
            let Some(account) = s.claimed_by_id else {
                continue;
            };
            let id: Option<Id> =
                sqlx::query_scalar("SELECT id FROM slot WHERE season_id = ? AND name = ?")
                    .bind(new_season)
                    .bind(&s.name)
                    .fetch_optional(&self.pool)
                    .await?;
            if let Some(id) = id {
                sqlx::query(
                    "INSERT OR IGNORE INTO claim (slot_id, account_id, claimed_at) VALUES (?, ?, ?)",
                )
                .bind(id)
                .bind(account)
                .bind(now)
                .execute(&self.pool)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn slots(&self, season: Id) -> Result<Vec<SlotRow>, sqlx::Error> {
        sqlx::query_as::<_, SlotRow>(
            r#"SELECT s.id, s.game_id, s.is_seat, s.name, s.ap_slot, s.game, a.username AS claimed_by,
                      c.account_id AS claimed_by_id, s.common_key
               FROM slot s LEFT JOIN claim c ON c.slot_id = s.id
               LEFT JOIN account a ON a.id = c.account_id
               WHERE s.season_id = ?
               ORDER BY s.ap_slot, s.name"#,
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await
    }

    /// Claim `slot_id` — only if it's one of `season`'s slots.
    pub async fn claim_one(
        &self,
        season: Id,
        slot_id: Id,
        account: i64,
    ) -> Result<ClaimOutcome, sqlx::Error> {
        let found: Option<Id> =
            sqlx::query_scalar("SELECT season_id FROM slot WHERE id = ? AND season_id = ?")
                .bind(slot_id)
                .bind(season)
                .fetch_optional(&self.pool)
                .await?;
        if found.is_none() {
            return Ok(ClaimOutcome::SlotTaken);
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let res = sqlx::query(
            "INSERT OR IGNORE INTO claim (slot_id, account_id, claimed_at) VALUES (?, ?, ?)",
        )
        .bind(slot_id)
        .bind(account)
        .bind(now)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() > 0 {
            return Ok(ClaimOutcome::Ok);
        }
        // Slot was already claimed — fine if it's this same user, else taken.
        let owner: Option<i64> =
            sqlx::query_scalar("SELECT account_id FROM claim WHERE slot_id = ?")
                .bind(slot_id)
                .fetch_optional(&self.pool)
                .await?;
        if owner == Some(account) {
            Ok(ClaimOutcome::Ok)
        } else {
            Ok(ClaimOutcome::SlotTaken)
        }
    }

    /// Record a signup submission, unless `limit` (0 = none) says the account has
    /// enough already: `None` then, decided within the insert so that parallel
    /// signups can't pass it. Errors (e.g. duplicate slot name) bubble up.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_submission(
        &self,
        season: Id,
        account: i64,
        slot_name: &str,
        game: &str,
        is_custom: bool,
        yaml_file: &str,
        held_for: Option<&str>,
        limit: i64,
    ) -> Result<Option<Id>, sqlx::Error> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let id = Id::new();
        let res = sqlx::query(
            "INSERT INTO submission (id, season_id, account_id, slot_name, game, is_custom, yaml_file, held_for, created_at)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
             WHERE ?10 = 0 OR (SELECT COUNT(*) FROM submission WHERE season_id = ?2 AND account_id = ?3) < ?10",
        )
        .bind(id)
        .bind(season)
        .bind(account)
        .bind(slot_name)
        .bind(game)
        .bind(is_custom as i64)
        .bind(yaml_file)
        .bind(held_for)
        .bind(now)
        .bind(limit)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Ok(None);
        }
        Ok(Some(id))
    }

    pub async fn submissions(&self, season: Id) -> Result<Vec<SubmissionRow>, sqlx::Error> {
        sqlx::query_as::<_, SubmissionRow>(
            "SELECT s.id, s.account_id, a.username, s.slot_name, s.game, s.is_custom,
                    s.yaml_file, s.held_for, s.held_err, s.notes,
                    COALESCE((SELECT group_concat(name, ', ') FROM (
                        SELECT name FROM seat WHERE submission_id = s.id
                        UNION
                        SELECT b.name FROM slot b
                        JOIN slot h ON h.season_id = b.season_id AND h.ap_slot = b.ap_slot
                        WHERE h.season_id = s.season_id AND h.name = s.slot_name AND b.name <> s.slot_name
                          AND (SELECT state FROM season WHERE id = s.season_id) NOT IN ('draft', 'signup')
                    )), '') AS seat_names
             FROM submission s JOIN account a ON a.id = s.account_id
             WHERE s.season_id = ? ORDER BY s.rowid",
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await
    }

    /// One game of a season's signups.
    pub async fn submission(&self, season: Id, id: Id) -> Option<SubmissionRow> {
        sqlx::query_as::<_, SubmissionRow>(
            "SELECT s.id, s.account_id, a.username, s.slot_name, s.game, s.is_custom,
                    s.yaml_file, s.held_for, s.held_err, s.notes,
                    COALESCE((SELECT group_concat(name, ', ') FROM (
                        SELECT name FROM seat WHERE submission_id = s.id
                        UNION
                        SELECT b.name FROM slot b
                        JOIN slot h ON h.season_id = b.season_id AND h.ap_slot = b.ap_slot
                        WHERE h.season_id = s.season_id AND h.name = s.slot_name AND b.name <> s.slot_name
                          AND (SELECT state FROM season WHERE id = s.season_id) NOT IN ('draft', 'signup')
                    )), '') AS seat_names
             FROM submission s JOIN account a ON a.id = s.account_id
             WHERE s.id = ? AND s.season_id = ?",
        )
        .bind(id)
        .bind(season)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    /// Set a game's notes: its host and partners until the season generates, the
    /// season's moderators at any time.
    pub async fn set_submission_notes(
        &self,
        season: Id,
        id: Id,
        account: i64,
        is_mod: bool,
        notes: &str,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE submission SET notes = ?1
             WHERE id = ?2 AND season_id = ?3
               AND (?5
                    OR ((account_id = ?4
                         OR EXISTS (SELECT 1 FROM seat WHERE submission_id = ?2 AND account_id = ?4))
                        AND (SELECT state FROM season WHERE id = ?3) IN ('draft', 'signup')))",
        )
        .bind(notes)
        .bind(id)
        .bind(season)
        .bind(account)
        .bind(is_mod)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Point a game at a new YAML: its host's while signups are open, or a
    /// moderator's any time before generation. It keeps everything else, and is
    /// no longer held if the new file passed its check; `still_held` if its world is
    /// still waiting, so it couldn't be checked yet. Refused (false) otherwise,
    /// or if its file is no longer `old_yaml` (a replace or removal got there first).
    #[allow(clippy::too_many_arguments)]
    pub async fn replace_submission(
        &self,
        season: Id,
        id: Id,
        account: i64,
        is_mod: bool,
        still_held: bool,
        slot_name: &str,
        game: &str,
        old_yaml: &str,
        yaml_file: &str,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE submission
             SET slot_name = ?1, is_custom = ?9 OR (is_custom AND game = ?2), game = ?2, yaml_file = ?3,
                 held_for = CASE WHEN ?9 THEN held_for END, held_err = NULL
             WHERE id = ?4 AND season_id = ?5 AND yaml_file = ?7
               AND ((account_id = ?6 AND (SELECT state FROM season WHERE id = ?5) = 'signup')
                    OR (?8 AND (SELECT state FROM season WHERE id = ?5) IN ('draft', 'signup')))",
        )
        .bind(slot_name)
        .bind(game)
        .bind(yaml_file)
        .bind(id)
        .bind(season)
        .bind(account)
        .bind(old_yaml)
        .bind(is_mod)
        .bind(still_held)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// How many games this player has signed up with in a season.
    pub async fn games_of(&self, season: Id, account: i64) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM submission WHERE season_id = ? AND account_id = ?")
            .bind(season)
            .bind(account)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0)
    }

    /// Delete a submission (its own submitter, or an admin). Returns the deleted
    /// row's YAML path so the caller can clean it up.
    pub async fn delete_submission(
        &self,
        season: Id,
        id: Id,
        account: i64,
        is_admin: bool,
    ) -> Result<Option<String>, sqlx::Error> {
        let row = sqlx::query_as::<_, (i64, String)>(
            "SELECT account_id, yaml_file FROM submission WHERE id = ? AND season_id = ?",
        )
        .bind(id)
        .bind(season)
        .fetch_optional(&self.pool)
        .await?;
        let Some((owner, yaml_file)) = row else {
            return Ok(None);
        };
        if !is_admin && owner != account {
            return Ok(None);
        }
        // Only the file read above: a replace that got in first keeps its signup.
        // Once generated the game is in the seed; a moderator releases it instead.
        let res = sqlx::query(
            "DELETE FROM submission WHERE id = ?1 AND yaml_file = ?2
               AND (SELECT state FROM season WHERE id = ?3) IN ('draft', 'signup')",
        )
        .bind(id)
        .bind(&yaml_file)
        .bind(season)
        .execute(&self.pool)
        .await?;
        Ok((res.rows_affected() > 0).then_some(yaml_file))
    }

    /// The AP slot a live game is on in the current seed.
    pub async fn ap_slot_of_game(&self, season: Id, game_id: Id) -> Option<i64> {
        sqlx::query_scalar("SELECT ap_slot FROM slot WHERE season_id = ? AND game_id = ? LIMIT 1")
            .bind(season)
            .bind(game_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    /// Whether `account` claims any Bifrost slot mapping to `ap_slot` this season.
    pub async fn claims_ap_slot(&self, season: Id, account: i64, ap_slot: i64) -> bool {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM slot s JOIN claim c ON c.slot_id = s.id
             WHERE s.season_id = ? AND s.ap_slot = ? AND c.account_id = ?",
        )
        .bind(season)
        .bind(ap_slot)
        .bind(account)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        n > 0
    }

    /// (ap_slot, current claimant) for a Bifrost slot, or None if it doesn't exist.
    pub async fn slot_detail(
        &self,
        season: Id,
        id: Id,
    ) -> Result<Option<(i64, Option<i64>, bool)>, sqlx::Error> {
        sqlx::query_as::<_, (i64, Option<i64>, bool)>(
            "SELECT s.ap_slot, c.account_id, s.common_key IS NOT NULL FROM slot s
             LEFT JOIN claim c ON c.slot_id = s.id WHERE s.id = ? AND s.season_id = ?",
        )
        .bind(id)
        .bind(season)
        .fetch_optional(&self.pool)
        .await
    }
}
