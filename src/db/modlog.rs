//! The moderator log: what moderators and sysadmins did in season and
//! instance administration (see `crate::modlog`), for sysadmins.

use crate::id::Id;
use sqlx::FromRow;

use super::Db;

#[derive(Debug, FromRow, Clone, PartialEq)]
pub struct ModAction {
    pub id: i64,
    pub at: i64,
    pub username: String,
    pub season_id: Option<Id>,
    pub season_name: String,
    pub action: String,
    pub detail: String,
    pub status: i64,
}

/// What to show: each filter is off when empty. `before`: only rows older
/// than this id (paging).
#[derive(Debug, Default, Clone)]
pub struct ModLogFilter {
    pub username: String,
    pub season: String,
    pub action: String,
    pub before: Option<i64>,
}

/// A page of the log.
pub const PAGE: i64 = 100;

/// [`PAGE`], for pages that list it.
pub fn modlog_page() -> i64 {
    PAGE
}

impl Db {
    #[allow(clippy::too_many_arguments)]
    pub async fn log_mod_action(
        &self,
        at: i64,
        account: Option<i64>,
        username: &str,
        season: Option<Id>,
        season_name: &str,
        action: &str,
        detail: &str,
        status: u16,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO mod_action (at, account_id, username, season_id, season_name, action, detail, status)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(at)
        .bind(account)
        .bind(username)
        .bind(season)
        .bind(season_name)
        .bind(action)
        .bind(detail)
        .bind(status as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The newest rows matching `f`, newest first, at most [`PAGE`]. The
    /// season filter matches the season's id, or its name for rows whose
    /// season is gone.
    pub async fn mod_actions(&self, f: &ModLogFilter) -> Vec<ModAction> {
        sqlx::query_as(
            "SELECT id, at, username, season_id, season_name, action, detail, status FROM mod_action
             WHERE (?1 = '' OR username = ?1 COLLATE NOCASE)
               AND (?2 = '' OR season_id = ?2 OR (season_id IS NULL AND season_name = ?2))
               AND (?3 = '' OR action = ?3)
               AND (?4 IS NULL OR id < ?4)
             ORDER BY id DESC LIMIT ?5",
        )
        .bind(&f.username)
        .bind(&f.season)
        .bind(&f.action)
        .bind(f.before)
        .bind(PAGE)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// What the filters can choose from: (usernames, seasons as (key, name),
    /// actions), each sorted. A season's key is its id, or its name once it's
    /// gone.
    pub async fn mod_log_choices(&self) -> (Vec<String>, Vec<(String, String)>, Vec<String>) {
        let users = sqlx::query_scalar(
            "SELECT DISTINCT username FROM mod_action ORDER BY username COLLATE NOCASE",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        let seasons = sqlx::query_as(
            "SELECT DISTINCT COALESCE(season_id, season_name), season_name FROM mod_action
             WHERE season_id IS NOT NULL OR season_name != '' ORDER BY season_name COLLATE NOCASE",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        let actions = sqlx::query_scalar("SELECT DISTINCT action FROM mod_action ORDER BY action")
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default();
        (users, seasons, actions)
    }

    /// Drop rows older than `before` (seconds). Returns how many.
    pub async fn prune_mod_actions(&self, before: i64) -> u64 {
        sqlx::query("DELETE FROM mod_action WHERE at < ?")
            .bind(before)
            .execute(&self.pool)
            .await
            .map(|r| r.rows_affected())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Role;

    #[tokio::test]
    async fn the_log_filters_pages_and_outlives_its_season() {
        let path = std::env::temp_dir().join(format!("bifrost-dbmodlog-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Db::open(&path.to_string_lossy()).await.unwrap();
        let m = db
            .create_account("mia", "x", Role::Moderator, 0)
            .await
            .unwrap();
        let s = db.create_season("Spring", None, 0).await.unwrap();
        let t = db.create_season("Summer", None, 0).await.unwrap();
        db.log_mod_action(
            1,
            Some(m),
            "mia",
            Some(s),
            "Spring",
            "/admin/generate",
            "",
            200,
        )
        .await
        .unwrap();
        db.log_mod_action(
            2,
            Some(m),
            "mia",
            Some(t),
            "Summer",
            "/admin/console",
            "cmd=/players",
            200,
        )
        .await
        .unwrap();
        db.log_mod_action(
            3,
            None,
            "root",
            None,
            "",
            "/admin/users/role",
            "role=user",
            303,
        )
        .await
        .unwrap();
        let all = db.mod_actions(&ModLogFilter::default()).await;
        assert_eq!(all.iter().map(|a| a.at).collect::<Vec<_>>(), [3, 2, 1]);
        let f = |u: &str, s: &str, a: &str| ModLogFilter {
            username: u.into(),
            season: s.into(),
            action: a.into(),
            before: None,
        };
        assert_eq!(
            db.mod_actions(&f("MIA", "", "")).await.len(),
            2,
            "names match in any case"
        );
        assert_eq!(
            db.mod_actions(&f("", &t.to_string(), "")).await[0].detail,
            "cmd=/players"
        );
        assert_eq!(db.mod_actions(&f("", "", "/admin/generate")).await.len(), 1);
        let before = ModLogFilter {
            before: Some(all[1].id),
            ..Default::default()
        };
        assert_eq!(db.mod_actions(&before).await.len(), 1);
        let (users, seasons, actions) = db.mod_log_choices().await;
        assert_eq!(users, ["mia", "root"]);
        assert_eq!(seasons.len(), 2);
        assert_eq!(actions.len(), 3);

        // The season goes; its rows stay, found by its name.
        db.delete_season(s, true).await.unwrap();
        let gone = db.mod_actions(&f("", "Spring", "")).await;
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].season_id, None);

        assert_eq!(db.prune_mod_actions(3).await, 2, "the rows at 1 and 2");
        let _ = std::fs::remove_file(&path);
    }
}
