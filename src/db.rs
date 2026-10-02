//! Persistence for Bifrost slots + claims (SQLite).
//!
//! A **Bifrost slot** is one person (claimable, tied to an SSO), mapping to one
//! **AP slot** (the game). Co-op games (e.g. Don't Starve Together) have several
//! Bifrost slots pointing at the same `ap_slot`. A season is keyed by the AP
//! `seed_name`, so a new seed gets a fresh set of slots + claims.

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{FromRow, SqlitePool};

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

    /// Per-slot/per-season/per-kind minesweeper stats. Time stats are over wins
    /// only (a loss has no meaningful clear time); `None` means no wins yet.
    pub async fn mine_stats(&self, season: &str, sso: &str, kind: &str) -> MineStats {
        sqlx::query_as::<_, MineStats>(
            r#"SELECT
                 COALESCE(SUM(won), 0)                                    AS wins,
                 COALESCE(SUM(1 - won), 0)                                AS losses,
                 CAST(AVG(CASE WHEN won = 1 THEN duration_ms END) AS INTEGER) AS avg_ms,
                 MIN(CASE WHEN won = 1 THEN duration_ms END)              AS best_ms,
                 MAX(CASE WHEN won = 1 THEN duration_ms END)              AS worst_ms
               FROM mine_result WHERE season = ? AND sso = ? AND kind = ?"#,
        )
        .bind(season)
        .bind(sso)
        .bind(kind)
        .fetch_one(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// Best-time minesweeper leaderboard for a season/kind: (player, best_ms)
    /// over won games, fastest first, top `limit`.
    pub async fn mine_leaders_time(
        &self,
        season: &str,
        kind: &str,
        limit: i64,
    ) -> Vec<(String, i64)> {
        sqlx::query_as::<_, (String, i64)>(
            "SELECT sso, MIN(duration_ms) AS best FROM mine_result
             WHERE season = ? AND kind = ? AND won = 1
             GROUP BY sso ORDER BY best ASC LIMIT ?",
        )
        .bind(season)
        .bind(kind)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// Best completion-rate leaderboard for a season/kind: (player, wins, total)
    /// for players with at least `min_games`, highest rate first (more games
    /// breaks ties), top `limit`.
    pub async fn mine_leaders_rate(
        &self,
        season: &str,
        kind: &str,
        min_games: i64,
        limit: i64,
    ) -> Vec<(String, i64, i64)> {
        sqlx::query_as::<_, (String, i64, i64)>(
            "SELECT sso, SUM(won) AS wins, COUNT(*) AS total FROM mine_result
             WHERE season = ? AND kind = ?
             GROUP BY sso HAVING total >= ?
             ORDER BY (CAST(SUM(won) AS REAL) / COUNT(*)) DESC, total DESC LIMIT ?",
        )
        .bind(season)
        .bind(kind)
        .bind(min_games)
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
                "INSERT OR IGNORE INTO bifrost_slot (season, name, ap_slot, game) VALUES (?, ?, ?, ?)",
            )
            .bind(new_season)
            .bind(name)
            .bind(num)
            .bind(game)
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
            r#"SELECT s.id, s.name, s.ap_slot, s.game, c.sso AS claimed_by
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
    pub async fn slot_detail(&self, id: i64) -> Result<Option<(i64, Option<String>)>, sqlx::Error> {
        sqlx::query_as::<_, (i64, Option<String>)>(
            "SELECT s.ap_slot, c.sso FROM bifrost_slot s
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
