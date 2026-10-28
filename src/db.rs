//! Persistence for Bifrost slots + claims (SQLite).
//!
//! A **Bifrost slot** is one person (claimable, tied to an account), mapping to one
//! **AP slot** (the game). Co-op games (e.g. Don't Starve Together) have several
//! Bifrost slots pointing at the same `ap_slot`. A season is keyed by the AP
//! `seed_name`, so a new seed gets a fresh set of slots + claims.

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{FromRow, SqlitePool};

mod auth;
mod migrate;
pub use migrate::SCHEMA_VERSION;
mod seasons;
mod settings;
mod worlds;
pub use auth::{AccountRow, LinkKind, LinkRow, RedeemError};
pub use seasons::{SeasonRow, State};
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
    pub id: i64,
    pub name: String,
    pub ap_slot: i64,
    pub game: String,
    /// The claimant's current username (display) and account (authorization).
    pub claimed_by: Option<String>,
    pub claimed_by_id: Option<i64>,
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
        if t == 0 { 0 } else { (self.wins * 100) / t }
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
    pub account_id: i64,
    /// The submitter's current username.
    pub username: String,
    pub slot_name: String,
    pub game: String,
    pub is_custom: bool,
    pub coop_names: String,
    pub yaml_file: String,
    /// Held until this apworld (a pending upload's file name) is approved;
    /// its YAML can't be checked before then.
    pub held_for: Option<String>,
    /// Why a held submission can't go ahead (its check failed after approval,
    /// or the apworld was rejected).
    pub held_err: Option<String>,
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
    pub async fn connect(path: &str) -> Result<Self, sqlx::Error> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
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

