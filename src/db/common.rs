//! A season's common games and the names players register for their servers'
//! allowlists. The setups themselves are files: see [`crate::common`].

use super::Db;
use crate::id::Id;

/// A common game a season includes.
#[derive(sqlx::FromRow, Debug, Clone, PartialEq)]
pub struct SeasonCommon {
    pub key: String,
    pub name: String,
    pub slot_name: String,
    /// The season's own address for it; empty for the setup's default.
    pub address: String,
}

/// Result of registering a name for a common game's allowlist.
#[derive(Debug, PartialEq)]
pub enum SetIdentOutcome {
    Ok,
    /// Another player has registered that name for this kind of game.
    TakenByOther,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Db {
    /// The season's common games, by name.
    pub async fn season_commons(&self, season: Id) -> Vec<SeasonCommon> {
        sqlx::query_as(
            "SELECT key, name, slot_name, address FROM season_common
             WHERE season_id = ? ORDER BY name COLLATE NOCASE, key",
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// Add a common game to a season. Fails with a name clash (see
    /// [`super::name_clash`]) if its slot name is a signup's, a seat's or
    /// another common game's.
    pub async fn add_season_common(
        &self,
        season: Id,
        key: &str,
        name: &str,
        slot_name: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO season_common (season_id, key, name, slot_name) VALUES (?, ?, ?, ?)",
        )
        .bind(season)
        .bind(key)
        .bind(name)
        .bind(slot_name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Drop a common game from a season. True if it was there.
    pub async fn remove_season_common(&self, season: Id, key: &str) -> Result<bool, sqlx::Error> {
        let r = sqlx::query("DELETE FROM season_common WHERE season_id = ? AND key = ?")
            .bind(season)
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    /// Set where players join a season's common game (empty for the setup's
    /// default). True if the season has it.
    pub async fn set_common_address(
        &self,
        season: Id,
        key: &str,
        address: &str,
    ) -> Result<bool, sqlx::Error> {
        let r = sqlx::query("UPDATE season_common SET address = ? WHERE season_id = ? AND key = ?")
            .bind(address)
            .bind(season)
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    /// The seasons not yet archived that include a catalog setup, by name.
    pub async fn common_in_use(&self, key: &str) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT se.name FROM season_common c JOIN season se ON se.id = c.season_id
             WHERE c.key = ? AND se.state <> 'archived' ORDER BY se.name COLLATE NOCASE",
        )
        .bind(key)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// The name this player registered for a kind of common game.
    pub async fn game_ident(&self, account: i64, kind: &str) -> Option<String> {
        sqlx::query_scalar("SELECT name FROM game_ident WHERE account_id = ? AND kind = ?")
            .bind(account)
            .bind(kind)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    /// Register (or change) this player's name for a kind of common game,
    /// unless another player has it (ignoring case). The caller checks the
    /// name against the setup's rule.
    pub async fn set_game_ident(
        &self,
        account: i64,
        kind: &str,
        name: &str,
    ) -> Result<SetIdentOutcome, sqlx::Error> {
        let r = sqlx::query(
            "INSERT INTO game_ident (account_id, kind, name, updated_at) VALUES (?, ?, ?, ?)
             ON CONFLICT(account_id, kind) DO UPDATE SET name = excluded.name, updated_at = excluded.updated_at",
        )
        .bind(account)
        .bind(kind)
        .bind(name)
        .bind(now())
        .execute(&self.pool)
        .await;
        match r {
            Ok(_) => Ok(SetIdentOutcome::Ok),
            Err(e)
                if e.as_database_error()
                    .is_some_and(|d| d.is_unique_violation()) =>
            {
                Ok(SetIdentOutcome::TakenByOther)
            }
            Err(e) => Err(e),
        }
    }

    /// A season's allowlist for a kind of common game: the names registered by
    /// everyone with a game of their own in the season (a signup's slot or a
    /// seat on one), in name order.
    pub async fn season_allowlist(&self, season: Id, kind: &str) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT g.name FROM game_ident g
             WHERE g.kind = ?2 AND EXISTS (
                 SELECT 1 FROM claim c JOIN slot s ON s.id = c.slot_id
                 WHERE s.season_id = ?1 AND c.account_id = g.account_id AND s.common_key IS NULL
             )
             ORDER BY g.name COLLATE NOCASE",
        )
        .bind(season)
        .bind(kind)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh(tag: &str) -> (std::path::PathBuf, Db) {
        let path =
            std::env::temp_dir().join(format!("bifrost-dbcommon-{tag}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Db::open(&path.to_string_lossy()).await.unwrap();
        (path, db)
    }

    #[tokio::test]
    async fn a_common_games_slot_name_is_reserved_both_ways() {
        let (dir, db) = fresh("names").await;
        let s = db.create_season("S", None, 0).await.unwrap();
        db.add_season_common(s, "vanilla", "Minecraft", "Vanilla")
            .await
            .unwrap();
        // Another setup with the same slot name, in any case.
        let e = db
            .add_season_common(s, "modded", "Modded", "vanilla")
            .await
            .unwrap_err();
        assert!(super::super::name_clash(&e));
        assert!(db.slot_name_taken(s, "VANILLA").await);
        // The same key twice.
        assert!(
            db.add_season_common(s, "vanilla", "Minecraft", "Other")
                .await
                .is_err()
        );
        // Another season is free to use it.
        let t = db.create_season("T", None, 0).await.unwrap();
        db.add_season_common(t, "vanilla", "Minecraft", "Vanilla")
            .await
            .unwrap();
        assert_eq!(db.season_commons(s).await.len(), 1);
        assert!(db.remove_season_common(s, "vanilla").await.unwrap());
        assert!(!db.slot_name_taken(s, "Vanilla").await);
        let _ = std::fs::remove_file(&dir);
    }

    #[tokio::test]
    async fn a_name_is_one_players_per_kind() {
        let (dir, db) = fresh("idents").await;
        let a = db
            .idp_principal("a", "ann", None, 0)
            .await
            .unwrap()
            .unwrap()
            .account_id;
        let b = db
            .idp_principal("b", "bob", None, 0)
            .await
            .unwrap()
            .unwrap()
            .account_id;
        assert_eq!(
            db.set_game_ident(a, "minecraft", "Steve").await.unwrap(),
            SetIdentOutcome::Ok
        );
        assert_eq!(
            db.set_game_ident(b, "minecraft", "steve").await.unwrap(),
            SetIdentOutcome::TakenByOther
        );
        // Another kind is another namespace.
        assert_eq!(
            db.set_game_ident(b, "factorio", "Steve").await.unwrap(),
            SetIdentOutcome::Ok
        );
        // Renaming your own.
        assert_eq!(
            db.set_game_ident(a, "minecraft", "Alex").await.unwrap(),
            SetIdentOutcome::Ok
        );
        assert_eq!(db.game_ident(a, "minecraft").await.as_deref(), Some("Alex"));
        assert_eq!(
            db.set_game_ident(b, "minecraft", "Steve").await.unwrap(),
            SetIdentOutcome::Ok
        );
        let _ = std::fs::remove_file(&dir);
    }
}
