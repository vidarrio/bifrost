//! The moderator log: every state-changing request a moderator or sysadmin
//! makes to season or instance administration (console commands included) is
//! recorded, for sysadmins to look over at `/admin/audit`.
//!
//! It's a middleware, so a new administration route is logged without anyone
//! remembering to: who, which season, the route (the action), the form's
//! fields with anything secret left out, and how it went (the status).

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, header};
use axum::middleware::Next;
use axum::response::Response;

use crate::AppState;
use crate::auth::{Me, Role, Tier};

/// How long rows are kept: a year.
const KEEP_SECS: i64 = 365 * 86_400;
/// The most of a form's field value kept.
const VALUE_MAX: usize = 200;
/// The most of a summary kept.
const DETAIL_MAX: usize = 2000;
/// The biggest form read: the router's own body limit, so nothing a handler
/// would take is cut short by the log.
const BODY_MAX: usize = 2 * 1024 * 1024;
/// The biggest reply looked at to tell a refusal (an error page or
/// `{"ok": false}`) from a success.
const REPLY_MAX: usize = 1024 * 1024;

/// Whether `path` (season prefix stripped) is administration to log.
fn logged(path: &str) -> bool {
    matches!(crate::auth::tier(path), Tier::Admin | Tier::Sysadmin)
        // An admin assigning a slot to someone.
        || path == "/claim"
}

/// A player route where a moderator can act on other people's things:
/// finishing a slot, its PopTracker pack, prioritizing as admin, replacing,
/// editing or removing someone's signup or seats. Logged for moderators and
/// up (their own things included: the route alone can't tell).
fn power_route(path: &str) -> bool {
    if path == "/prioritize" || path == "/signup/delete" {
        return true;
    }
    let seg: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match seg.as_slice() {
        ["game", _, "finish"] | ["game", _, "poptracker"] | ["game", _, "poptracker", "remove"] => {
            true
        }
        ["signup", "game", _, rest @ ..] => matches!(
            rest,
            ["yaml"] | ["notes"] | ["seat", "remove"] | ["seat", "rename"]
        ),
        _ => false,
    }
}

/// A field whose value isn't kept: passwords, tokens, secrets.
fn secret(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["pass", "token", "secret"].iter().any(|s| k.contains(s)) || k == "pw" || k.ends_with("_pw")
}

/// A console command with a password in it (`/option password X`,
/// `/option server_password X`, `/password X`): the password hidden.
fn console_redacted(cmd: &str) -> Option<String> {
    let mut words = cmd.split_whitespace();
    let first = words.next()?.to_ascii_lowercase();
    if first == "/password" {
        return Some("/password (hidden)".into());
    }
    if first == "/option" {
        let name = words.next()?;
        if secret(name) {
            return Some(format!("/option {name} (hidden)"));
        }
    }
    None
}

/// A form's fields as `key=value, …`, secrets redacted and values cut short.
pub fn summarize(body: &[u8]) -> String {
    let fields: Vec<(String, String)> = serde_urlencoded::from_bytes(body).unwrap_or_default();
    let mut out = fields
        .into_iter()
        .filter(|(k, _)| k != "back")
        .map(|(k, v)| {
            let v = if secret(&k) {
                "(hidden)".to_string()
            } else if let Some(r) = (k == "cmd").then(|| console_redacted(&v)).flatten() {
                r
            } else {
                let one_line = v.replace(['\r', '\n'], " ");
                let mut cut: String = one_line.chars().take(VALUE_MAX).collect();
                if one_line.chars().count() > VALUE_MAX {
                    cut.push('…');
                }
                cut
            };
            format!("{k}={v}")
        })
        .collect::<Vec<_>>()
        .join(", ");
    if out.chars().count() > DETAIL_MAX {
        out = out.chars().take(DETAIL_MAX).collect::<String>() + "…";
    }
    out
}

/// Whether a reply says no: an error banner on a page, or `"ok": false`.
fn refused(content_type: &str, body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body);
    if content_type.starts_with("application/json") {
        return serde_json::from_str::<serde_json::Value>(&text).is_ok_and(|v| {
            v.get("ok") == Some(&serde_json::Value::Bool(false)) || v.get("error").is_some()
        });
    }
    content_type.starts_with("text/html") && text.contains(r#"class="banner bad""#)
}

