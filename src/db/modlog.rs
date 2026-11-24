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
    pub outcome: String,
}

/// What to show: each filter is off when empty. `page`: which page, newest
/// first, from 1, of `per` rows (the page asks for as many as fit the
/// screen; 0 means [`PAGE`]).
#[derive(Debug, Default, Clone)]
pub struct ModLogFilter {
    pub username: String,
    pub season: String,
    pub action: String,
    pub page: i64,
    pub per: i64,
}

/// The fewest and most rows a page may ask for.
pub const PER_MIN: i64 = 1;
pub const PER_MAX: i64 = 200;

impl ModLogFilter {
    /// Rows per page, within bounds.
    pub fn per(&self) -> i64 {
        if self.per == 0 {
            PAGE
        } else {
            self.per.clamp(PER_MIN, PER_MAX)
        }
    }
}

/// The filters' WHERE clause (?1 user, ?2 season, ?3 action).
const WHERE: &str = "WHERE (?1 = '' OR username = ?1 COLLATE NOCASE)
               AND (?2 = '' OR season_id = ?2 OR (season_id IS NULL AND season_name = ?2))
               AND (?3 = '' OR action = ?3)";

/// A page of the log.
pub const PAGE: i64 = 50;

impl Db {
    /// Start a row, before the request runs (so a request that never
    /// finishes, or deletes what it's about, is still logged). Returns its id.
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
    ) -> Result<i64, sqlx::Error> {
        let r = sqlx::query(
            "INSERT INTO mod_action (at, account_id, username, season_id, season_name, action, detail, status)
             VALUES (?, ?, ?, ?, ?, ?, ?, 0)",
        )
        .bind(at)
        .bind(account)
        .bind(username)
        .bind(season)
        .bind(season_name)
        .bind(action)
        .bind(detail)
        .execute(&self.pool)
        .await?;
        Ok(r.last_insert_rowid())
    }

    /// Finish a row: the request's status and how it went.
    pub async fn finish_mod_action(&self, id: i64, status: u16, outcome: &str) {
        let _ = sqlx::query("UPDATE mod_action SET status = ?, outcome = ? WHERE id = ?")
            .bind(status as i64)
            .bind(outcome)
            .bind(id)
            .execute(&self.pool)
            .await;
    }

    /// Page `f.page` of the rows matching `f`, newest first.
    pub async fn mod_actions(&self, f: &ModLogFilter) -> Vec<ModAction> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT id, at, username, season_id, season_name, action, detail, status, outcome
             FROM mod_action {WHERE} ORDER BY id DESC LIMIT ?4 OFFSET ?5"
        )))
        .bind(&f.username)
        .bind(&f.season)
        .bind(&f.action)
        .bind(f.per())
        .bind((f.page.max(1) - 1) * f.per())
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// How many rows match `f` (its page aside).
    pub async fn mod_actions_count(&self, f: &ModLogFilter) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM mod_action {WHERE}"
        )))
        .bind(&f.username)
        .bind(&f.season)
        .bind(&f.action)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0)
    }

    /// What the filters can choose from: (usernames, seasons as (key, name,
    /// gone), actions), each sorted. A season's key is its id, or its name
    /// once it's gone.
    #[allow(clippy::type_complexity)]
    pub async fn mod_log_choices(&self) -> (Vec<String>, Vec<(String, String, bool)>, Vec<String>) {
        let users = sqlx::query_scalar(
            "SELECT DISTINCT username FROM mod_action ORDER BY username COLLATE NOCASE",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        let seasons = sqlx::query_as(
            "SELECT DISTINCT COALESCE(season_id, season_name), season_name, season_id IS NULL FROM mod_action
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
        let first = db
            .log_mod_action(1, Some(m), "mia", Some(s), "Spring", "/admin/generate", "")
            .await
            .unwrap();
        db.finish_mod_action(first, 200, "ok").await;
        db.log_mod_action(
            2,
            Some(m),
            "mia",
            Some(t),
            "Summer",
            "/admin/console",
            "cmd=/players",
        )
        .await
        .unwrap();
        db.log_mod_action(3, None, "root", None, "", "/admin/users/role", "role=user")
            .await
            .unwrap();
        let all = db.mod_actions(&ModLogFilter::default()).await;
        assert_eq!(all.iter().map(|a| a.at).collect::<Vec<_>>(), [3, 2, 1]);
        assert_eq!((all[2].status, all[2].outcome.as_str()), (200, "ok"));
        assert_eq!(
            (all[1].status, all[1].outcome.as_str()),
            (0, "unfinished"),
            "not finished yet"
        );
        let f = |u: &str, s: &str, a: &str| ModLogFilter {
            username: u.into(),
            season: s.into(),
            action: a.into(),
            page: 1,
            per: 0,
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
        assert_eq!(db.mod_actions_count(&f("mia", "", "")).await, 2);
        // Pages of PAGE: a page past the rows is empty, a page of 0 is the first.
        for i in 0..PAGE {
            db.log_mod_action(10 + i, None, "bulk", None, "", "/admin/generate", "")
                .await
                .unwrap();
        }
        let bulk = |page| ModLogFilter {
            username: "bulk".into(),
            page,
            ..Default::default()
        };
        assert_eq!(db.mod_actions_count(&bulk(1)).await, PAGE);
        assert_eq!(db.mod_actions(&bulk(1)).await.len() as i64, PAGE);
        assert_eq!(db.mod_actions(&bulk(0)).await.len() as i64, PAGE);
        assert!(db.mod_actions(&bulk(2)).await.is_empty());
        let small = ModLogFilter {
            username: "bulk".into(),
            page: 2,
            per: 10,
            ..Default::default()
        };
        assert_eq!(
            db.mod_actions(&small).await.len(),
            10,
            "a page of its own size"
        );
        let per = |per| {
            ModLogFilter {
                per,
                ..Default::default()
            }
            .per()
        };
        assert_eq!((per(0), per(-3), per(9999)), (PAGE, PER_MIN, PER_MAX));
        let mut all = db
            .mod_actions(&ModLogFilter {
                page: 2,
                ..Default::default()
            })
            .await;
        assert_eq!(
            all.iter().map(|a| a.at).collect::<Vec<_>>(),
            [3, 2, 1],
            "page 2: the oldest three"
        );
        all.clear();
        let (users, seasons, actions) = db.mod_log_choices().await;
        assert_eq!(users, ["bulk", "mia", "root"]);
        assert_eq!(seasons.len(), 2);
        assert_eq!(actions.len(), 3);

        // The season goes (its own row written first, as the middleware
        // does); its rows stay, found by its name.
        let del = db
            .log_mod_action(
                4,
                Some(m),
                "mia",
                Some(s),
                "Spring",
                "/admin/season/delete",
                "",
            )
            .await
            .unwrap();
        db.delete_season(s, true).await.unwrap();
        db.finish_mod_action(del, 303, "ok").await;
        let gone = db.mod_actions(&f("", "Spring", "")).await;
        assert_eq!(gone.len(), 2);
        assert!(gone.iter().all(|g| g.season_id.is_none()));
        assert_eq!(gone[0].outcome, "ok", "the deletion's own row is finished");
        assert!(
            db.mod_log_choices()
                .await
                .1
                .iter()
                .any(|(k, n, gone)| k == "Spring" && n == "Spring" && *gone)
        );

        assert_eq!(db.prune_mod_actions(3).await, 2, "the rows at 1 and 2");
        let _ = std::fs::remove_file(&path);
    }
}
