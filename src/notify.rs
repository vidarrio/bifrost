//! What Bifrost tells people, through the header's mailbox (see
//! `db::notify`). Each helper words one kind of notification and leaves it
//! for whoever it's about; a failure to store one is logged, never passed on.

use crate::db::Db;
use crate::id::Id;

async fn leave(db: &Db, account: i64, season: Id, text: &str, link: &str) {
    if let Err(e) = db
        .notify(account, Some(season), text, link, crate::clock::now())
        .await
    {
        tracing::warn!("notifying account {account}: {e}");
    }
}

async fn season_name(db: &Db, season: Id) -> String {
    db.season(season)
        .await
        .map(|s| s.name)
        .unwrap_or_else(|| "a season".into())
}

/// A signup needs its player's attention: its YAML failed a check (against
/// a newly approved or replaced world), or its world was rejected.
pub async fn signup_flagged(db: &Db, season: Id, submission: Id, why: &str) {
    let Some((account, slot)) = db.submission_owner(submission).await else {
        return;
    };
    let name = season_name(db, season).await;
    let why = why.trim_end_matches('.');
    leave(
        db,
        account,
        season,
        &format!("Your signup {slot} in {name} needs attention: {why}."),
        &format!("/s/{season}/signup/game/{submission}"),
    )
    .await;
}

/// A moderator decided on a world someone uploaded.
pub async fn world_decided(
    db: &Db,
    season: Id,
    uploader: &str,
    file: &str,
    approved: bool,
    by: &str,
) {
    if uploader == by {
        return;
    }
    let Some(acc) = db.account_by_name(uploader).await else {
        return;
    };
    let name = season_name(db, season).await;
    let text = if approved {
        format!("{by} approved your {file} in {name}.")
    } else {
        format!("{by} rejected your {file} in {name}.")
    };
    leave(db, acc.id, season, &text, &format!("/s/{season}/signup")).await;
}

/// Someone prioritized a hint for an item that's in another player's world:
/// tell that world's players (but not whoever prioritized it).
#[allow(clippy::too_many_arguments)]
pub async fn prioritized(
    db: &Db,
    season: Id,
    finding: i64,
    by_account: i64,
    by: &str,
    names: (String, String, String, String),
) {
    let (finder, receiver, item, location) = names;
    let name = season_name(db, season).await;
    let text = format!(
        "{by} prioritized {receiver}'s {item}, at {location} in your world {finder} ({name})."
    );
    for account in db.ap_slot_claimants(season, finding).await {
        if account != by_account {
            leave(db, account, season, &text, &format!("/s/{season}/board")).await;
        }
    }
}
