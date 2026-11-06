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

/// At most this many seasons, this many games in a season, and this many
/// seasons live (or generating) at once: each live season runs a game server
/// of a couple of hundred MB.
pub const MAX_SEASONS: usize = 12;
pub const MAX_GAMES: i64 = 12;
pub const MAX_LIVE: usize = 4;

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
    // An external server is shared, and outside what the demo can keep to
    // localhost; season consoles would reach it too.
    assert!(
        crate::config::get().ap_managed,
        "a demo build only runs with managed game servers: unset BIFROST_AP_MODE"
    );
    // Rate limits are per address in a demo build. Behind a proxy that Bifrost
    // doesn't trust, every visitor has the proxy's address and one shared limit.
    assert!(
        !crate::config::get().trusted_proxies.is_empty()
            || std::env::var("BIFROST_DEMO_DIRECT").as_deref() == Ok("1"),
        "a demo build needs BIFROST_TRUSTED_PROXIES set to its reverse proxy, so rate limits \
         see each visitor's address (or BIFROST_DEMO_DIRECT=1 if visitors reach it directly)"
    );
    reset_interval();
    // Started from a snapshot: its seasons are the demo's own.
    if std::env::var_os("BIFROST_DEMO_SNAPSHOT").is_some() {
        let ids: std::collections::HashSet<_> =
            db.seasons().await.into_iter().map(|s| s.id).collect();
        let mut subs = std::collections::HashSet::new();
        for id in &ids {
            subs.extend(
                db.submissions(*id)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| s.id),
            );
        }
        let _ = PROTECTED.set(ids);
        let _ = SEEDED.set(subs);
    }
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

/// `POST /demo/as/<role>`: sign in as that role's demo account and go back.
/// A POST, so the cross-site check applies and a link elsewhere can't switch
/// a visitor's account; it ends the session it replaces, and it's rate
/// limited per address (`ratelimit`).
pub(crate) async fn switch(
    State(st): State<AppState>,
    UrlPath(role): UrlPath<String>,
    Query(q): Query<Rd>,
    headers: HeaderMap,
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
    if let Some(t) = st.auth.session_token(&headers) {
        st.db.end_session(&auth::token_hash(&t)).await;
    }
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
    let protected: Vec<String> = PROTECTED
        .get()
        .map(|p| p.iter().map(|id| id.to_string()).collect())
        .unwrap_or_default();
    Json(serde_json::json!({ "reset_in": reset_in, "protected": protected }))
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
    // (Imports are below: a season bundle is an upload of up to 2 GiB that is
    // kept and served back as the season's download.)
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
        // Season bundles: any file, kept and served from the demo.
        || path == "/admin/seasons/import"
        // Accounts: roles, disabling, invite and reset links.
        || path.starts_with("/admin/users/")
        || path.starts_with("/admin/links/")
    )
}

/// The seasons the demo starts with (those in the snapshot). Visitors can look
/// at them, administration included, and play in them (hints, minesweeper,
/// signups, their notes), but not change them: one visitor mustn't be able to
/// delete, regenerate, finish or empty the demo's own seasons for everyone
/// until the reset. Everything works on a season a visitor makes.
static PROTECTED: OnceLock<std::collections::HashSet<crate::id::Id>> = OnceLock::new();

/// The signups in the demo's own seasons when it started. Visitors can add
/// signups to them, and withdraw, replace or annotate those, but not touch
/// these (see `web::signup`).
static SEEDED: OnceLock<std::collections::HashSet<crate::id::Id>> = OnceLock::new();

/// Was this signup in the demo's snapshot?
pub fn is_seeded(submission: &crate::id::Id) -> bool {
    SEEDED.get().is_some_and(|s| s.contains(submission))
}

/// The refusal for a change to a seeded signup.
pub const SEEDED_OFF: &str = "The demo's own signups stay as they are. You can withdraw, replace or annotate a signup you made.";

/// Is this one of the demo's own seasons?
pub fn is_protected(id: &crate::id::Id) -> bool {
    PROTECTED.get().is_some_and(|p| p.contains(id))
}

/// The season a request acts on: its `/s/<id>/`, or for a season page asked
/// for without one, the default season (as `scope::SeasonCtx` picks it).
async fn season_of(
    st: &AppState,
    selected: Option<crate::id::Id>,
    path: &str,
) -> Option<crate::id::Id> {
    if selected.is_some() {
        return selected;
    }
    let global = crate::scope::is_instance(path) || path == "/" || path.starts_with("/demo/");
    if global {
        return None;
    }
    st.db.default_season().await.map(|s| s.id)
}

/// What visitors can still do in the demo's own seasons: play, sign up, and
/// look things up in the console (its commands are checked in
/// `admin_console`, see [`console_ok`]).
fn allowed_on_own(path: &str) -> bool {
    matches!(
        path,
        "/hint" | "/prioritize" | "/signup" | "/signup/check" | "/signup/delete" | "/admin/console"
    ) || path.starts_with("/minesweeper/")
        || path.starts_with("/seat/")
        || path
            .strip_prefix("/common/")
            .is_some_and(|r| r.ends_with("/ident") && r.matches('/').count() == 1)
        || path.strip_prefix("/signup/game/").is_some_and(|r| {
            (r.ends_with("/notes") || r.ends_with("/yaml")) && r.matches('/').count() == 1
        })
}

