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
/// The biggest form read for the log (others are noted, not read).
const BODY_MAX: usize = 256 * 1024;

/// Whether `path` (season prefix stripped) is administration to log, as far
/// as the path tells.
fn logged(path: &str) -> bool {
    matches!(crate::auth::tier(path), Tier::Admin | Tier::Sysadmin)
        // An admin assigning a slot to someone.
        || path == "/claim"
}

/// A field whose value isn't kept: passwords, tokens, secrets.
fn secret(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["pass", "pw", "token", "secret"]
        .iter()
        .any(|s| k.contains(s))
}

/// A form's fields as `key=value, …`, secrets redacted and values cut short.
pub fn summarize(body: &[u8]) -> String {
    let fields: Vec<(String, String)> = serde_urlencoded::from_bytes(body).unwrap_or_default();
    fields
        .into_iter()
        .filter(|(k, _)| k != "back")
        .map(|(k, v)| {
            let v = if secret(&k) {
                "(hidden)".to_string()
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
        .join(", ")
}

pub async fn middleware(State(st): State<AppState>, req: Request, next: Next) -> Response {
    if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return next.run(req).await;
    }
    let path = req.uri().path().to_string();
    if !logged(&path) {
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
            // Too big to read for the log: the handler would refuse it too.
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
    let res = next.run(req).await;
    let (sid, sname) = season
        .map(|s| (Some(s.id), s.name))
        .unwrap_or((None, String::new()));
    if let Err(e) = st
        .db
        .log_mod_action(
            crate::clock::now(),
            Some(who.account_id).filter(|id| *id != 0),
            &who.username,
            sid,
            &sname,
            &path,
            &detail,
            res.status().as_u16(),
        )
        .await
    {
        tracing::warn!("moderator log: {e}");
    }
    res
}

/// What an action is, for people: the route's own name where there's one.
pub fn label(action: &str) -> &str {
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
    }
}
