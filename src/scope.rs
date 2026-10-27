//! Season-scoped URLs.
//!
//! Season pages live under `/s/{id}/…`. [`rewrite`] wraps the whole router: it
//! strips the prefix *before routing* (so routes, auth tiers and rate-limit
//! rules match plain paths like `/board`) and remembers the selection. Handlers
//! take a [`SeasonCtx`], which resolves the selected season — or, for an
//! unscoped URL, the default one — once.
//!
//! Links: every page carries `<base href="/s/{id}/">` for the season it was
//! rendered for, and season links are base-relative (`board`, `game/3`), so a
//! page keeps pointing at *its* season even if another becomes the default
//! meanwhile. Instance-level paths (`/`, `/login`, `/admin/users`, `/api/…`)
//! stay absolute. Server-side redirects use [`SeasonCtx::url`].

use std::cell::RefCell;
use std::sync::Arc;

use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::AppState;
use crate::db::{SeasonRow, State};
use crate::runtime::SeasonRt;

tokio::task_local! {
    /// The link base for the page being rendered ("" = instance level).
    static BASE: RefCell<String>;
}

/// The current request's link base, e.g. `/s/3` ("" outside a season).
pub fn link_base() -> String {
    BASE.try_with(|b| b.borrow().clone()).unwrap_or_default()
}

/// The season a `/s/{id}/…` URL selected.
#[derive(Clone, Copy)]
struct Selected(i64);

/// Instance-level pages: not about a season, so not reachable under `/s/{id}/`
/// (one URL each — a forward-auth proxy gating them by prefix sees them all).
const INSTANCE_ROUTES: &[&str] = &[
    "/admin/seasons",
    "/admin/settings",
    "/admin/catalog",
    "/admin/users",
    "/admin/links",
    "/api",
    "/login",
    "/logout",
    "/setup",
    "/invite",
    "/reset",
    "/health",
    "/favicon.svg",
];

/// Split `/s/{id}/rest` into `(id, "/rest")`; `/s/{id}` alone is its board.
/// `None` for an instance-level page under `/s/` (it then 404s).
fn split(path: &str) -> Option<(i64, String)> {
    let rest = path.strip_prefix("/s/")?;
    let (id, tail) = rest.split_once('/').unwrap_or((rest, ""));
    let id = id.parse::<i64>().ok().filter(|&i| i > 0)?;
    let tail = format!("/{}", if tail.is_empty() { "board" } else { tail });
    let instance = INSTANCE_ROUTES
        .iter()
        .any(|r| tail.strip_prefix(r).is_some_and(|after| after.is_empty() || after.starts_with('/')));
    (!instance).then_some((id, tail))
}

pub async fn rewrite(mut req: Request, next: Next) -> Response {
    // Keep the URL as requested (e.g. for the post-login redirect).
    let original = axum::extract::OriginalUri(req.uri().clone());
    req.extensions_mut().insert(original);
    if let Some((id, path)) = split(req.uri().path()) {
        let pq = match req.uri().query() {
            Some(q) => format!("{path}?{q}"),
            None => path,
        };
        let mut parts = req.uri().clone().into_parts();
        parts.path_and_query = pq.parse().ok();
        if let Ok(uri) = Uri::from_parts(parts) {
            *req.uri_mut() = uri;
            req.extensions_mut().insert(Selected(id));
        }
    }
    BASE.scope(RefCell::new(String::new()), next.run(req)).await
}

/// The season a request is about.
#[derive(Clone)]
pub struct SeasonCtx {
    pub id: i64,
    pub row: SeasonRow,
    pub rt: Arc<SeasonRt>,
    /// Who runs this season besides sysadmins (creator + co-moderators).
    pub mods: Vec<i64>,
}

impl SeasonCtx {
    /// An absolute URL inside this season (`path` starts with `/`).
    pub fn url(&self, path: &str) -> String {
        format!("/s/{}{path}", self.id)
    }
    /// A post-form redirect: the form's local `back` path (else `default`),
    /// inside this season.
    pub fn back(&self, back: Option<&str>, default: &str) -> String {
        match back {
            Some(p) if p.starts_with('/') && !p.starts_with("//") && !p.contains('\\') => {
                self.url(p)
            }
            _ => self.url(default),
        }
    }
    /// May `me` run this season?
    pub fn can_manage(&self, me: &crate::auth::Me) -> bool {
        me.can(crate::auth::Action::ManageSeason { mods: &self.mods })
    }
    /// `(username, may run this season)` — the pair season handlers branch on.
    pub fn ident(&self, me: &crate::auth::Me) -> (String, bool) {
        (me.username(), self.can_manage(me))
    }
    pub fn state(&self) -> State {
        self.row.state()
    }
    pub fn dirs(&self) -> crate::config::SeasonDirs {
        crate::config::get().season(self.id)
    }
}

impl FromRequestParts<AppState> for SeasonCtx {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Response> {
        let row = match parts.extensions.get::<Selected>() {
            Some(Selected(id)) => st.db.season(*id).await.ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    crate::render::notice("No such season.", false, "/"),
                )
                    .into_response()
            })?,
            None => st.db.default_season().await.ok_or_else(|| {
                crate::render::notice(
                    "There's no season yet — a sysadmin can create one under admin → seasons.",
                    false,
                    "/",
                )
                .into_response()
            })?,
        };
        let id = row.id;
        let _ = BASE.try_with(|b| *b.borrow_mut() = format!("/s/{id}"));
        Ok(SeasonCtx {
            id,
            rt: st.seasons.get(id),
            mods: st.db.season_mods(id).await,
            row,
        })
    }
}

/// `Option<SeasonCtx>`: `None` when no season exists at all (an explicitly
/// selected unknown season is still a 404).
impl axum::extract::OptionalFromRequestParts<AppState> for SeasonCtx {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        st: &AppState,
    ) -> Result<Option<Self>, Response> {
        if parts.extensions.get::<Selected>().is_none() && st.db.default_season().await.is_none() {
            return Ok(None);
        }
        <SeasonCtx as FromRequestParts<AppState>>::from_request_parts(parts, st)
            .await
            .map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn season_prefix_is_split() {
        assert_eq!(split("/s/3/board"), Some((3, "/board".into())));
        assert_eq!(
            split("/s/3/game/7/poptracker/state"),
            Some((3, "/game/7/poptracker/state".into()))
        );
        assert_eq!(split("/s/3"), Some((3, "/board".into())));
        assert_eq!(split("/s/3/"), Some((3, "/board".into())));
        assert_eq!(split("/s/x/board"), None);
        assert_eq!(split("/s/0/board"), None);
        assert_eq!(split("/season/3"), None);
        assert_eq!(split("/board"), None);
        // Instance pages have one URL only.
        assert_eq!(split("/s/1/admin/users"), None);
        assert_eq!(split("/s/1/admin/settings/ap/install"), None);
        assert_eq!(split("/s/1/api/me"), None);
        assert_eq!(split("/s/1/login"), None);
        assert_eq!(split("/s/1/admin"), Some((1, "/admin".into())));
        assert_eq!(split("/s/1/admin/seasonsx"), Some((1, "/admin/seasonsx".into())));
        assert_eq!(split("/s/1/admin/worlds/approve"), Some((1, "/admin/worlds/approve".into())));
    }
}