pub async fn middleware(State(st): State<AppState>, req: Request, next: Next) -> Response {
    if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return next.run(req).await;
    }
    let path = req.uri().path().to_string();
    let admin = logged(&path);
    if !admin && !power_route(&path) {
        return next.run(req).await;
    }
    let Some(who) = req
        .extensions()
        .get::<Me>()
        .and_then(|m| m.principal())
        .cloned()
    else {
        return next.run(req).await;
    };
    if who.role < Role::Moderator {
        return next.run(req).await;
    }
    // The season: a `/s/{id}/` URL's, else the default one for a season page.
    let season = match crate::scope::selected(&req) {
        Some(id) => st.db.season(id).await,
        None if !crate::scope::is_instance(&path) => st.db.default_season().await,
        None => None,
    };
    let kind = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (req, detail) = if kind.starts_with("application/x-www-form-urlencoded") {
        let (parts, body) = req.into_parts();
        match axum::body::to_bytes(body, BODY_MAX).await {
            Ok(bytes) => {
                let detail = summarize(&bytes);
                (Request::from_parts(parts, Body::from(bytes)), detail)
            }
            // Over the router's own limit: the handler would refuse it too.
            Err(_) => (
                Request::from_parts(parts, Body::empty()),
                "(a form too big to read)".to_string(),
            ),
        }
    } else if kind.starts_with("multipart/") {
        (req, "(a file upload)".to_string())
    } else {
        (req, String::new())
    };
    // Only an admin's assigning, of what `/claim` does.
    if path == "/claim" && !detail.split(", ").any(|f| f == "op=assign") {
        return next.run(req).await;
    }
    // Written first: a request that's dropped midway (a client that goes),
    // or that deletes its own season, is still in the log.
    let (sid, sname) = season
        .map(|s| (Some(s.id), s.name))
        .unwrap_or((None, String::new()));
    let row = st
        .db
        .log_mod_action(
            crate::clock::now(),
            Some(who.account_id).filter(|id| *id != 0),
            &who.username,
            sid,
            &sname,
            &path,
            &detail,
        )
        .await;
    let row = match row {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!("moderator log: {e}");
            return next.run(req).await;
        }
    };
    let res = next.run(req).await;
    let status = res.status();
    let ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    // Pages and JSON replies are small: read them to tell a refusal.
    let (res, outcome) = if !status.is_success() && !status.is_redirection() {
        (res, "refused")
    } else if status.is_success()
        && (ct.starts_with("text/html") || ct.starts_with("application/json"))
    {
        let (parts, body) = res.into_parts();
        match axum::body::to_bytes(body, REPLY_MAX).await {
            Ok(bytes) => {
                let outcome = if refused(&ct, &bytes) {
                    "refused"
                } else {
                    "ok"
                };
                (Response::from_parts(parts, Body::from(bytes)), outcome)
            }
            Err(_) => (Response::from_parts(parts, Body::empty()), "ok"),
        }
    } else {
        (res, "ok")
    };
    st.db.finish_mod_action(row, status.as_u16(), outcome).await;
    res
}

/// What an action is, for people: the route's own name where there's one.
pub fn label(action: &str) -> &str {
    // (A route with an id in it, `/game/<id>/finish`, is grouped by its shape.)
    match action {
        "/admin/console" => "console command",
        "/admin/generate" => "generate",
        "/admin/regenerate" => "re-roll",
        "/admin/signups" => "open/close signups",
        "/admin/clear" => "clear",
        "/admin/delete-player" => "remove a signup",
        "/admin/delete-apworld" => "delete an apworld",
        "/admin/worlds/approve" => "approve an apworld",
        "/admin/worlds/reject" => "reject an apworld",
        "/admin/worlds/to-catalog" => "add to catalog",
        "/admin/worlds/update" => "update to the catalog's version",
        "/admin/mods/add" => "add a moderator",
        "/admin/mods/remove" => "remove a moderator",
        "/claim" => "assign a slot",
        "/admin/users/role" => "change a role",
        "/admin/users/disable" => "disable an account",
        "/admin/links/invite" => "make an invite",
        "/admin/seasons" => "create a season",
        "/admin/seasons/import" => "import a season",
        "/admin/season/delete" => "delete a season",
        "/admin/archive-season" => "archive a season",
        "/admin/cancel" => "cancel generation",
        "/admin/connect-password" => "change the connect password",
        "/admin/minesweeper" => "minesweeper on/off",
        "/admin/minesweeper/hints" => "minesweeper hint cap",
        "/admin/signups/limit" => "games per player",
        "/admin/privacy" => "season privacy",
        "/admin/common" => "season common games",
        "/admin/settings" => "instance settings",
        "/admin/settings/ap/check" => "check Archipelago releases",
        "/admin/settings/ap/install" => "install Archipelago",
        "/admin/settings/ap/use" => "switch Archipelago version",
        "/admin/settings/ap/remove" => "remove Archipelago version",
        "/admin/settings/backups/now" => "back up now",
        "/admin/settings/backups/delete" => "delete a backup",
        "/admin/settings/export" => "export token",
        "/admin/catalog/add" => "add to the catalog",
        "/admin/catalog/remove" => "remove from the catalog",
        "/admin/catalog/notes" => "catalog notes",
        "/admin/catalog/approval" => "approval mode",
        "/admin/catalog/offer/promote" => "promote a catalog offer",
        "/admin/catalog/offer/dismiss" => "dismiss a catalog offer",
        "/admin/links/reset" => "make a reset link",
        "/admin/links/community" => "community link",
        "/admin/links/revoke" => "revoke a link",
        "/prioritize" => "prioritize a hint (as admin)",
        "/signup/delete" => "remove a signup (player route)",
        a if a.starts_with("/admin/common-games") => "common games setup",
        a if a.starts_with("/game/") && a.ends_with("/finish") => "finish a slot",
        a if a.starts_with("/game/") && a.contains("/poptracker") => "PopTracker pack",
        a if a.starts_with("/signup/game/") && a.ends_with("/yaml") => "replace a signup's YAML",
        a if a.starts_with("/signup/game/") && a.ends_with("/notes") => "edit a signup's notes",
        a if a.starts_with("/signup/game/") && a.contains("/seat/") => "change a signup's seats",
        _ => action,
    }
}

