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
    let link = format!("/s/{season}/board");
    for account in db.ap_slot_claimants(season, finding).await {
        if account != by_account
            && let Err(e) = db
                .notify_once(
                    account,
                    Some(season),
                    &text,
                    &link,
                    crate::clock::now(),
                    None,
                )
                .await
        {
            tracing::warn!("notifying account {account}: {e}");
        }
    }
}

// ---- for those who run seasons and the server

/// Six hours: how long the same alert about a recurring problem (a game
/// server that keeps stopping, backups that keep failing) stays told.
const RECURRING: i64 = 6 * 3600;

async fn to_all(
    db: &Db,
    accounts: Vec<i64>,
    season: Option<Id>,
    text: &str,
    link: &str,
    within: Option<i64>,
) {
    for account in accounts {
        if let Err(e) = db
            .notify_once(account, season, text, link, crate::clock::now(), within)
            .await
        {
            tracing::warn!("notifying account {account}: {e}");
        }
    }
}

/// A season's moderators (its owner and co-moderators), but not `except`.
async fn to_mods(db: &Db, season: Id, except: Option<i64>, text: &str, within: Option<i64>) {
    let mods: Vec<i64> = db
        .season_mods(season)
        .await
        .into_iter()
        .filter(|m| Some(*m) != except)
        .collect();
    to_all(
        db,
        mods,
        Some(season),
        text,
        &format!("/s/{season}/admin"),
        within,
    )
    .await;
}

/// Every sysadmin.
async fn to_sysadmins(db: &Db, text: &str, link: &str, within: Option<i64>) {
    let ids = db.sysadmin_ids().await;
    to_all(db, ids, None, text, link, within).await;
}

/// An upload in a season waits for its moderators' approval.
pub async fn upload_waiting(db: &Db, season: Id, uploader: &str, file: &str, game: &str) {
    let name = season_name(db, season).await;
    let what = if game.is_empty() {
        file.to_string()
    } else {
        format!("{file} ({game})")
    };
    let except = db.account_by_name(uploader).await.map(|a| a.id);
    to_mods(
        db,
        season,
        except,
        &format!("{uploader} uploaded {what} in {name}; it's waiting for approval."),
        None,
    )
    .await;
}

/// A signup in a season needs attention (for its moderators; its player
/// hears through [`signup_flagged`]).
pub async fn signup_flagged_for_mods(db: &Db, season: Id, submission: Id, why: &str) {
    let Some((owner, slot)) = db.submission_owner(submission).await else {
        return;
    };
    let name = season_name(db, season).await;
    let why = why.trim_end_matches('.');
    to_mods(
        db,
        season,
        Some(owner),
        &format!("The signup {slot} in {name} needs attention: {why}."),
        None,
    )
    .await;
}

/// A season's generation ended, well or not.
pub async fn generated(db: &Db, season: Id, outcome: &str) {
    let name = season_name(db, season).await;
    let outcome = outcome.trim_end_matches('.');
    to_mods(db, season, None, &format!("{name}: {outcome}."), None).await;
}

/// A live season's game server stopped on its own; Bifrost restarts it.
pub async fn server_stopped(db: &Db, season: Id, status: &str) {
    let name = season_name(db, season).await;
    let text =
        format!("{name}'s game server stopped unexpectedly ({status}). Bifrost is restarting it.");
    to_mods(db, season, None, &text, Some(RECURRING)).await;
}

/// A world was offered for the catalog.
pub async fn catalog_offer(db: &Db, offer: &crate::worlds::Offer) {
    let what = format!("{} {}", offer.game, offer.version)
        .trim()
        .to_string();
    let text = if offer.approved_by.is_empty() {
        format!(
            "{} uploaded {what}, newer than the catalog's {}: it's offered for the catalog.",
            offer.uploaded_by, offer.replaces
        )
    } else {
        format!(
            "{} approved {what} (uploaded by {}) in a season: it's offered for the catalog.",
            offer.approved_by, offer.uploaded_by
        )
    };
    to_sysadmins(db, &text, "/admin/catalog", None).await;
}

/// A newer Bifrost release is out.
pub async fn release_out(db: &Db, version: &str, current: &str) {
    to_sysadmins(
        db,
        &format!("Bifrost {version} is out (this is {current})."),
        "/admin/settings",
        None,
    )
    .await;
}

/// A backup Bifrost takes on its own failed.
pub async fn backup_failed(db: &Db, error: &str) {
    to_sysadmins(
        db,
        &format!("The nightly backup failed: {error}."),
        "/admin/settings",
        Some(RECURRING),
    )
    .await;
}

/// Installing an Archipelago version failed (or its test generation did).
pub async fn ap_install_failed(db: &Db, version: &str, error: &str) {
    to_sysadmins(
        db,
        &format!("Installing Archipelago {version} failed: {error}."),
        "/admin/settings",
        Some(RECURRING),
    )
    .await;
}
