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

#[derive(Debug, FromRow, Clone)]
pub struct SubmissionRow {
    pub id: i64,
    pub sso: String,
    pub slot_name: String,
    pub game: String,
    pub is_custom: bool,
    pub coop_names: String,
    pub yaml_file: String,
    pub apworld_file: Option<String>,
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
        Ok(Self { pool })
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

    /// Claim a free slot. Returns Ok(true) if newly claimed, Ok(false) if it was
    /// already claimed by someone.
    pub async fn claim(&self, slot_id: i64, sso: &str) -> Result<bool, sqlx::Error> {
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
        Ok(res.rows_affected() > 0)
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
            "SELECT id, sso, slot_name, game, is_custom, coop_names, yaml_file, apworld_file
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