/// Prune rows older than a year, now and then daily.
pub fn spawn_pruner(db: crate::db::Db) {
    tokio::spawn(async move {
        loop {
            let n = db.prune_mod_actions(crate::clock::now() - KEEP_SECS).await;
            if n > 0 {
                tracing::info!("moderator log: removed {n} rows older than a year");
            }
            tokio::time::sleep(std::time::Duration::from_secs(86_400)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_are_summed_up_without_secrets() {
        assert_eq!(
            summarize(b"cmd=%2Fsend+Ann+Map+Width&back=%2Fadmin"),
            "cmd=/send Ann Map Width"
        );
        assert_eq!(
            summarize(b"password=hunter2&connect_pw=x&token=t&name=foo.apworld"),
            "password=(hidden), connect_pw=(hidden), token=(hidden), name=foo.apworld"
        );
        let long = format!("note={}", "a".repeat(500));
        let s = summarize(long.as_bytes());
        assert!(s.chars().count() < 220 && s.ends_with('…'), "{s}");
        assert_eq!(summarize(b"a=line%0Abreak"), "a=line break");
        assert_eq!(
            summarize(b"cmd=%2Foption+server_password+hunter2"),
            "cmd=/option server_password (hidden)"
        );
        assert_eq!(
            summarize(b"cmd=%2Fpassword+hunter2"),
            "cmd=/password (hidden)"
        );
        assert_eq!(
            summarize(b"cmd=%2Foption+hint_cost+5"),
            "cmd=/option hint_cost 5"
        );
        assert_eq!(
            summarize(b"apworld=x.apworld"),
            "apworld=x.apworld",
            "not a secret for containing pw"
        );
        assert_eq!(summarize(b""), "");
    }

    #[test]
    fn administration_is_logged_and_play_isnt() {
        for p in [
            "/admin/console",
            "/admin/generate",
            "/admin/users/role",
            "/admin/catalog/add",
            "/claim",
        ] {
            assert!(logged(p), "{p}");
        }
        for p in [
            "/signup",
            "/hint",
            "/prioritize",
            "/login",
            "/api/notifications/read",
        ] {
            assert!(!logged(p), "{p}");
        }
        for p in [
            "/prioritize",
            "/signup/delete",
            "/game/ab/finish",
            "/game/ab/poptracker",
            "/game/ab/poptracker/remove",
            "/signup/game/ab/yaml",
            "/signup/game/ab/notes",
            "/signup/game/ab/seat/remove",
        ] {
            assert!(power_route(p), "{p}");
        }
        for p in [
            "/signup",
            "/hint",
            "/signup/game/ab/invite",
            "/game/ab/invite",
            "/minesweeper/new",
        ] {
            assert!(!power_route(p), "{p}");
        }
    }

    #[test]
    fn refusals_are_told_from_successes() {
        assert!(refused(
            "text/html; charset=utf-8",
            br#"<div class="banner bad">Admins only.</div>"#
        ));
        assert!(!refused(
            "text/html",
            br#"<div class="banner ok">Saved.</div>"#
        ));
        assert!(refused(
            "application/json",
            br#"{"ok": false, "error": "x"}"#
        ));
        assert!(!refused("application/json", br#"{"ok": true}"#));
    }
}
