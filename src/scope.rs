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

use crate::id::Id;
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
struct Selected(Id);

/// Instance-level pages: not about a season, so not reachable under `/s/{id}/`
/// (one URL each — a forward-auth proxy gating them by prefix sees them all).
const INSTANCE_ROUTES: &[&str] = &[
    "/signups",
    "/admin/seasons",
    "/admin/settings",
    "/admin/catalog",
    "/admin/common-games",
    "/admin/users",
    "/admin/links",
    "/api",
    "/export",
    "/login",
    "/logout",
    "/setup",
    "/invite",
    "/reset",
    "/health",
    "/favicon.svg",
];

/// Make a request about season `id`, as if it had come under `/s/{id}/`.
#[cfg(feature = "demo")]
pub fn select(req: &mut Request, id: Id) {
    req.extensions_mut().insert(Selected(id));
}

/// Is `path` an instance-level page (one of [`INSTANCE_ROUTES`])?
#[cfg(feature = "demo")]
pub fn is_instance(path: &str) -> bool {
    INSTANCE_ROUTES.iter().any(|r| {
        path.strip_prefix(r)
            .is_some_and(|after| after.is_empty() || after.starts_with('/'))
    })
}

/// Split `/s/{id}/rest` into `(id, "/rest")`; `/s/{id}` alone is its board.
/// `None` for an instance-level page under `/s/` (it then 404s), and for any
/// id or tail a forward-auth proxy's path rule might not read the same way
/// (`+1`, `/s/x//admin`): only a generated id selects a season.
fn split(path: &str) -> Option<(Id, String)> {
    let rest = path.strip_prefix("/s/")?;
    let (id, tail) = rest.split_once('/').unwrap_or((rest, ""));
    if tail.starts_with('/') {
        return None;
    }
    let id = id.parse::<Id>().ok()?;
    let tail = format!("/{}", if tail.is_empty() { "board" } else { tail });
    let instance = INSTANCE_ROUTES.iter().any(|r| {
        tail.strip_prefix(r)
            .is_some_and(|after| after.is_empty() || after.starts_with('/'))
    });
    (!instance).then_some((id, tail))
}