/// Console commands that only read, the ones the demo's own seasons take.
pub fn console_ok(cmd: &str) -> bool {
    let first = cmd
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        first.as_str(),
        "/help" | "/players" | "/status" | "/license"
    )
}

/// Refuses what [`locked`] says, and changes to the demo's own seasons,
/// whoever asks.
pub async fn guard(State(st): State<AppState>, mut req: Request, next: Next) -> Response {
    let changes = req.method() != Method::GET && req.method() != Method::HEAD;
    let mut own = None;
    if changes {
        let (selected, path) = (crate::scope::selected(&req), req.uri().path().to_string());
        let season = season_of(&st, selected, &path).await;
        // Pin it, so the handler acts on the season checked here, not on a
        // default season that has changed since.
        if let (None, Some(id)) = (selected, season) {
            crate::scope::select(&mut req, id);
        }
        own = season.filter(|id| is_protected(id) && !allowed_on_own(&path));
    }
    let mut r = if let Some(id) = own {
        (
            StatusCode::FORBIDDEN,
            render::notice(
                "The demo's own seasons stay as they are, so every visitor sees them. Make a season of your own to try this on.",
                false,
                &format!("/s/{id}/admin"),
            ),
        )
            .into_response()
    } else if !locked(req.method(), req.uri().path()) {
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

/// Held from the live-season count to the season's move to Generating, so
/// generate requests sent together can't all pass the cap.
pub static GO_LIVE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The cap on live seasons, for generating one (under [`GO_LIVE`]).
pub async fn live_full(db: &Db) -> Option<String> {
    use crate::db::State;
    let live = db
        .seasons()
        .await
        .iter()
        .filter(|s| matches!(s.state(), State::Live | State::Generating))
        .count();
    (live >= MAX_LIVE).then(|| {
        format!(
            "The demo runs at most {MAX_LIVE} live seasons at once: archive or delete one first, or wait for the reset."
        )
    })
}

/// The cap on a season's games, for a signup.
pub async fn games_full(db: &Db, season: crate::id::Id) -> Option<String> {
    (db.games_in(season).await >= MAX_GAMES)
        .then(|| format!("A demo season holds at most {MAX_GAMES} games."))
}

/// Where game servers listen in a demo build, whatever `BIFROST_AP_BIND` says.
pub const AP_BIND: &str = "127.0.0.1";

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
    let Some(after) = reset_interval() else {
        return std::future::pending().await;
    };
    let _ = RESET_AT.set(Instant::now() + after);
    tracing::info!(
        "demo: resetting in {} minute(s)",
        after.as_secs_f64() / 60.0
    );
    tokio::time::sleep(after).await;
    tracing::info!("demo: time for the reset");
    // A graceful shutdown waits for every open request, so one that trickles
    // in forever would keep the demo from resetting. The snapshot is restored
    // on the next start anyway, so after a grace period, stop hard.
    tokio::spawn(async {
        tokio::time::sleep(RESET_GRACE).await;
        tracing::warn!("demo: still not stopped; stopping now");
        for pid in crate::apserver::server_pids().into_values() {
            // SAFETY: signalling a game server this process started.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
        std::process::exit(0);
    });
}

/// `BIFROST_DEMO_RESET_MINUTES`: unset means no reset; otherwise it must be a
/// number of minutes above 0 and at most a week, or Bifrost won't start (a
/// typo mustn't quietly turn the reset off).
fn reset_interval() -> Option<Duration> {
    let v = std::env::var("BIFROST_DEMO_RESET_MINUTES").ok()?;
    let after = v
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|m| *m > 0.0 && *m <= 7.0 * 24.0 * 60.0)
        .and_then(|m| Duration::try_from_secs_f64(m * 60.0).ok());
    Some(after.unwrap_or_else(|| {
        panic!(
            "BIFROST_DEMO_RESET_MINUTES={v:?}: give a number of minutes, above 0 and at most a week"
        )
    }))
}

/// How long a reset waits for open requests and game servers.
const RESET_GRACE: Duration = Duration::from_secs(30);

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
    fn what_visitors_can_do_in_the_demos_own_seasons() {
        for p in [
            "/hint",
            "/prioritize",
            "/signup",
            "/signup/check",
            "/admin/console",
            "/minesweeper/new",
            "/minesweeper/reveal",
            "/seat/abc",
            "/common/minecraft/ident",
            "/signup/game/ab12/notes",
            "/signup/game/ab12/yaml",
            "/signup/delete",
        ] {
            assert!(allowed_on_own(p), "{p}");
        }
        for p in [
            "/admin/season/delete",
            "/admin/regenerate",
            "/admin/archive-season",
            "/admin/clear",
            "/admin/signups",
            "/game/ab12/finish",
            "/game/ab12/poptracker/remove",
            "/game/ab12/invite",
            "/claim",
            "/signup/game/ab12/seat/remove",
            "/common/minecraft/file/0",
        ] {
            assert!(!allowed_on_own(p), "{p}");
        }
        for c in ["/players", "/status", "/HELP", "/license"] {
            assert!(console_ok(c), "{c}");
        }
        for c in [
            "/release Ann",
            "/collect Ann",
            "/send Ann x",
            "/exit",
            "/option x y",
            "hello",
        ] {
            assert!(!console_ok(c), "{c}");
        }
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
            "/admin/seasons/import",
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
            "/admin/seasons",
            "/admin/bundle",
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