/// The schema as of schema version 1, the first release's. Frozen: change the
/// schema with a step in [`migrate::STEPS`], never by editing these tables.
async fn baseline(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    // Accounts and seasons first: the tables below reference them.
    auth::migrate(pool).await?;
    seasons::migrate(pool).await?;
    settings::migrate(pool).await?;
    worlds::migrate(pool).await?;
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS bifrost_slot (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 season INTEGER NOT NULL REFERENCES season(id) ON DELETE CASCADE,
                 name TEXT NOT NULL,
                 ap_slot INTEGER NOT NULL,
                 game TEXT NOT NULL,
                 -- The shared "common game" slot (one AP slot everyone plays).
                 is_common INTEGER NOT NULL DEFAULT 0,
                 UNIQUE(season, name)
               )"#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS claim (
                 bifrost_slot_id INTEGER PRIMARY KEY REFERENCES bifrost_slot(id) ON DELETE CASCADE,
                 account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
                 claimed_at INTEGER NOT NULL
               )"#,
    )
    .execute(pool)
    .await?;
    // Signup submissions for a season (before its seed exists).
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS submission (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 season INTEGER NOT NULL REFERENCES season(id) ON DELETE CASCADE,
                 account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
                 slot_name TEXT NOT NULL,
                 game TEXT NOT NULL,
                 is_custom INTEGER NOT NULL DEFAULT 0,
                 coop_names TEXT NOT NULL DEFAULT '',
                 yaml_file TEXT NOT NULL,
                 held_for TEXT,
                 held_err TEXT,
                 created_at INTEGER NOT NULL,
                 UNIQUE(season, slot_name)
               )"#,
    )
    .execute(pool)
    .await?;
    // Bonus hint credits earned via the minesweeper minigame (per season).
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS hint_credit (
                 season INTEGER NOT NULL REFERENCES season(id) ON DELETE CASCADE,
                 account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
                 credits INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (season, account_id)
               )"#,
    )
    .execute(pool)
    .await?;
    // Hints a player has flagged to surface on the board.
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS hint_priority (
                 season INTEGER NOT NULL REFERENCES season(id) ON DELETE CASCADE,
                 finding INTEGER NOT NULL,
                 location INTEGER NOT NULL,
                 item INTEGER NOT NULL,
                 PRIMARY KEY (season, finding, location, item)
               )"#,
    )
    .execute(pool)
    .await?;
    // One row per finished minesweeper game, for per-slot/per-season stats.
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS mine_result (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 -- NULL = freeplay (outside any season).
                 season INTEGER REFERENCES season(id) ON DELETE CASCADE,
                 account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
                 won INTEGER NOT NULL,
                 duration_ms INTEGER NOT NULL,
                 ts INTEGER NOT NULL,
                 kind TEXT NOT NULL DEFAULT 'standard'
               )"#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_mine_result ON mine_result (season, account_id, kind)",
    )
    .execute(pool)
    .await?;
    // A player's Minecraft username, for the common-game server's whitelist.
    // Keyed by account (persists across seasons — a returning player
    // keeps their name); the case-insensitive unique index stops two people
    // claiming the same Minecraft account. The *whitelist* itself is scoped
    // to the current season's claimants (see `mc_whitelist`).
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS mc_account (
                 account_id INTEGER PRIMARY KEY REFERENCES account(id) ON DELETE CASCADE,
                 mc_username TEXT NOT NULL,
                 updated_at INTEGER NOT NULL
               )"#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_mc_account_name ON mc_account (mc_username COLLATE NOCASE)",
        )
        .execute(pool)
        .await?;
    // Persistent activity log, one row per kept AP journal event. Scoped by
    // season so an archived season can replay it; NOT cleared by the admin
    // reset. Ordered by `id` = journal order. De-duplication across runs is
    // handled by the season's journal cursor, not a UNIQUE constraint, so
    // genuine identical events in the same second are never dropped.
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS activity (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 season INTEGER NOT NULL REFERENCES season(id) ON DELETE CASCADE,
                 ts INTEGER NOT NULL,
                 kind TEXT NOT NULL,
                 text TEXT NOT NULL,
                 sender TEXT,
                 receiver TEXT,
                 item_class TEXT
               )"#,
    )
    .execute(pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_activity ON activity (season, id)")
        .execute(pool)
        .await?;
    // Universal-Tracker in-logic cache: one row per (season, ap_slot), holding
    // that slot's currently-reachable location names (computed out-of-band by
    // the `inlogic` recomputer shelling out to UT). `sent`/`received` are the
    // slot state the row was computed at, so the recomputer re-runs only when
    // the slot's state moves.
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS inlogic (
                 season INTEGER NOT NULL REFERENCES season(id) ON DELETE CASCADE,
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
    .execute(pool)
    .await?;
    Ok(())
}

impl Db {
    /// A player's bonus-hint credits for a season.
    pub async fn credits(&self, season: i64, account: i64) -> i64 {
        sqlx::query_scalar("SELECT credits FROM hint_credit WHERE season = ? AND account_id = ?")
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
    pub async fn has_claimed_slot(&self, season: i64, account: i64) -> bool {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM claim c JOIN bifrost_slot s ON s.id = c.bifrost_slot_id
             WHERE s.season = ? AND c.account_id = ? AND s.is_common = 0",
        )
        .bind(season)
        .bind(account)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        n > 0
    }

    /// This player's registered Minecraft username, if any.
    pub async fn mc_username(&self, account: i64) -> Option<String> {
        sqlx::query_scalar("SELECT mc_username FROM mc_account WHERE account_id = ?")
            .bind(account)
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
        account: i64,
        name: &str,
    ) -> Result<SetMcOutcome, sqlx::Error> {
        // Taken by someone else? (Same user re-saving their own name is fine.)
        let owner: Option<i64> = sqlx::query_scalar(
            "SELECT account_id FROM mc_account WHERE mc_username = ? COLLATE NOCASE",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        if matches!(owner, Some(o) if o != account) {
            return Ok(SetMcOutcome::TakenByOther);
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        sqlx::query(
            "INSERT INTO mc_account (account_id, mc_username, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(account_id) DO UPDATE SET mc_username = excluded.mc_username, updated_at = excluded.updated_at",
        )
        .bind(account)
        .bind(name)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(SetMcOutcome::Ok)
    }

    /// The Minecraft whitelist for a season: the registered usernames of everyone
    /// holding a regular (non-common) slot this season. Exported for the
    /// host-side `mc-whitelist-sync` to apply to the Minecraft server.
    /// Everyone to whitelist on the shared Minecraft server: claimants of a
    /// regular slot in any live season that has the common game on.
    pub async fn mc_whitelist_all(&self) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT a.mc_username FROM mc_account a
             WHERE EXISTS (
                 SELECT 1 FROM claim c JOIN bifrost_slot s ON s.id = c.bifrost_slot_id
                 JOIN season se ON se.id = s.season
                 WHERE se.state = 'live' AND se.mc_enabled = 1
                   AND c.account_id = a.account_id AND s.is_common = 0
             )
             ORDER BY a.mc_username COLLATE NOCASE",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    #[cfg(test)]
    pub async fn mc_whitelist(&self, season: i64) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT a.mc_username FROM mc_account a
             WHERE EXISTS (
                 SELECT 1 FROM claim c JOIN bifrost_slot s ON s.id = c.bifrost_slot_id
                 WHERE s.season = ? AND c.account_id = a.account_id AND s.is_common = 0
             )
             ORDER BY a.mc_username COLLATE NOCASE",
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// Grant `n` bonus-hint credits, returning the new total.
    pub async fn add_credits(&self, season: i64, account: i64, n: i64) -> Result<i64, sqlx::Error> {
        sqlx::query(
            "INSERT INTO hint_credit (season, account_id, credits) VALUES (?, ?, ?)
             ON CONFLICT(season, account_id) DO UPDATE SET credits = credits + excluded.credits",
        )
        .bind(season)
        .bind(account)
        .bind(n)
        .execute(&self.pool)
        .await?;
        Ok(self.credits(season, account).await)
    }

    /// Spend one bonus-hint credit if available; returns true if one was spent.
    pub async fn spend_credit(&self, season: i64, account: i64) -> bool {
        let res = sqlx::query(
            "UPDATE hint_credit SET credits = credits - 1
             WHERE season = ? AND account_id = ? AND credits > 0",
        )
        .bind(season)
        .bind(account)
        .execute(&self.pool)
        .await;
        matches!(res, Ok(r) if r.rows_affected() > 0)
    }

    /// Record a finished minesweeper game (win or loss, with its duration) for a
    /// given board kind ("standard" / "expert").
    pub async fn record_mine_result(
        &self,
        season: Option<i64>,
        account: i64,
        kind: &str,
        won: bool,
        duration_ms: i64,
    ) {
        let _ = sqlx::query(
            "INSERT INTO mine_result (season, account_id, kind, won, duration_ms, ts)
             VALUES (?, ?, ?, ?, ?, strftime('%s','now'))",
        )
        .bind(season)
        .bind(account)
        .bind(kind)
        .bind(won as i64)
        .bind(duration_ms)
        .execute(&self.pool)
        .await;
    }

    /// Per-kind minesweeper stats for a player. `season = Some(s)` scopes to one
    /// season; `None` is all-time (across every season + freeplay) — the
    /// "permanent" view. Time stats are over wins only; `None` = no wins yet.
    pub async fn mine_stats(&self, season: Option<i64>, account: i64, kind: &str) -> MineStats {
        let mut sql = String::from(
            "SELECT COALESCE(SUM(won),0) AS wins, COALESCE(SUM(1-won),0) AS losses, \
             CAST(AVG(CASE WHEN won=1 THEN duration_ms END) AS INTEGER) AS avg_ms, \
             MIN(CASE WHEN won=1 THEN duration_ms END) AS best_ms, \
             MAX(CASE WHEN won=1 THEN duration_ms END) AS worst_ms \
             FROM mine_result WHERE account_id=? AND kind=?",
        );
        if season.is_some() {
            sql.push_str(" AND season=?");
        }
        let mut q = sqlx::query_as::<_, MineStats>(&sql)
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
        season: Option<i64>,
        kind: &str,
        limit: i64,
    ) -> Vec<(String, i64)> {
        let mut sql = String::from(
            "SELECT a.username, MIN(m.duration_ms) AS best FROM mine_result m \
             JOIN account a ON a.id = m.account_id WHERE m.kind=? AND m.won=1",
        );
        if season.is_some() {
            sql.push_str(" AND m.season=?");
        }
        sql.push_str(" GROUP BY m.account_id ORDER BY best ASC LIMIT ?");
        let mut q = sqlx::query_as::<_, (String, i64)>(&sql).bind(kind);
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
        season: Option<i64>,
        kind: &str,
        min_games: i64,
        limit: i64,
    ) -> Vec<(String, i64, i64)> {
        let mut sql = String::from(
            "SELECT a.username, SUM(m.won) AS wins, COUNT(*) AS total FROM mine_result m \
             JOIN account a ON a.id = m.account_id WHERE m.kind=?",
        );
        if season.is_some() {
            sql.push_str(" AND m.season=?");
        }
        sql.push_str(
            " GROUP BY m.account_id HAVING total>=? ORDER BY (CAST(SUM(m.won) AS REAL)/COUNT(*)) DESC, total DESC LIMIT ?",
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
    pub async fn clear_submissions(&self, season: i64) -> Result<Vec<String>, sqlx::Error> {
        let files: Vec<String> =
            sqlx::query_scalar("SELECT yaml_file FROM submission WHERE season = ?")
                .bind(season)
                .fetch_all(&self.pool)
                .await?;
        sqlx::query("DELETE FROM submission WHERE season = ?")
            .bind(season)
            .execute(&self.pool)
            .await?;
        Ok(files)
    }

    /// Delete a season's Bifrost slots (claims cascade). Used by the admin reset.
    pub async fn clear_all_slots(&self, season: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM bifrost_slot WHERE season = ?")
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
        season: i64,
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
    pub async fn activity_any(&self, season: i64) -> bool {
        sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM activity WHERE season = ?)")
            .bind(season)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0)
            == 1
    }

    /// Cached in-logic result for one slot, if computed this season.
    pub async fn inlogic_get(&self, season: i64, ap_slot: i64) -> Option<InLogic> {
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
    pub async fn inlogic_counts(&self, season: i64) -> std::collections::HashMap<i64, i64> {
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
    pub async fn inlogic_state(&self, season: i64, ap_slot: i64) -> Option<(i64, i64)> {
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
        season: i64,
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
    pub async fn activity_latest(&self, season: i64, limit: i64) -> Vec<ActivityRow> {
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
    pub async fn activity_count(&self, season: i64, f: &ActFilter<'_>) -> i64 {
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
        season: i64,
        f: &ActFilter<'_>,
        offset: i64,
        limit: i64,
    ) -> Vec<ActivityRow> {
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT ts, kind, text, item_class FROM activity WHERE season = ",
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
        season: i64,
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
    pub async fn activity_count_for_participant(&self, season: i64, name: &str, q: &str) -> i64 {
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
    pub async fn activity_participants(&self, season: i64) -> Vec<String> {
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
        season: i64,
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
        season: i64,
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
    pub async fn unprioritize_many(&self, season: i64, hints: &[(i64, i64, i64)]) {
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
        season: i64,
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
        season: i64,
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
                    "INSERT OR IGNORE INTO claim (bifrost_slot_id, account_id, claimed_at) VALUES (?, ?, ?)",
                )
                .bind(id)
                .bind(sub.account_id)
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

    /// After a re-roll: rebuild the season's Bifrost slots (1:1 AP slots +
    /// co-op seats) on the new seed's AP slot numbers by name, carrying each
    /// claim over. Player names are stable across a re-roll, so claims and
    /// co-op grouping survive.
    pub async fn remap_season(
        &self,
        season: i64,
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
        // Names that were communal common-game slots last season stay communal.
        let common_names: HashSet<&str> = old
            .iter()
            .filter(|s| s.is_common)
            .map(|s| s.name.as_str())
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
            let Some(account) = s.claimed_by_id else {
                continue;
            };
            let id: Option<i64> =
                sqlx::query_scalar("SELECT id FROM bifrost_slot WHERE season = ? AND name = ?")
                    .bind(new_season)
                    .bind(&s.name)
                    .fetch_optional(&self.pool)
                    .await?;
            if let Some(id) = id {
                sqlx::query(
                    "INSERT OR IGNORE INTO claim (bifrost_slot_id, account_id, claimed_at) VALUES (?, ?, ?)",
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

    pub async fn slots(&self, season: i64) -> Result<Vec<SlotRow>, sqlx::Error> {
        sqlx::query_as::<_, SlotRow>(
            r#"SELECT s.id, s.name, s.ap_slot, s.game, a.username AS claimed_by,
                      c.account_id AS claimed_by_id, s.is_common
               FROM bifrost_slot s LEFT JOIN claim c ON c.bifrost_slot_id = s.id
               LEFT JOIN account a ON a.id = c.account_id
               WHERE s.season = ?
               ORDER BY s.ap_slot, s.name"#,
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await
    }

    /// Claim a slot, enforcing one claim per player per season.
    /// Claim `slot_id` — only if it's one of `season`'s slots.
    pub async fn claim_one(
        &self,
        season: i64,
        slot_id: i64,
        account: i64,
    ) -> Result<ClaimOutcome, sqlx::Error> {
        let found: Option<i64> =
            sqlx::query_scalar("SELECT season FROM bifrost_slot WHERE id = ? AND season = ?")
                .bind(slot_id)
                .bind(season)
                .fetch_optional(&self.pool)
                .await?;
        if found.is_none() {
            return Ok(ClaimOutcome::SlotTaken);
        }
        // Already holding a *different* slot this season? One per player.
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT s.name FROM claim c JOIN bifrost_slot s ON s.id = c.bifrost_slot_id
             WHERE s.season = ? AND c.account_id = ? AND s.id <> ?",
        )
        .bind(season)
        .bind(account)
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
            "INSERT OR IGNORE INTO claim (bifrost_slot_id, account_id, claimed_at) VALUES (?, ?, ?)",
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
            sqlx::query_scalar("SELECT account_id FROM claim WHERE bifrost_slot_id = ?")
                .bind(slot_id)
                .fetch_optional(&self.pool)
                .await?;
        if owner == Some(account) {
            Ok(ClaimOutcome::Ok)
        } else {
            Ok(ClaimOutcome::SlotTaken)
        }
    }

    /// Record a signup submission. Errors (e.g. duplicate slot name) bubble up.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_submission(
        &self,
        season: i64,
        account: i64,
        slot_name: &str,
        game: &str,
        is_custom: bool,
        coop_names: &str,
        yaml_file: &str,
        held_for: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let res = sqlx::query(
            "INSERT INTO submission (season, account_id, slot_name, game, is_custom, coop_names, yaml_file, held_for, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(season)
        .bind(account)
        .bind(slot_name)
        .bind(game)
        .bind(is_custom as i64)
        .bind(coop_names)
        .bind(yaml_file)
        .bind(held_for)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    pub async fn submissions(&self, season: i64) -> Result<Vec<SubmissionRow>, sqlx::Error> {
        sqlx::query_as::<_, SubmissionRow>(
            "SELECT s.id, s.account_id, a.username, s.slot_name, s.game, s.is_custom,
                    s.coop_names, s.yaml_file, s.held_for, s.held_err
             FROM submission s JOIN account a ON a.id = s.account_id
             WHERE s.season = ? ORDER BY s.created_at",
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await
    }

    /// Delete a submission (its own submitter, or an admin). Returns the deleted
    /// row's YAML path so the caller can clean it up.
    pub async fn delete_submission(
        &self,
        season: i64,
        id: i64,
        account: i64,
        is_admin: bool,
    ) -> Result<Option<String>, sqlx::Error> {
        let row = sqlx::query_as::<_, (i64, String)>(
            "SELECT account_id, yaml_file FROM submission WHERE id = ? AND season = ?",
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
        sqlx::query("DELETE FROM submission WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(Some(yaml_file))
    }

    /// Whether `account` claims any Bifrost slot mapping to `ap_slot` this season.
    pub async fn claims_ap_slot(&self, season: i64, account: i64, ap_slot: i64) -> bool {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM bifrost_slot s JOIN claim c ON c.bifrost_slot_id = s.id
             WHERE s.season = ? AND s.ap_slot = ? AND c.account_id = ?",
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
        season: i64,
        id: i64,
    ) -> Result<Option<(i64, Option<i64>, bool)>, sqlx::Error> {
        sqlx::query_as::<_, (i64, Option<i64>, bool)>(
            "SELECT s.ap_slot, c.account_id, s.is_common FROM bifrost_slot s
             LEFT JOIN claim c ON c.bifrost_slot_id = s.id WHERE s.id = ? AND s.season = ?",
        )
        .bind(id)
        .bind(season)
        .fetch_optional(&self.pool)
        .await
    }

    /// Release a claim. A non-admin may only release their own; an admin any.
    /// Release `slot_id` (one of `season`'s slots): the claimant's own, or
    /// anyone's for that season's admins.
    pub async fn unclaim(
        &self,
        season: i64,
        slot_id: i64,
        account: i64,
        is_admin: bool,
    ) -> Result<bool, sqlx::Error> {
        const IN_SEASON: &str =
            "bifrost_slot_id IN (SELECT id FROM bifrost_slot WHERE id = ? AND season = ?)";
        let res = if is_admin {
            sqlx::query(&format!("DELETE FROM claim WHERE {IN_SEASON}"))
                .bind(slot_id)
                .bind(season)
                .execute(&self.pool)
                .await?
        } else {
            sqlx::query(&format!(
                "DELETE FROM claim WHERE {IN_SEASON} AND account_id = ?"
            ))
            .bind(slot_id)
            .bind(season)
            .bind(account)
            .execute(&self.pool)
            .await?
        };
        Ok(res.rows_affected() > 0)
    }
}