/// A path a proxy and Bifrost can only read one way: no percent-escapes, no
/// `.`/`..` segments and no empty segment (bar a trailing slash). Sign-in is
/// decided by path prefix here and, in forward-auth mode, at the proxy; a proxy
/// that normalizes the path before matching its rule would otherwise see a
/// different path than the one Bifrost tiers. No route needs any of these.
fn canonical(path: &str) -> bool {
    if path == "/" {
        return true;
    }
    let Some(rest) = path.strip_prefix('/') else {
        return false;
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    !path.contains('%')
        && !rest.is_empty()
        && rest.split('/').all(|s| !matches!(s, "" | "." | ".."))
}

/// The season id of a bare `/s/{id}` or `/s/{id}/`.
fn bare_season(path: &str) -> Option<&str> {
    let id = path.strip_prefix("/s/")?;
    let id = id.strip_suffix('/').unwrap_or(id);
    id.parse::<Id>().is_ok().then_some(id)
}

pub async fn rewrite(mut req: Request, next: Next) -> Response {
    if !canonical(req.uri().path()) {
        return (StatusCode::BAD_REQUEST, "Bad path").into_response();
    }
    // Its own URL, so a forward-auth proxy's path rule sees the board as the board.
    if let Some(id) = bare_season(req.uri().path()) {
        return axum::response::Redirect::to(&format!("/s/{id}/board")).into_response();
    }
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

/// The season a request under `/s/{id}/` is about, once [`rewrite`] has run.
#[cfg(feature = "demo")]
pub fn selected(req: &Request) -> Option<Id> {
    req.extensions().get::<Selected>().map(|s| s.0)
}

/// The season a request is about.
#[derive(Clone)]
pub struct SeasonCtx {
    pub id: Id,
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

/// Routes that decide for themselves: season admins' pages, and the links that
/// are how someone gets in.
fn gate_exempt(path: &str) -> bool {
    path == "/admin"
        || path.starts_with("/admin/")
        || path.starts_with("/seat/")
        || path.starts_with("/signup/join/")
}

/// Whether someone can see a season at its current stage. Before generation it's
/// for any signed-in user, unless signups are private; after, it's open unless
/// private, when it's for members. The season's moderators and sysadmins always can.
pub async fn can_view(st: &AppState, row: &SeasonRow, mods: &[i64], me: &crate::auth::Me) -> bool {
    if me.can(crate::auth::Action::ManageSeason { mods }) {
        return true;
    }
    let account = me.id();
    match row.state() {
        State::Draft | State::Signup => {
            account != 0
                && (!row.private_signup
                    || st.db.is_season_member(row.id, account).await
                    || st.db.is_season_invited(row.id, account).await)
        }
        _ => !row.private_live || (account != 0 && st.db.is_season_member(row.id, account).await),
    }
}

/// A season you can't see: sign in if you haven't, else it isn't there.
fn refused(st: &AppState, parts: &Parts, me: &crate::auth::Me) -> Response {
    if me.principal().is_none() {
        if st.auth.mode != crate::auth::AuthMode::Native {
            return (
                StatusCode::UNAUTHORIZED,
                "Sign in required. If you did, this server's operator needs to cover this page in the proxy and set season_pages: see the user guide.",
            )
                .into_response();
        }
        if parts.method != axum::http::Method::GET {
            return (StatusCode::UNAUTHORIZED, "Sign in required.").into_response();
        }
        let original = parts
            .extensions
            .get::<axum::extract::OriginalUri>()
            .map(|o| o.0.clone())
            .unwrap_or_else(|| parts.uri.clone());
        let rd = original
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");
        return axum::response::Redirect::to(&format!("/login?rd={}", crate::auth::pct(rd)))
            .into_response();
    }
    (
        StatusCode::NOT_FOUND,
        crate::render::notice("No such season.", false, "/"),
    )
        .into_response()
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
        let mods = st.db.season_mods(row.id).await;
        let me = parts
            .extensions
            .get::<crate::auth::Me>()
            .cloned()
            .unwrap_or_default();
        if !gate_exempt(parts.uri.path()) && !can_view(st, &row, &mods, &me).await {
            return Err(refused(st, parts, &me));
        }
        let id = row.id;
        let _ = BASE.try_with(|b| *b.borrow_mut() = format!("/s/{id}"));
        Ok(SeasonCtx {
            id,
            rt: st.seasons.get(id),
            mods,
            row,
        })
    }
}

/// Minesweeper's scope: an unscoped URL is general minesweeper, for any account.
/// A season's board is for its players and the people running it.
pub struct MineScope(pub Option<SeasonCtx>);

impl FromRequestParts<AppState> for MineScope {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Response> {
        if parts.extensions.get::<Selected>().is_none() {
            return Ok(MineScope(None));
        }
        let ctx = <SeasonCtx as FromRequestParts<AppState>>::from_request_parts(parts, st).await?;
        let me = parts
            .extensions
            .get::<crate::auth::Me>()
            .cloned()
            .unwrap_or_default();
        if !ctx.can_manage(&me) && !st.db.is_season_member(ctx.id, me.id()).await {
            return Err((
                StatusCode::FORBIDDEN,
                crate::render::notice("This season's minesweeper is for its players. Anyone can play the general one.", false, "/minesweeper"),
            )
                .into_response());
        }
        Ok(MineScope(Some(ctx)))
    }
}

/// A live game in a URL: its id, and the AP slot it's on in the current seed.
#[derive(Clone, Copy)]
pub struct GameRef {
    pub id: Id,
    pub ap_slot: i64,
}

impl FromRequestParts<AppState> for GameRef {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Response> {
        let not_found = || {
            (
                StatusCode::NOT_FOUND,
                crate::render::notice("No such game.", false, "board"),
            )
                .into_response()
        };
        // The season first: it sets the link base the not-found page needs.
        let ctx = <SeasonCtx as FromRequestParts<AppState>>::from_request_parts(parts, st).await?;
        let axum::extract::Path(id) = axum::extract::Path::<Id>::from_request_parts(parts, st)
            .await
            .map_err(|_| not_found())?;
        let ap_slot = st
            .db
            .ap_slot_of_game(ctx.id, id)
            .await
            .ok_or_else(not_found)?;
        Ok(GameRef { id, ap_slot })
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
        if parts.extensions.get::<Selected>().is_none() {
            // An unscoped page with a fallback (the YAML checker) doesn't answer
            // for a default season its visitor can't see.
            let Some(row) = st.db.default_season().await else {
                return Ok(None);
            };
            let me = parts
                .extensions
                .get::<crate::auth::Me>()
                .cloned()
                .unwrap_or_default();
            let mods = st.db.season_mods(row.id).await;
            if !can_view(st, &row, &mods, &me).await {
                return Ok(None);
            }
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
    fn only_canonical_paths_pass() {
        for ok in [
            "/",
            "/board",
            "/s/3/",
            "/s/abcdefghjkmnpqrs/game/7",
            "/admin/seasons",
        ] {
            assert!(canonical(ok), "{ok}");
        }
        for bad in [
            "",
            "//",
            "//admin",
            "/admin//users",
            "/./admin",
            "/s/1/../admin",
            "/admin/.",
            "/%61dmin",
            "/s/1/game/%2e%2e/admin",
        ] {
            assert!(!canonical(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn rewrite_refuses_ambiguous_paths_before_routing() {
        use tower::Service;
        let mut app = axum::Router::new()
            .fallback(|| async { "routed" })
            .layer(axum::middleware::from_fn(rewrite));
        for (path, status) in [
            ("/board", 200),
            ("/s/abcdefghjkmnpqrs/admin", 200),
            ("/s/abcdefghjkmnpqrs", 303),
            ("/s/abcdefghjkmnpqrs/", 303),
            ("/s/1", 200),
            ("/s/1/../admin", 400),
            ("/%61dmin", 400),
            ("//", 400),
        ] {
            let req = Request::builder()
                .uri(path)
                .body(axum::body::Body::empty())
                .unwrap();
            let res = app.call(req).await.unwrap();
            assert_eq!(res.status().as_u16(), status, "{path}");
        }
    }

    #[test]
    fn season_prefix_is_split() {
        const A: &str = "abcdefghjkmnpqrs";
        let id: Id = A.parse().unwrap();
        let split_at = |tail: &str| split(&format!("/s/{A}{tail}"));
        assert_eq!(split_at("/board"), Some((id, "/board".into())));
        assert_eq!(
            split_at("/game/7/poptracker/state"),
            Some((id, "/game/7/poptracker/state".into()))
        );
        assert_eq!(split_at(""), Some((id, "/board".into())));
        assert_eq!(split_at("/"), Some((id, "/board".into())));
        // Only a real id: a proxy rule must see every path that reaches a season.
        for bad in [
            "x",
            "0",
            "1",
            "+1",
            "-1",
            " 1",
            "01",
            "",
            "ABCDEFGHJKMNPQRS",
            "abcdefghjkmnpqr",
        ] {
            assert_eq!(split(&format!("/s/{bad}/admin")), None, "{bad}");
        }
        assert_eq!(split_at("//admin"), None);
        assert_eq!(split("/season/3"), None);
        assert_eq!(split("/board"), None);
        // Instance pages have one URL only.
        for tail in [
            "/admin/users",
            "/admin/settings/ap/install",
            "/api/me",
            "/login",
        ] {
            assert_eq!(split_at(tail), None, "{tail}");
        }
        assert_eq!(split_at("/admin"), Some((id, "/admin".into())));
        assert_eq!(
            split_at("/admin/seasonsx"),
            Some((id, "/admin/seasonsx".into()))
        );
        assert_eq!(
            split_at("/admin/worlds/approve"),
            Some((id, "/admin/worlds/approve".into()))
        );
    }
}
