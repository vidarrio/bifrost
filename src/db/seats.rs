//! Co-op seats: partners join a game through a one-use link from its host.
//!
//! Before generation a seat belongs to the signup (`seat`) and generation mints
//! its Bifrost slot and claim. Once live, using a link mints them directly.

use super::{Db, SubmissionRow};
use crate::clock::now;
use crate::id::Id;

/// One co-op partner on a game, before generation.
#[derive(sqlx::FromRow, Debug, Clone)]
pub struct SeatRow {
    pub id: Id,
    pub account_id: i64,
    pub username: String,
    pub name: String,
}

/// A link's game: a signup, or (for a live season) a slot by its connect name.
#[derive(sqlx::FromRow, Debug, Clone)]
pub struct SeatLink {
    pub submission_id: Option<Id>,
    pub slot_name: Option<String>,
}

/// How using a seat link went.
#[derive(Debug, PartialEq)]
pub enum JoinOutcome {
    Joined,
    /// Used, revoked, or the season can't take seats at the moment.
    LinkGone,
    NameTaken,
    AlreadyIn,
}

/// Why a seat name can't be used, if it can't. Archipelago caps names at 16.
pub fn seat_name_error(name: &str) -> Option<&'static str> {
    let n = name.chars().count();
    if n == 0 || n > 16 {
        return Some("A seat name has 1 to 16 characters.");
    }
    if name
        .chars()
        .any(|c| c.is_control() || matches!(c, '{' | '}' | ','))
    {
        return Some("A seat name can't have line breaks, commas or curly brackets.");
    }
    None
}

/// Whether an error says a slot or seat name is already in use in the season.
pub fn name_clash(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .is_some_and(|d| d.is_unique_violation() || d.message().contains("slot name taken"))
}

