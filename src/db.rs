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
        Ok(Self { pool })
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
