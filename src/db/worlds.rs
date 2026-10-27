//! The apworld catalog — instance-wide, curated by sysadmins, usable in any
//! season without approval — and per-season uploads waiting for approval.
//! The files themselves live on disk (`catalog/<sha256>.apworld`,
//! `seasons/<id>/pending/`); these tables hold what they are and who sent them.

use sqlx::{FromRow, SqlitePool};

use super::Db;

pub(super) async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS catalog_world (
             sha256 TEXT PRIMARY KEY,
             -- The name it installs as in a season's worlds/ (e.g. pokemon_crystal.apworld).
             file TEXT NOT NULL,
             game TEXT NOT NULL DEFAULT '',
             version TEXT NOT NULL DEFAULT '',
             note TEXT NOT NULL DEFAULT '',
             added_by TEXT NOT NULL,
             added_at INTEGER NOT NULL
           )"#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS world_request (
             season INTEGER NOT NULL REFERENCES season(id) ON DELETE CASCADE,
             file TEXT NOT NULL,
             sha256 TEXT NOT NULL,
             game TEXT NOT NULL DEFAULT '',
             uploaded_by TEXT NOT NULL,
             uploaded_at INTEGER NOT NULL,
             PRIMARY KEY (season, file)
           )"#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, FromRow, Clone, PartialEq)]
pub struct CatalogWorld {
    pub sha256: String,
    pub file: String,
    pub game: String,
    pub version: String,
    pub note: String,
    pub added_by: String,
    pub added_at: i64,
}

/// An uploaded apworld waiting for approval in a season.
#[derive(Debug, FromRow, Clone, PartialEq)]
pub struct WorldRequest {
    pub file: String,
    pub sha256: String,
    pub game: String,
    pub uploaded_by: String,
    pub uploaded_at: i64,
}

