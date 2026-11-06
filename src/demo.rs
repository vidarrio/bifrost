//! The demo build (`cargo build --features demo`), for a live demo open to
//! anyone. Release builds don't have this module.
//!
//! - One account per role, and a bar on every page that switches between
//!   them without a password (`/demo/as/<role>`). Only sign-in is skipped:
//!   it starts an ordinary session, and permissions and the cross-site check
//!   work as in any build.
//! - Since every visitor can be sysadmin, what would let one run code, host
//!   files, put links on every page or lock others out is turned off
//!   ([`locked`]), and seasons and their games are capped.
//! - Games' servers listen on localhost only.
//! - `BIFROST_DEMO_SNAPSHOT` restores the data dir from a snapshot on every
//!   start, and `BIFROST_DEMO_RESET_MINUTES` stops Bifrost after that long, so
//!   the service manager starts it again, back at the snapshot.
//! - Native sign-in only: it refuses to start in forward-auth mode.

use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::{Path as UrlPath, Query, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;

use crate::auth::{self, Role};
use crate::db::Db;
use crate::{AppState, render};

/// The demo accounts: the bar's label is also the username.
pub const ACCOUNTS: &[(&str, Role)] = &[
    ("player", Role::User),
    ("moderator", Role::Moderator),
    ("sysadmin", Role::Sysadmin),
];

/// At most this many seasons, and this many games in a season.
pub const MAX_SEASONS: usize = 12;
pub const MAX_GAMES: i64 = 12;

/// The bar (the header script appends it, through `render`'s `demo_js!`).
#[cfg(test)]
const BAR_JS: &str = include_str!("demo_bar.js");

const OFF: &str = "That's turned off in the demo.";

/// Make sure every demo account exists, is enabled and has its role. Their
/// passwords are random and thrown away; `/demo/as/<role>` is the way in. Runs
/// before the first-run check, so a demo build never asks for setup.
pub async fn ensure_accounts(db: &Db, mode: auth::AuthMode) {
    assert!(
        mode == auth::AuthMode::Native,
        "a demo build only runs with native sign-in: `bifrost auth mode native`"
    );
    tracing::warn!(
        "DEMO BUILD: anyone who can reach this instance can act as any account, sysadmin \
         included, without a password"
    );
    for &(name, role) in ACCOUNTS {
        match db.account_by_name(name).await {
            Some(a) => {
                if a.role != role.as_str() {
                    let _ = db.set_role(a.id, role).await;
                }
                if a.disabled {
                    let _ = db.set_disabled(a.id, false).await;
                }
            }
            None => {
                let hash = auth::hash_password_async(auth::new_token())
                    .await
                    .expect("hash a throwaway password");
                db.create_account(name, &hash, role, auth::now())
                    .await
                    .expect("create a demo account");
            }
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct Rd {
    rd: Option<String>,
}

/// `/demo/as/<role>`: sign in as that role's demo account and go back.
pub(crate) async fn switch(
    State(st): State<AppState>,
    UrlPath(role): UrlPath<String>,
    Query(q): Query<Rd>,
) -> Response {
    let Some(&(name, _)) = ACCOUNTS.iter().find(|(n, _)| *n == role) else {
        return (
            StatusCode::NOT_FOUND,
            render::notice("No such demo account.", false, "/"),
        )
            .into_response();
    };
    let Some(account) = st.db.account_by_name(name).await else {
        return render::notice(
            "The demo accounts are missing: restart Bifrost.",
            false,
            "/",
        )
        .into_response();
    };
    match st.auth.start_session(account.id).await {
        Ok(cookie) => {
            let mut r = Redirect::to(&auth::safe_redirect(q.rd.as_deref())).into_response();
            r.headers_mut().insert(header::SET_COOKIE, cookie);
            r
        }
        Err(e) => render::notice(&e, false, "/").into_response(),
    }
}

/// `/demo/status`: seconds until the demo resets, for the bar.
pub(crate) async fn status() -> Json<serde_json::Value> {
    let reset_in = RESET_AT
        .get()
        .map(|at| at.saturating_duration_since(Instant::now()).as_secs());
    Json(serde_json::json!({ "reset_in": reset_in }))
}

/// Is this request one the demo turns off? `path` is inside the season
/// rewrite, so `/s/<id>/admin/x` reads `/admin/x`.
pub fn locked(method: &Method, path: &str) -> bool {
    let post = method != Method::GET && method != Method::HEAD;
    let seg = |p: &str| {
        path.strip_prefix(p)
            .is_some_and(|r| r.is_empty() || r.starts_with('/'))
    };
    // A download of the instance's whole database.
    if path.starts_with("/admin/settings/backups/download") {
        return true;
    }
    post && (
        // Instance settings: links on every page, Archipelago installs, backups, the export token.
        seg("/admin/settings")
        // Code and files: apworlds, common games' files, PopTracker packs.
        || path.starts_with("/admin/catalog/")
        || seg("/admin/common-games")
        || matches!(path, "/admin/worlds/approve" | "/admin/worlds/to-catalog" | "/signup/apworld")
        || path.strip_prefix("/game/").is_some_and(|r| r.split('/').nth(1) == Some("poptracker") && r.split('/').count() == 2)
        // Accounts: roles, disabling, invite and reset links.
        || path.starts_with("/admin/users/")
        || path.starts_with("/admin/links/")
    )
}

/// Refuses what [`locked`] says, whoever asks.
pub async fn guard(req: Request, next: Next) -> Response {
    let mut r = if !locked(req.method(), req.uri().path()) {
        next.run(req).await
    } else if req.uri().path() == "/signup/apworld" {
        Json(serde_json::json!({"ok": false, "error": OFF})).into_response()
    } else {
        (StatusCode::FORBIDDEN, render::notice(OFF, false, "/")).into_response()
    };
    mark(r.headers_mut());
    r
}

/// The cap on seasons, for creating one.
pub async fn seasons_full(db: &Db) -> Option<String> {
    (db.seasons().await.len() >= MAX_SEASONS).then(|| {
        format!(
            "The demo holds at most {MAX_SEASONS} seasons: delete one first, or wait for the reset."
        )
    })
}

/// The cap on a season's games, for a signup.
pub async fn games_full(db: &Db, season: crate::id::Id) -> Option<String> {
    (db.games_in(season).await >= MAX_GAMES)
        .then(|| format!("A demo season holds at most {MAX_GAMES} games."))
}

/// Game servers listen on localhost only.
pub fn ap_bind(_configured: String) -> String {
    "127.0.0.1".into()
}

static RESET_AT: OnceLock<Instant> = OnceLock::new();

/// Server start, before anything reads the data dir: with
/// `BIFROST_DEMO_SNAPSHOT`, replace the data dir (`BIFROST_DATA`, which must be
/// set) with a copy of the snapshot.
pub fn restore_snapshot() {
    let Some(snap) = std::env::var_os("BIFROST_DEMO_SNAPSHOT") else {
        return;
    };
    let data =
        std::env::var_os("BIFROST_DATA").expect("BIFROST_DEMO_SNAPSHOT needs BIFROST_DATA set too");
    let (snap, data) = (Path::new(&snap), Path::new(&data));
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    assert!(
        !real(snap).starts_with(real(data)) && !real(data).starts_with(real(snap)),
        "BIFROST_DEMO_SNAPSHOT and BIFROST_DATA mustn't contain each other"
    );
    assert!(
        snap.join("bifrost.db").is_file(),
        "BIFROST_DEMO_SNAPSHOT ({}) isn't a Bifrost data dir",
        snap.display()
    );
    if let Ok(entries) = std::fs::read_dir(data) {
        for e in entries.flatten() {
            let p = e.path();
            let r = if e.file_type().is_ok_and(|t| t.is_dir()) {
                std::fs::remove_dir_all(&p)
            } else {
                std::fs::remove_file(&p)
            };
            r.unwrap_or_else(|err| panic!("clear {}: {err}", p.display()));
        }
    }
    copy_dir(snap, data).unwrap_or_else(|e| panic!("restore the demo snapshot: {e}"));
    eprintln!("bifrost: demo data restored from {}", snap.display());
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let t = e.file_type()?;
        if t.is_dir() {
            copy_dir(&src, &dst)?;
        } else if t.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// Resolves when the demo is due to reset (`BIFROST_DEMO_RESET_MINUTES`);
/// never without it. Bifrost then shuts down as on SIGTERM, for the service
/// manager to start it again.
pub async fn reset_due() {
    let minutes = std::env::var("BIFROST_DEMO_RESET_MINUTES")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|m| *m > 0.0);
    let Some(minutes) = minutes else {
        return std::future::pending().await;
    };
    let after = Duration::from_secs_f64(minutes * 60.0);
    let _ = RESET_AT.set(Instant::now() + after);
    tracing::info!("demo: resetting in {minutes} minute(s)");
    tokio::time::sleep(after).await;
    tracing::info!("demo: time for the reset");
}

/// A demo build announces itself on every response, so a test (or a curious
/// operator) can tell builds apart without signing in.
fn mark(headers: &mut HeaderMap) {
    headers.insert("x-bifrost-demo", HeaderValue::from_static("1"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_demo_account_per_role() {
        for role in [Role::User, Role::Moderator, Role::Sysadmin] {
            assert_eq!(ACCOUNTS.iter().filter(|a| a.1 == role).count(), 1);
        }
        for (name, _) in ACCOUNTS {
            assert!(auth::valid_username(name), "{name}");
            assert!(BAR_JS.contains(&format!("'{name}'")), "{name}");
        }
        assert!(!BAR_JS.contains("innerHTML"));
    }

    #[test]
    fn what_the_demo_turns_off() {
        let post = Method::POST;
        for p in [
            "/admin/settings",
            "/admin/settings/ap/install",
            "/admin/settings/ap/check",
            "/admin/settings/backups/now",
            "/admin/settings/export",
            "/admin/catalog/add",
            "/admin/catalog/approval",
            "/admin/common-games/new",
            "/admin/common-games/vanilla",
            "/admin/common-games/vanilla/apworld",
            "/admin/common-games/vanilla/download",
            "/admin/worlds/approve",
            "/admin/worlds/to-catalog",
            "/signup/apworld",
            "/game/ab12/poptracker",
            "/admin/users/role",
            "/admin/users/disable",
            "/admin/links/invite",
            "/admin/links/reset",
            "/admin/links/community",
        ] {
            assert!(locked(&post, p), "{p}");
        }
        assert!(locked(
            &Method::GET,
            "/admin/settings/backups/download?name=x"
        ));
        for p in [
            "/admin/settings",
            "/admin/catalog",
            "/admin/common-games",
            "/admin/users",
        ] {
            assert!(!locked(&Method::GET, p), "GET {p}");
        }
        for p in [
            "/admin/generate",
            "/admin/signups",
            "/admin/common",
            "/admin/season/delete",
            "/admin/worlds/reject",
            "/admin/settingsx",
            "/signup",
            "/game/ab12/poptracker/remove",
            "/hint",
            "/minesweeper/new",
            "/logout",
        ] {
            assert!(!locked(&post, p), "{p}");
        }
    }
}