impl Db {
    /// Whether a name is in use in a season: a signup, a seat, a common game or a generated
    /// slot (before generation, slots are only
    /// what a failed attempt left, which the next one clears).
    pub async fn slot_name_taken(&self, season: Id, name: &str) -> bool {
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT slot_name FROM submission WHERE season_id = ?1
             UNION ALL SELECT name FROM seat WHERE season_id = ?1
             UNION ALL SELECT slot_name FROM season_common WHERE season_id = ?1
             UNION ALL SELECT name FROM slot WHERE season_id = ?1
               AND (SELECT state FROM season WHERE id = ?1) NOT IN ('draft', 'signup')",
        )
        .bind(season)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        // In Rust: SQLite only folds ASCII, and Archipelago folds all of it.
        let name = name.to_lowercase();
        names.iter().any(|n| n.to_lowercase() == name)
    }

    pub async fn add_seat_link(
        &self,
        season: Id,
        token: &str,
        submission: Option<Id>,
        slot_name: Option<&str>,
        created_by: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO seat_link (token, season_id, submission_id, slot_name, created_by, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(token)
        .bind(season)
        .bind(submission)
        .bind(slot_name)
        .bind(created_by)
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn seat_link(&self, season: Id, token: &str) -> Option<SeatLink> {
        sqlx::query_as(
            "SELECT submission_id, slot_name FROM seat_link WHERE season_id = ? AND token = ?",
        )
        .bind(season)
        .bind(token)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    /// A signup's unused links.
    pub async fn seat_links_of_signup(&self, submission: Id) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT token FROM seat_link WHERE submission_id = ? ORDER BY created_at, token",
        )
        .bind(submission)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// A live game's unused links, by its connect name: those made on the live
    /// page and any left over from its signup.
    pub async fn seat_links_of_slot(&self, season: Id, slot_name: &str) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT l.token FROM seat_link l
             LEFT JOIN submission s ON s.id = l.submission_id
             WHERE l.season_id = ?1 AND (l.slot_name = ?2 OR s.slot_name = ?2)
             ORDER BY l.created_at, l.token",
        )
        .bind(season)
        .bind(slot_name)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// Revoke an unused link, given the links its host may revoke.
    pub async fn revoke_seat_link(&self, season: Id, token: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM seat_link WHERE season_id = ? AND token = ?")
            .bind(season)
            .bind(token)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn seats_of(&self, submission: Id) -> Vec<SeatRow> {
        self.try_seats_of(submission).await.unwrap_or_default()
    }

    pub async fn try_seats_of(&self, submission: Id) -> Result<Vec<SeatRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT s.id, s.account_id, a.username, s.name
             FROM seat s JOIN account a ON a.id = s.account_id
             WHERE s.submission_id = ? ORDER BY s.rowid",
        )
        .bind(submission)
        .fetch_all(&self.pool)
        .await
    }

    /// Use a link before generation: it's spent, and the partner has a seat.
    pub async fn join_signup(
        &self,
        season: Id,
        token: &str,
        sub: &SubmissionRow,
        account: i64,
        name: &str,
    ) -> Result<JoinOutcome, sqlx::Error> {
        if sub.account_id == account
            || self
                .seats_of(sub.id)
                .await
                .iter()
                .any(|s| s.account_id == account)
        {
            return Ok(JoinOutcome::AlreadyIn);
        }
        let mut tx = self.pool.begin().await?;
        let spent = sqlx::query(
            "DELETE FROM seat_link WHERE token = ?1 AND season_id = ?2
               AND (SELECT state FROM season WHERE id = ?2) IN ('draft', 'signup')",
        )
        .bind(token)
        .bind(season)
        .execute(&mut *tx)
        .await?;
        if spent.rows_affected() == 0 {
            return Ok(JoinOutcome::LinkGone);
        }
        let added = sqlx::query(
            "INSERT INTO seat (id, season_id, submission_id, account_id, name, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(Id::new())
        .bind(season)
        .bind(sub.id)
        .bind(account)
        .bind(name)
        .bind(now())
        .execute(&mut *tx)
        .await;
        match added {
            Ok(_) => {
                tx.commit().await?;
                Ok(JoinOutcome::Joined)
            }
            Err(e) if name_clash(&e) => Ok(JoinOutcome::NameTaken),
            Err(e) => Err(e),
        }
    }

    /// Use a link on a live season: it's spent, and the partner has a slot of
    /// the game, claimed.
    #[allow(clippy::too_many_arguments)]
    pub async fn join_live(
        &self,
        season: Id,
        token: &str,
        ap_slot: i64,
        game_id: Id,
        game: &str,
        account: i64,
        name: &str,
    ) -> Result<JoinOutcome, sqlx::Error> {
        if self.claims_ap_slot(season, account, ap_slot).await {
            return Ok(JoinOutcome::AlreadyIn);
        }
        let mut tx = self.pool.begin().await?;
        let spent = sqlx::query(
            "DELETE FROM seat_link WHERE token = ?1 AND season_id = ?2
               AND (SELECT state FROM season WHERE id = ?2) = 'live'",
        )
        .bind(token)
        .bind(season)
        .execute(&mut *tx)
        .await?;
        if spent.rows_affected() == 0 {
            return Ok(JoinOutcome::LinkGone);
        }
        let slot_id = Id::new();
        let slot = sqlx::query(
            "INSERT INTO slot (id, season_id, game_id, name, ap_slot, game, is_seat) VALUES (?, ?, ?, ?, ?, ?, 1)",
        )
        .bind(slot_id)
        .bind(season)
        .bind(game_id)
        .bind(name)
        .bind(ap_slot)
        .bind(game)
        .execute(&mut *tx)
        .await;
        match slot {
            Ok(_) => {}
            Err(e) if name_clash(&e) => return Ok(JoinOutcome::NameTaken),
            Err(e) => return Err(e),
        }
        sqlx::query("INSERT INTO claim (slot_id, account_id, claimed_at) VALUES (?, ?, ?)")
            .bind(slot_id)
            .bind(account)
            .bind(now())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(JoinOutcome::Joined)
    }

    /// Rename your own seat, before generation.
    pub async fn rename_seat(
        &self,
        season: Id,
        seat: Id,
        account: i64,
        name: &str,
    ) -> Result<JoinOutcome, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE seat SET name = ?1 WHERE id = ?2 AND season_id = ?3 AND account_id = ?4
               AND (SELECT state FROM season WHERE id = ?3) IN ('draft', 'signup')",
        )
        .bind(name)
        .bind(seat)
        .bind(season)
        .bind(account)
        .execute(&self.pool)
        .await;
        match res {
            Ok(r) if r.rows_affected() > 0 => Ok(JoinOutcome::Joined),
            Ok(_) => Ok(JoinOutcome::LinkGone),
            Err(e) if name_clash(&e) => Ok(JoinOutcome::NameTaken),
            Err(e) => Err(e),
        }
    }

    /// Leave a game you're a partner on, before generation.
    pub async fn leave_seat(
        &self,
        season: Id,
        submission: Id,
        account: i64,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "DELETE FROM seat WHERE submission_id = ?1 AND season_id = ?2 AND account_id = ?3
               AND (SELECT state FROM season WHERE id = ?2) IN ('draft', 'signup')",
        )
        .bind(submission)
        .bind(season)
        .bind(account)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Remove a partner from a game, for its host, before generation.
    pub async fn remove_seat(&self, season: Id, seat: Id, host: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "DELETE FROM seat WHERE id = ?1 AND season_id = ?2
               AND submission_id IN (SELECT id FROM submission WHERE season_id = ?2 AND account_id = ?3)
               AND (SELECT state FROM season WHERE id = ?2) IN ('draft', 'signup')",
        )
        .bind(seat)
        .bind(season)
        .bind(host)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::State;

    async fn db(tag: &str) -> (Db, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("bifrost-seats-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(dir.join("t.db").to_str().unwrap()).await.unwrap();
        (db, dir)
    }

    async fn account(db: &Db, name: &str) -> i64 {
        db.create_account(name, "h", crate::auth::Role::User, 0)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn seeding_fails_rather_than_seat_someone_on_another_game() {
        let (db, dir) = db("clash").await;
        let s = db.create_season("S", None, 0).await.unwrap();
        let (host, ann) = (account(&db, "host").await, account(&db, "ann").await);
        let id = db
            .add_submission(s, host, "Host", "G", false, "/y", None, 0)
            .await
            .unwrap()
            .unwrap();
        let sub = db.submission(s, id).await.unwrap();
        db.add_seat_link(s, "t", Some(id), None, host)
            .await
            .unwrap();
        db.join_signup(s, "t", &sub, ann, "Other").await.unwrap();
        let subs = db.submissions(s).await.unwrap();
        let slots = [
            (1, "Other".to_string(), "H".to_string()),
            (2, "Host".to_string(), "G".to_string()),
        ];
        assert!(
            db.seed_from_submissions(s, &slots, &subs, &[])
                .await
                .is_err()
        );
        assert!(!db.claims_ap_slot(s, ann, 1).await);
        let reversed = [slots[1].clone(), slots[0].clone()];
        assert!(
            db.seed_from_submissions(s, &reversed, &subs, &[])
                .await
                .is_err()
        );
        // Slot names ignore case, so "other" is the same name.
        let lower = [
            (1, "other".to_string(), "H".to_string()),
            (2, "Host".to_string(), "G".to_string()),
        ];
        assert!(
            db.seed_from_submissions(s, &lower, &subs, &[])
                .await
                .is_err()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn an_external_season_gets_its_slots_with_shared_game_ids() {
        let (db, dir) = db("external").await;
        let s = db.create_season("S", None, 0).await.unwrap();
        let roster = [
            (1, "Ann".to_string(), "G".to_string()),
            (2, "Bob".to_string(), "H".to_string()),
        ];
        let coop = [("Ann".to_string(), "Cam".to_string())];
        db.seed_season(s, &roster, &coop).await.unwrap();
        let first = db.slots(s).await.unwrap();
        assert_eq!(first.len(), 3);
        let game = |n: &str| first.iter().find(|r| r.name == n).unwrap().game_id;
        assert_eq!(game("Ann"), game("Cam"));
        assert_ne!(game("Ann"), game("Bob"));
        assert_eq!(db.ap_slot_of_game(s, game("Cam")).await, Some(1));
        // Idempotent: nothing new, and the same ids.
        db.seed_season(s, &roster, &coop).await.unwrap();
        let again = db.slots(s).await.unwrap();
        assert_eq!(again.len(), 3);
        assert_eq!(again[0].id, first[0].id);
        assert!(again.iter().find(|r| r.name == "Cam").unwrap().is_seat);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn seat_names_follow_the_slot_rules() {
        assert!(seat_name_error("Ann B").is_none());
        for bad in ["", "seventeen letters", "a,b", "{x}", "a\nb"] {
            assert!(seat_name_error(bad).is_some(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn a_link_seats_one_partner_once_and_names_are_unique() {
        let (db, dir) = db("join").await;
        let s = db.create_season("S", None, 0).await.unwrap();
        let (host, ann, bob) = (
            account(&db, "host").await,
            account(&db, "ann").await,
            account(&db, "bob").await,
        );
        let id = db
            .add_submission(s, host, "Host", "G", false, "/y", None, 0)
            .await
            .unwrap()
            .unwrap();
        let sub = db.submission(s, id).await.unwrap();
        for t in ["t1", "t2", "t3"] {
            db.add_seat_link(s, t, Some(id), None, host).await.unwrap();
        }
        let join = |t: &'static str, a, n: &'static str| {
            let (db, sub) = (&db, &sub);
            async move { db.join_signup(s, t, sub, a, n).await.unwrap() }
        };
        assert_eq!(join("t1", host, "X").await, JoinOutcome::AlreadyIn);
        assert_eq!(join("t1", ann, "Host").await, JoinOutcome::NameTaken);
        assert_eq!(
            db.seat_links_of_signup(id).await.len(),
            3,
            "a refusal spends nothing"
        );
        assert!(db.slot_name_taken(s, "hOST").await);
        assert_eq!(join("t1", ann, "Ann").await, JoinOutcome::Joined);
        assert_eq!(join("t1", bob, "Bob").await, JoinOutcome::LinkGone);
        assert_eq!(join("t2", ann, "Ann2").await, JoinOutcome::AlreadyIn);
        assert_eq!(join("t2", bob, "Ann").await, JoinOutcome::NameTaken);
        assert_eq!(join("t2", bob, "Bob").await, JoinOutcome::Joined);
        // A new signup can't take a seat's name either, and vice versa on rename.
        assert!(
            db.add_submission(s, ann, "Bob", "G", false, "/z", None, 0)
                .await
                .is_err()
        );
        let seats = db.seats_of(id).await;
        assert_eq!(
            db.rename_seat(s, seats[0].id, ann, "Host").await.unwrap(),
            JoinOutcome::NameTaken
        );
        assert_eq!(
            db.rename_seat(s, seats[0].id, bob, "Nope").await.unwrap(),
            JoinOutcome::LinkGone,
            "only your own"
        );
        assert_eq!(
            db.rename_seat(s, seats[0].id, ann, "Annie").await.unwrap(),
            JoinOutcome::Joined
        );
        // Partners share the notes; strangers don't.
        assert!(
            db.set_submission_notes(s, id, ann, false, "n")
                .await
                .unwrap()
        );
        let stranger = account(&db, "stranger").await;
        assert!(
            !db.set_submission_notes(s, id, stranger, false, "x")
                .await
                .unwrap()
        );
        // A partner can leave, and the name is free again.
        assert!(!db.leave_seat(s, id, stranger).await.unwrap());
        assert!(db.leave_seat(s, id, ann).await.unwrap());
        assert!(!db.slot_name_taken(s, "Annie").await);
        db.add_seat_link(s, "t3b", Some(id), None, host)
            .await
            .unwrap();
        db.join_signup(s, "t3b", &sub, ann, "Annie").await.unwrap();
        let seats = db.seats_of(id).await;
        let seats = [seats[1].clone(), seats[0].clone()];
        // Only the host removes, and retracting takes the rest with it.
        assert!(!db.remove_seat(s, seats[1].id, ann).await.unwrap());
        assert!(db.remove_seat(s, seats[1].id, host).await.unwrap());
        db.delete_submission(s, id, host, false).await.unwrap();
        assert!(db.seats_of(id).await.is_empty());
        assert!(db.seat_links_of_signup(id).await.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn generation_seats_partners_and_a_live_link_mints_a_slot() {
        let (db, dir) = db("live").await;
        let s = db.create_season("S", None, 0).await.unwrap();
        let (host, ann, bob) = (
            account(&db, "host").await,
            account(&db, "ann").await,
            account(&db, "bob").await,
        );
        let id = db
            .add_submission(s, host, "Host", "G", false, "/y", None, 0)
            .await
            .unwrap()
            .unwrap();
        let sub = db.submission(s, id).await.unwrap();
        db.add_seat_link(s, "t1", Some(id), None, host)
            .await
            .unwrap();
        db.add_seat_link(s, "t2", Some(id), None, host)
            .await
            .unwrap();
        db.join_signup(s, "t1", &sub, ann, "Ann").await.unwrap();
        let subs = db.submissions(s).await.unwrap();
        assert_eq!(subs[0].seat_names, "Ann");
        db.transition(s, State::Generating, 1).await.unwrap();
        assert_eq!(
            db.join_signup(s, "t2", &sub, bob, "Bob").await.unwrap(),
            JoinOutcome::LinkGone,
            "not while generating"
        );
        db.seed_from_submissions(s, &[(1, "Host".into(), "G".into())], &subs, &[])
            .await
            .unwrap();
        db.transition(s, State::Live, 2).await.unwrap();
        assert!(db.claims_ap_slot(s, host, 1).await && db.claims_ap_slot(s, ann, 1).await);
        // Bifrost connects as the real slot, even if a seat's name sorts first.
        let slots = db.slots(s).await.unwrap();
        assert_eq!(slots[0].name, "Ann", "a seat first, by name");
        assert_eq!(
            crate::db::connect_slot(&slots, None).as_deref(),
            Some("Host")
        );
        assert_eq!(
            crate::db::connect_slot(&slots, Some("Ann")).as_deref(),
            Some("Host")
        );
        // Locked once live: no renames or removals, moderators or not.
        let seat = db.seats_of(id).await[0].id;
        assert_eq!(
            db.rename_seat(s, seat, ann, "Annie").await.unwrap(),
            JoinOutcome::LinkGone
        );
        assert!(!db.remove_seat(s, seat, host).await.unwrap());
        assert!(!db.leave_seat(s, id, ann).await.unwrap(), "nor leaving");
        // A signup's unused link still works, for a mid-season join.
        assert_eq!(db.seat_links_of_slot(s, "Host").await, ["t2"]);
        assert_eq!(
            db.join_live(s, "t2", 1, id, "G", ann, "Ann2")
                .await
                .unwrap(),
            JoinOutcome::AlreadyIn
        );
        assert_eq!(
            db.join_live(s, "t2", 1, id, "G", bob, "Ann").await.unwrap(),
            JoinOutcome::NameTaken
        );
        assert_eq!(
            db.join_live(s, "t2", 1, id, "G", bob, "Bob").await.unwrap(),
            JoinOutcome::Joined
        );
        assert!(db.claims_ap_slot(s, bob, 1).await);
        assert!(db.seat_links_of_slot(s, "Host").await.is_empty());
        assert_eq!(
            db.submissions(s).await.unwrap()[0].seat_names,
            "Ann, Bob",
            "a mid-season joiner shows too"
        );
        // A re-roll keeps them, on the new AP slot, on the new seed, without
        // the old seed's activity.
        db.activity_add(s, 3, "sent", "x", None, None, None).await;
        db.remap_season(s, "S2", &[(7, "Host".into(), "G".into())])
            .await
            .unwrap();
        assert_eq!(db.season(s).await.unwrap().seed.as_deref(), Some("S2"));
        assert_eq!(db.activity_count(s, &Default::default()).await, 0);
        let rerolled = db.slots(s).await.unwrap();
        assert!(rerolled.iter().find(|r| r.name == "Ann").unwrap().is_seat);
        assert!(!rerolled.iter().find(|r| r.name == "Host").unwrap().is_seat);
        assert_eq!(
            db.ap_slot_of_game(s, id).await,
            Some(7),
            "the game keeps its id"
        );
        assert!(db.claims_ap_slot(s, bob, 7).await && db.claims_ap_slot(s, ann, 7).await);
        // Retracting is over too.
        assert_eq!(db.delete_submission(s, id, host, true).await.unwrap(), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