impl Db {
    pub async fn catalog(&self) -> Vec<CatalogWorld> {
        sqlx::query_as::<_, CatalogWorld>(
            "SELECT sha256, file, game, version, note, added_by, added_at
             FROM catalog_world ORDER BY lower(game), lower(file), added_at",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    pub async fn catalog_world(&self, sha256: &str) -> Option<CatalogWorld> {
        sqlx::query_as::<_, CatalogWorld>(
            "SELECT sha256, file, game, version, note, added_by, added_at
             FROM catalog_world WHERE sha256 = ?",
        )
        .bind(sha256)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    /// Add (or re-describe) a catalog entry.
    pub async fn catalog_add(&self, w: &CatalogWorld) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO catalog_world (sha256, file, game, version, note, added_by, added_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(sha256) DO UPDATE SET
               file = excluded.file, game = excluded.game,
               version = excluded.version, note = excluded.note",
        )
        .bind(&w.sha256)
        .bind(&w.file)
        .bind(&w.game)
        .bind(&w.version)
        .bind(&w.note)
        .bind(&w.added_by)
        .bind(w.added_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn catalog_remove(&self, sha256: &str) -> Result<bool, sqlx::Error> {
        let r = sqlx::query("DELETE FROM catalog_world WHERE sha256 = ?")
            .bind(sha256)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    pub async fn world_requests(&self, season: i64) -> Vec<WorldRequest> {
        sqlx::query_as::<_, WorldRequest>(
            "SELECT file, sha256, game, uploaded_by, uploaded_at
             FROM world_request WHERE season = ? ORDER BY uploaded_at",
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    pub async fn world_request(&self, season: i64, file: &str) -> Option<WorldRequest> {
        sqlx::query_as::<_, WorldRequest>(
            "SELECT file, sha256, game, uploaded_by, uploaded_at
             FROM world_request WHERE season = ? AND file = ?",
        )
        .bind(season)
        .bind(file)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    pub async fn add_world_request(
        &self,
        season: i64,
        r: &WorldRequest,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO world_request (season, file, sha256, game, uploaded_by, uploaded_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(season)
        .bind(&r.file)
        .bind(&r.sha256)
        .bind(&r.game)
        .bind(&r.uploaded_by)
        .bind(r.uploaded_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Remove a request (approved or rejected), returning it.
    pub async fn take_world_request(&self, season: i64, file: &str) -> Option<WorldRequest> {
        let r = self.world_request(season, file).await?;
        sqlx::query("DELETE FROM world_request WHERE season = ? AND file = ?")
            .bind(season)
            .bind(file)
            .execute(&self.pool)
            .await
            .ok()?;
        Some(r)
    }

    pub async fn clear_world_requests(&self, season: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM world_request WHERE season = ?")
            .bind(season)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Submissions held for `file`: (id, YAML path).
    pub async fn held_submissions(&self, season: i64, file: &str) -> Vec<(i64, String)> {
        sqlx::query_as::<_, (i64, String)>(
            "SELECT id, yaml_file FROM submission WHERE season = ? AND held_for = ?",
        )
        .bind(season)
        .bind(file)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// Worlds signups are still held for although nothing waits for approval
    /// under that name, and that no verdict was reached on (never checked, or
    /// the check couldn't run): a re-check was cut short.
    pub async fn stale_holds(&self, season: i64) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT DISTINCT held_for FROM submission s
             WHERE season = ? AND held_for IS NOT NULL
               AND (held_err IS NULL OR held_err LIKE 'couldn''t be checked%')
               AND NOT EXISTS (SELECT 1 FROM world_request w WHERE w.season = s.season AND w.file = s.held_for)",
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// A held submission passed its check: take the slot name and game the
    /// check read, and stop holding it. Fails if that slot name is taken.
    pub async fn release_submission(
        &self,
        id: i64,
        slot_name: &str,
        game: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE submission SET slot_name = ?, game = ?, held_for = NULL, held_err = NULL
             WHERE id = ?",
        )
        .bind(slot_name)
        .bind(game)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_held_err(&self, id: i64, err: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE submission SET held_err = ? WHERE id = ?")
            .bind(err)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db(tag: &str) -> (Db, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("bifrost-worlds-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(dir.join("t.db").to_str().unwrap()).await.unwrap();
        (db, dir)
    }

    #[tokio::test]
    async fn catalog_round_trip() {
        let (db, dir) = db("catalog").await;
        let w = CatalogWorld {
            sha256: "ab".repeat(32),
            file: "foo.apworld".into(),
            game: "Foo".into(),
            version: "1.2.0".into(),
            note: String::new(),
            added_by: "root".into(),
            added_at: 1,
        };
        db.catalog_add(&w).await.unwrap();
        assert_eq!(db.catalog().await, vec![w.clone()]);
        assert_eq!(db.catalog_world(&w.sha256).await, Some(w.clone()));
        assert!(db.catalog_remove(&w.sha256).await.unwrap());
        assert!(db.catalog().await.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn held_submissions_wait_for_their_world() {
        let (db, dir) = db("held").await;
        let season = db.create_season("S", None, 0).await.unwrap();
        let acct = db
            .create_account("alice", "x", crate::auth::Role::User, 0)
            .await
            .unwrap();
        let r = WorldRequest {
            file: "foo.apworld".into(),
            sha256: "cd".repeat(32),
            game: "Foo".into(),
            uploaded_by: "alice".into(),
            uploaded_at: 1,
        };
        db.add_world_request(season, &r).await.unwrap();
        assert!(
            db.add_world_request(season, &r).await.is_err(),
            "one request per file"
        );
        let id = db
            .add_submission(
                season,
                acct,
                "Alice",
                "Foo",
                true,
                "",
                "/x.yaml",
                Some("foo.apworld"),
            )
            .await
            .unwrap();
        assert_eq!(
            db.held_submissions(season, "foo.apworld").await,
            vec![(id, "/x.yaml".into())]
        );
        db.set_held_err(id, "nope").await.unwrap();
        let sub = &db.submissions(season).await.unwrap()[0];
        assert!(sub.held() && sub.held_err.as_deref() == Some("nope"));
        db.release_submission(id, "Alice2", "Foo").await.unwrap();
        let sub = &db.submissions(season).await.unwrap()[0];
        assert!(!sub.held() && sub.held_err.is_none() && sub.slot_name == "Alice2");
        assert_eq!(db.take_world_request(season, "foo.apworld").await, Some(r));
        assert!(db.world_requests(season).await.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
