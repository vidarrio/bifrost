//! The apworld catalog — instance-wide, curated by sysadmins, usable in any
//! season without approval — and per-season uploads waiting for approval.
//! The files themselves live on disk (`catalog/<sha256>.apworld`,
//! `seasons/<id>/pending/`); these tables hold what they are and who sent them.

use crate::id::Id;
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
             season TEXT NOT NULL REFERENCES season(id) ON DELETE CASCADE,
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

    pub async fn world_requests(&self, season: Id) -> Vec<WorldRequest> {
        sqlx::query_as::<_, WorldRequest>(
            "SELECT file, sha256, game, uploaded_by, uploaded_at
             FROM world_request WHERE season = ? ORDER BY uploaded_at",
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    pub async fn world_request(&self, season: Id, file: &str) -> Option<WorldRequest> {
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

    pub async fn add_world_request(&self, season: Id, r: &WorldRequest) -> Result<(), sqlx::Error> {
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
    pub async fn take_world_request(&self, season: Id, file: &str) -> Option<WorldRequest> {
        let r = self.world_request(season, file).await?;
        sqlx::query("DELETE FROM world_request WHERE season = ? AND file = ?")
            .bind(season)
            .bind(file)
            .execute(&self.pool)
            .await
            .ok()?;
        Some(r)
    }

    pub async fn clear_world_requests(&self, season: Id) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM world_request WHERE season = ?")
            .bind(season)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Submissions held for `file`: (id, YAML path).
    pub async fn held_submissions(&self, season: Id, file: &str) -> Vec<(Id, String)> {
        sqlx::query_as::<_, (Id, String)>(
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
    pub async fn stale_holds(&self, season: Id) -> Vec<String> {
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

    /// A held submission's file passed its check: take the slot name and game the
    /// check read, and stop holding it. Fails if that slot name is taken. If its
    /// host replaced the file meanwhile, it stays as it is.
    pub async fn release_submission(
        &self,
        id: Id,
        yaml_file: &str,
        slot_name: &str,
        game: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE submission SET slot_name = ?, game = ?, held_for = NULL, held_err = NULL
             WHERE id = ? AND yaml_file = ? AND held_for IS NOT NULL",
        )
        .bind(slot_name)
        .bind(game)
        .bind(id)
        .bind(yaml_file)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Why a held submission's file can't go ahead, unless it's been replaced since.
    pub async fn set_held_err(
        &self,
        id: Id,
        yaml_file: &str,
        err: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE submission SET held_err = ? WHERE id = ? AND yaml_file = ? AND held_for IS NOT NULL",
        )
        .bind(err)
        .bind(id)
        .bind(yaml_file)
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
                "/x.yaml",
                Some("foo.apworld"),
                0,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            db.held_submissions(season, "foo.apworld").await,
            vec![(id, "/x.yaml".into())]
        );
        db.set_held_err(id, "/x.yaml", "nope").await.unwrap();
        let sub = &db.submissions(season).await.unwrap()[0];
        assert!(sub.held() && sub.held_err.as_deref() == Some("nope"));
        db.release_submission(id, "/x.yaml", "Alice2", "Foo")
            .await
            .unwrap();
        let sub = &db.submissions(season).await.unwrap()[0];
        assert!(!sub.held() && sub.held_err.is_none() && sub.slot_name == "Alice2");
        assert_eq!(db.take_world_request(season, "foo.apworld").await, Some(r));
        assert!(db.world_requests(season).await.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_game_changes_only_for_its_host_and_while_the_season_allows() {
        use crate::db::State;
        let (db, dir) = db("game-edits").await;
        let season = db.create_season("S", None, 0).await.unwrap();
        let host = db
            .create_account("host", "h", crate::auth::Role::User, 0)
            .await
            .unwrap();
        let other = db
            .create_account("other", "h", crate::auth::Role::User, 0)
            .await
            .unwrap();
        let id = db
            .add_submission(
                season,
                host,
                "Old",
                "Foo",
                true,
                "/old.yaml",
                Some("foo.apworld"),
                0,
            )
            .await
            .unwrap()
            .unwrap();
        // Draft: notes yes, the YAML no.
        assert!(
            db.set_submission_notes(season, id, host, false, "n")
                .await
                .unwrap()
        );
        assert!(
            !db.set_submission_notes(season, id, other, false, "x")
                .await
                .unwrap()
        );
        assert!(
            !db.set_submission_notes(Id::new(), id, host, false, "x")
                .await
                .unwrap()
        );
        let replace = |account, old: &'static str, game: &'static str, new: &'static str| {
            let db = &db;
            async move {
                db.replace_submission(season, id, account, false, false, "New", game, old, new)
                    .await
                    .unwrap()
            }
        };
        assert!(!replace(host, "/old.yaml", "Foo", "/a.yaml").await);
        db.transition(season, State::Signup, 1).await.unwrap();
        assert!(!replace(other, "/old.yaml", "Foo", "/a.yaml").await);
        assert!(
            !replace(host, "/stale.yaml", "Foo", "/a.yaml").await,
            "lost a race"
        );
        assert!(replace(host, "/old.yaml", "Foo", "/a.yaml").await);
        let sub = db.submission(season, id).await.unwrap();
        assert_eq!(
            (
                sub.slot_name.as_str(),
                sub.yaml_file.as_str(),
                sub.notes.as_str()
            ),
            ("New", "/a.yaml", "n")
        );
        assert!(
            sub.is_custom && sub.held_for.is_none(),
            "same game, no hold"
        );
        // A re-check of the old file that was already running changes nothing.
        db.release_submission(id, "/old.yaml", "Stale", "Stale")
            .await
            .unwrap();
        db.set_held_err(id, "/old.yaml", "stale").await.unwrap();
        let sub = db.submission(season, id).await.unwrap();
        assert_eq!(sub.slot_name, "New");
        assert!(sub.held_err.is_none());
        assert!(replace(host, "/a.yaml", "Bar", "/b.yaml").await);
        assert!(
            !db.submission(season, id).await.unwrap().is_custom,
            "another game"
        );
        // A moderator can in a closed season too, and with notes at any stage.
        db.transition(season, State::Draft, 1).await.unwrap();
        assert!(
            !replace(host, "/b.yaml", "Bar", "/m.yaml").await,
            "signups are closed"
        );
        assert!(
            db.replace_submission(
                season, id, other, true, false, "New", "Bar", "/b.yaml", "/m.yaml"
            )
            .await
            .unwrap()
        );
        assert!(
            db.replace_submission(
                season, id, other, true, false, "New", "Bar", "/m.yaml", "/b.yaml"
            )
            .await
            .unwrap()
        );
        db.transition(season, State::Signup, 1).await.unwrap();
        // Once generating: neither.
        db.transition(season, State::Generating, 2).await.unwrap();
        assert!(
            !db.set_submission_notes(season, id, host, false, "x")
                .await
                .unwrap()
        );
        assert!(!replace(host, "/b.yaml", "Bar", "/c.yaml").await);
        assert!(
            !db.replace_submission(
                season, id, other, true, false, "New", "Bar", "/b.yaml", "/c.yaml"
            )
            .await
            .unwrap(),
            "not even a moderator once it's generating"
        );
        assert!(
            db.set_submission_notes(season, id, other, true, "m")
                .await
                .unwrap()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn the_games_limit_holds_per_player_and_season() {
        let (db, dir) = db("games-limit").await;
        let s1 = db.create_season("S1", None, 0).await.unwrap();
        let s2 = db.create_season("S2", None, 0).await.unwrap();
        let a = db
            .create_account("a", "h", crate::auth::Role::User, 0)
            .await
            .unwrap();
        let b = db
            .create_account("b", "h", crate::auth::Role::User, 0)
            .await
            .unwrap();
        let add = |season, account, name: &'static str, limit| {
            let db = &db;
            async move {
                db.add_submission(season, account, name, "G", false, "/y", None, limit)
                    .await
                    .unwrap()
                    .is_some()
            }
        };
        assert!(add(s1, a, "A1", 2).await);
        assert!(add(s1, a, "A2", 2).await);
        assert!(!add(s1, a, "A3", 2).await, "past the limit");
        assert!(add(s1, b, "B1", 2).await, "someone else's count is theirs");
        assert!(
            add(s2, a, "A4", 2).await,
            "another season's count is its own"
        );
        assert!(add(s1, a, "A5", 0).await, "0 is no limit");
        let names: Vec<_> = db
            .submissions(s1)
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.slot_name)
            .collect();
        assert_eq!(names, ["A1", "A2", "B1", "A5"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
