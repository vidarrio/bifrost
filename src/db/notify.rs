//! Notifications: short messages for one account, shown in the header's
//! mailbox. Read ones stay listed; only the most recent [`KEEP`] per account
//! are kept.

use crate::id::Id;
use sqlx::FromRow;

use super::Db;

/// How many notifications an account keeps.
pub const KEEP: i64 = 50;

#[derive(Debug, FromRow, Clone, PartialEq, serde::Serialize)]
pub struct Notification {
    pub id: i64,
    pub season_id: Option<Id>,
    pub text: String,
    /// A path on this site to go to, or empty.
    pub link: String,
    pub created_at: i64,
    pub read: bool,
}

impl Db {
    /// Leave `account` a notification (and drop its oldest beyond [`KEEP`]).
    pub async fn notify(
        &self,
        account: i64,
        season: Option<Id>,
        text: &str,
        link: &str,
        now: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO notification (account_id, season_id, text, link, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(account)
        .bind(season)
        .bind(text)
        .bind(link)
        .bind(now)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "DELETE FROM notification WHERE account_id = ?1 AND id NOT IN
               (SELECT id FROM notification WHERE account_id = ?1 ORDER BY id DESC LIMIT ?2)",
        )
        .bind(account)
        .bind(KEEP)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// An account's notifications, newest first.
    pub async fn notifications(&self, account: i64) -> Vec<Notification> {
        sqlx::query_as(
            "SELECT id, season_id, text, link, created_at, read FROM notification
             WHERE account_id = ? ORDER BY id DESC LIMIT ?",
        )
        .bind(account)
        .bind(KEEP)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// How many of an account's notifications are unread.
    pub async fn unread_notifications(&self, account: i64) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM notification WHERE account_id = ? AND read = 0")
            .bind(account)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0)
    }

    /// Who claims an AP slot's Bifrost slots (not a common game's, which
    /// everyone in the season plays).
    pub async fn ap_slot_claimants(&self, season: Id, ap_slot: i64) -> Vec<i64> {
        sqlx::query_scalar(
            "SELECT DISTINCT c.account_id FROM slot s JOIN claim c ON c.slot_id = s.id
             WHERE s.season_id = ? AND s.ap_slot = ? AND s.common_key IS NULL",
        )
        .bind(season)
        .bind(ap_slot)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// A signup's owner and slot name.
    pub async fn submission_owner(&self, id: Id) -> Option<(i64, String)> {
        sqlx::query_as("SELECT account_id, slot_name FROM submission WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    /// Mark an account's notifications read, up to and including `upto` (the
    /// newest the viewer saw), so one that arrives meanwhile stays unread.
    pub async fn mark_notifications_read(&self, account: i64, upto: i64) {
        let _ = sqlx::query(
            "UPDATE notification SET read = 1 WHERE account_id = ? AND id <= ? AND read = 0",
        )
        .bind(account)
        .bind(upto)
        .execute(&self.pool)
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Role;

    #[tokio::test]
    async fn notifications_are_kept_listed_read_and_pruned() {
        let path = std::env::temp_dir().join(format!("bifrost-dbnotify-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Db::open(&path.to_string_lossy()).await.unwrap();
        let a = db.create_account("ann", "x", Role::User, 0).await.unwrap();
        let b = db.create_account("bob", "x", Role::User, 0).await.unwrap();
        let s = db.create_season("S", None, 0).await.unwrap();
        db.notify(a, Some(s), "one", "/s/x/signup", 1)
            .await
            .unwrap();
        db.notify(a, None, "two", "", 2).await.unwrap();
        db.notify(b, None, "bob's", "", 3).await.unwrap();
        let n = db.notifications(a).await;
        assert_eq!(
            n.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
            ["two", "one"]
        );
        assert_eq!(n[1].season_id, Some(s));
        assert_eq!(db.unread_notifications(a).await, 2);

        // Read up to what was seen: a newer one stays unread.
        db.mark_notifications_read(a, n[0].id).await;
        db.notify(a, None, "three", "", 4).await.unwrap();
        assert_eq!(db.unread_notifications(a).await, 1);
        assert!(
            db.notifications(a)
                .await
                .iter()
                .filter(|n| n.text != "three")
                .all(|n| n.read)
        );
        assert_eq!(
            db.unread_notifications(b).await,
            1,
            "another account's are its own"
        );

        for i in 0..KEEP + 5 {
            db.notify(a, None, &format!("n{i}"), "", 10 + i)
                .await
                .unwrap();
        }
        let n = db.notifications(a).await;
        assert_eq!(n.len() as i64, KEEP);
        assert_eq!(n[0].text, format!("n{}", KEEP + 4));

        // A season's go with it.
        db.notify(b, Some(s), "in S", "", 99).await.unwrap();
        db.delete_season(s, true).await.unwrap();
        assert!(db.notifications(b).await.iter().all(|n| n.text != "in S"));
        let _ = std::fs::remove_file(&path);
    }
}
