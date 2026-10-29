//! The HTTP handlers, one file per area of the site. `main` builds the
//! router from them; helpers shared between areas live here.

use crate::*;

use axum::{
    Json, Router,
    extract::{Form, Multipart, Path, Query, State},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

mod board;
mod catalog;
mod game;
mod logs;
mod minecraft;
mod minesweeper;
mod play;
mod public;
mod season_admin;
mod seasons;
mod settings;
mod signup;

pub(crate) use board::*;
pub(crate) use catalog::*;
pub(crate) use game::*;
pub(crate) use logs::*;
pub(crate) use minecraft::*;
pub(crate) use minesweeper::*;
pub(crate) use play::*;
pub(crate) use public::*;
pub(crate) use season_admin::*;
pub(crate) use seasons::*;
pub(crate) use settings::*;
pub(crate) use signup::*;

pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Send a file as an attachment named `name` (ranges, conditional requests).
pub(crate) async fn download(
    path: std::path::PathBuf,
    name: &str,
    req: axum::extract::Request,
) -> Response {
    let disposition = format!("attachment; filename=\"{name}\"");
    match tower::ServiceExt::oneshot(tower_http::services::ServeFile::new(path), req).await {
        Ok(mut r) => {
            if let Ok(v) = axum::http::HeaderValue::from_str(&disposition) {
                r.headers_mut()
                    .insert(axum::http::header::CONTENT_DISPOSITION, v);
            }
            r.map(axum::body::Body::new).into_response()
        }
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Build an attachment download for a seed member's bytes. The filename sent to
/// the browser is the bare member name, sanitised for the header.
pub(crate) fn download_response(member: &str, data: Vec<u8>) -> Response {
    let fname: String = member
        .rsplit('/')
        .next()
        .unwrap_or(member)
        .chars()
        .map(|c| if "\"\\\r\n".contains(c) { '_' } else { c })
        .collect();
    let mut resp = Response::new(axum::body::Body::from(data));
    let h = resp.headers_mut();
    h.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(
        axum::http::header::CONTENT_DISPOSITION,
        axum::http::HeaderValue::from_str(&format!("attachment; filename=\"{fname}\""))
            .unwrap_or(axum::http::HeaderValue::from_static("attachment")),
    );
    resp
}

/// Every route, behind the auth and rate-limit middleware and the baseline
/// response headers. `main` wraps it in the season-URL rewrite.
pub(crate) fn router(state: AppState, auth: auth::Auth, trusted_proxies: Vec<net::Cidr>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/", get(landing))
        .route("/favicon.svg", get(favicon))
        .route("/check", get(check_form).post(check_run))
        .route("/board", get(board_view))
        .route("/board/fragment", get(board_fragment_view))
        .route("/log", get(log_view))
        .route("/log/fragment", get(log_fragment_view))
        .route("/game/{ap_slot}", get(game_view))
        .route(
            "/game/{ap_slot}/poptracker",
            post(pt_upload).layer(upload_limit()),
        )
        .route("/game/{ap_slot}/poptracker/remove", post(pt_remove))
        .route("/game/{ap_slot}/log/fragment", get(game_log_fragment))
        .route("/game/{ap_slot}/poptracker/state", get(pt_state))
        .route("/game/{ap_slot}/poptracker/asset", get(pt_asset))
        .route("/game/{ap_slot}/hints", get(game_hints))
        .route("/game/{ap_slot}/items", get(game_items))
        .route("/game/{ap_slot}/output", get(game_output))
        .route("/game/mc/{which}", get(mc_asset))
        .route("/game/{ap_slot}/finish", post(game_finish))
        .route("/claim", get(claim_get).post(claim_post))
        .route("/mc/username", post(mc_username_post))
        .route("/hint", post(hint_post))
        .route("/prioritize", post(prioritize_post))
        .route("/signup", get(signup_get).post(signup_post))
        .route(
            "/signup/apworld",
            post(signup_apworld).layer(upload_limit()),
        )
        .route("/signup/check", post(signup_check))
        .route("/signup/delete", post(signup_delete))
        .route("/admin", get(admin_view))
        .route("/admin/seasons", get(seasons_view).post(seasons_create))
        .route(
            "/admin/seasons/import",
            post(seasons_import).layer(axum::extract::DefaultBodyLimit::max(2 << 30)),
        )
        .route("/admin/bundle", get(admin_bundle))
        .route("/admin/settings", get(settings_view).post(settings_save))
        .route("/admin/settings/ap/check", post(ap_check))
        .route("/admin/settings/ap/install", post(ap_install))
        .route("/admin/settings/ap/use", post(ap_use))
        .route("/admin/settings/ap/remove", post(ap_remove))
        .route("/admin/settings/load", get(settings_load))
        .route("/admin/settings/backups/now", post(backup_now))
        .route("/admin/settings/backups/download", get(backup_download))
        .route("/admin/settings/backups/delete", post(backup_delete))
        .route("/admin/mods/add", post(mods_add))
        .route("/admin/mods/remove", post(mods_remove))
        .route("/admin/signups", post(admin_signups))
        .route("/admin/generate", post(admin_generate))
        .route("/admin/regenerate", post(admin_regenerate))
        .route("/admin/clear", post(admin_clear))
        .route("/admin/delete-player", post(admin_delete_player))
        .route("/admin/delete-apworld", post(admin_delete_apworld))
        .route("/admin/worlds/approve", post(admin_world_approve))
        .route("/admin/worlds/reject", post(admin_world_reject))
        .route("/admin/worlds/to-catalog", post(admin_world_to_catalog))
        .route("/admin/catalog", get(catalog_view))
        .route(
            "/admin/catalog/add",
            post(catalog_upload).layer(upload_limit()),
        )
        .route("/admin/catalog/remove", post(catalog_remove))
        .route("/admin/catalog/approval", post(catalog_approval))
        .route("/admin/console", post(admin_console))
        .route("/admin/connect-password", post(admin_connect_password))
        .route("/admin/minesweeper", post(admin_minesweeper))
        .route("/admin/minesweeper/hints", post(admin_mine_hints))
        .route("/admin/mc", post(admin_mc))
        .route("/admin/output", get(admin_output))
        .route("/admin/archive-season", post(admin_archive_season))
        .route("/season", get(season_view))
        .route("/minesweeper", get(minesweeper_page))
        .route("/minesweeper/new", post(mine_new))
        .route("/minesweeper/reveal", post(mine_reveal))
        .route("/minesweeper/state", get(mine_state))
        .route("/minesweeper/pause", post(mine_pause))
        .route("/minesweeper/leaderboard", get(mine_leaderboard))
        .route("/api/me", get(api_me))
        .merge(account::routes())
        // Rate-limit auth + expensive endpoints (inside auth: knows the account).
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(ratelimit::Limiter::new(trusted_proxies)),
            ratelimit::middleware,
        ))
        // Authenticate + enforce the route tiers before any handler runs.
        .layer(axum::middleware::from_fn_with_state(auth, auth::middleware))
        // Small bodies everywhere (YAMLs are capped at 512 KiB); only the two
        // file-upload routes get `upload_limit`.
        .layer(axum::extract::DefaultBodyLimit::max(2 * 1024 * 1024))
        // Dynamic HTML/JSON must never be cached (or bfcache'd) by the browser:
        // a stale page would keep running old inline JS. `no-store` also disables
        // the back/forward cache. Applied only where a handler didn't already set
        // Cache-Control, so pack assets keep their explicit immutable/no-cache.
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        ))
        // Baseline security headers (previously left to the reverse proxy).
        // No strict CSP: pages rely on inline scripts; framing is same-origin.
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::X_CONTENT_TYPE_OPTIONS,
            axum::http::HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::X_FRAME_OPTIONS,
            axum::http::HeaderValue::from_static("SAMEORIGIN"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::CONTENT_SECURITY_POLICY,
            axum::http::HeaderValue::from_static(
                "frame-ancestors 'self'; base-uri 'self'; object-src 'none'",
            ),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::REFERRER_POLICY,
            axum::http::HeaderValue::from_static("same-origin"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::HeaderName::from_static("permissions-policy"),
            axum::http::HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Body limit for the file-upload routes (apworlds, PopTracker packs).
pub(crate) fn upload_limit() -> axum::extract::DefaultBodyLimit {
    axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024)
}
