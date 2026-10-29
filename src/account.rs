//! Account pages: native sign-in/out, first-run setup, invite and reset-link
//! redemption, and the sysadmin's users & links console (`/admin/users`).
//! The rules behind them live in [`crate::auth`]; storage in `db::auth`.

use axum::{
    Form, Router,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;

use crate::auth::{self, Action, AuthMode, Me, Role};
use crate::db::{LinkKind, RedeemError};
use crate::{AppState, render};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/login", get(login_get).post(login_post))
        .route("/logout", post(logout))
        .route("/setup", get(setup_get).post(setup_post))
        .route("/invite/{token}", get(invite_get).post(invite_post))
        .route("/reset/{token}", get(reset_get).post(reset_post))
        .route("/admin/users", get(users_get))
        .route("/admin/users/role", post(users_role))
        .route("/admin/users/disable", post(users_disable))
        .route("/admin/links/reset", post(links_reset))
        .route("/admin/links/invite", post(links_invite))
        .route("/admin/links/community", post(links_community))
        .route("/admin/links/revoke", post(links_revoke))
}

/// How long the links the console mints stay valid.
const RESET_TTL_SECS: i64 = 24 * 3600;
const INVITE_MAX_DAYS: i64 = 90;

fn native_only(st: &AppState) -> Option<Response> {
    (st.auth.mode != AuthMode::Native).then(|| {
        render::notice(
            "This instance signs in through its identity provider, not Bifrost accounts.",
            false,
            "/",
        )
        .into_response()
    })
}

/// A redirect that also sets the session cookie.
fn with_cookie(to: &str, cookie: axum::http::HeaderValue) -> Response {
    ([(header::SET_COOKIE, cookie)], Redirect::to(to)).into_response()
}

async fn hash(password: String) -> Result<String, String> {
    auth::hash_password_async(password).await
}

/// Shared checks for a chosen password + its confirmation.
fn new_password_problem(password: &str, confirm: &str) -> Option<String> {
    if password != confirm {
        return Some("The passwords don't match.".into());
    }
    auth::password_problem(password)
}

// ---------------------------------------------------------------- sign in/out

#[derive(Deserialize)]
struct LoginQuery {
    rd: Option<String>,
}

async fn login_get(State(st): State<AppState>, me: Me, Query(q): Query<LoginQuery>) -> Response {
    if let Some(r) = native_only(&st) {
        return r;
    }
    let rd = auth::safe_redirect(q.rd.as_deref());
    if me.principal().is_some() {
        return Redirect::to(&rd).into_response();
    }
    render::login_page(&rd, "", None).into_response()
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
    rd: Option<String>,
}

async fn login_post(
    State(st): State<AppState>,
    peer: Option<axum::Extension<ConnectInfo<std::net::SocketAddr>>>,
    headers: HeaderMap,
    Form(f): Form<LoginForm>,
) -> Response {
    if let Some(r) = native_only(&st) {
        return r;
    }
    let rd = auth::safe_redirect(f.rd.as_deref());
    let username = f.username.trim().to_string();
    let client = crate::ratelimit::client_ip(
        &crate::config::get().trusted_proxies,
        &headers,
        peer.map(|p| p.0.0.ip()),
    );
    // Usernames are at most 32 bytes: anything longer can't sign in, and
    // isn't worth remembering.
    if !auth::valid_username(&username) {
        return render::login_page(&rd, &username, Some("Wrong username or password."))
            .into_response();
    }
    let found = st.auth.db.login_lookup(&username).await;
    let known = found
        .as_ref()
        .is_some_and(|(_, h)| st.auth.known_device(&headers, &username, h));
    if let Err(wait) = st.auth.throttle.attempt(&username, client, known) {
        let msg = format!("Too many failed attempts — try again in {wait} s.");
        return render::login_page(&rd, &username, Some(&msg)).into_response();
    }
    // Verify against a dummy hash when the user doesn't exist, so a miss takes
    // as long as a wrong password.
    let hash = found.as_ref().map(|(_, h)| h.clone());
    let Some(ok) = auth::verify_password_async(hash, f.password).await else {
        // The password was never checked: don't count it.
        st.auth.throttle.cancel(&username, client);
        return render::login_page(&rd, &username, Some(auth::BUSY)).into_response();
    };
    match found {
        Some((id, hash)) if ok => {
            st.auth.throttle.succeed(&username, client);
            match st.auth.start_session(id).await {
                // Appended, not inserted: both cookies must be sent.
                Ok(cookie) => (
                    axum::response::AppendHeaders([
                        (header::SET_COOKIE, cookie),
                        (header::SET_COOKIE, st.auth.device_cookie(&username, &hash)),
                    ]),
                    Redirect::to(&rd),
                )
                    .into_response(),
                Err(e) => {
                    render::login_page(&rd, &username, Some(&format!("Couldn't sign in: {e}")))
                        .into_response()
                }
            }
        }
        _ => {
            // Already counted by `attempt`.
            render::login_page(&rd, &username, Some("Wrong username or password.")).into_response()
        }
    }
}

async fn logout(State(st): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Some(t) = auth::session_token(&headers) {
        st.auth.db.end_session(&auth::token_hash(&t)).await;
    }
    with_cookie("/", st.auth.session_cookie(None))
}

// ---------------------------------------------------------------- first run

#[derive(Deserialize)]
struct SetupQuery {
    token: Option<String>,
}

/// Is `token` the current first-run setup token?
fn setup_token_ok(st: &AppState, token: &str) -> bool {
    let cur = st.auth.setup_token.lock().unwrap();
    matches!(cur.as_deref(), Some(t) if !token.is_empty() && t == token)
}

async fn setup_get(State(st): State<AppState>, Query(q): Query<SetupQuery>) -> Response {
    if let Some(r) = native_only(&st) {
        return r;
    }
    let token = q.token.unwrap_or_default();
    if !setup_token_ok(&st, &token) {
        return render::notice(
            "Setup isn't available. On first run Bifrost prints a one-time setup link to its log.",
            false,
            "/",
        )
        .into_response();
    }
    render::setup_page(&token, "", None).into_response()
}

#[derive(Deserialize)]
struct SetupForm {
    token: String,
    username: String,
    password: String,
    confirm: String,
}

async fn setup_post(State(st): State<AppState>, Form(f): Form<SetupForm>) -> Response {
    if let Some(r) = native_only(&st) {
        return r;
    }
    if !setup_token_ok(&st, &f.token) || st.auth.db.account_count().await > 0 {
        return render::notice("Setup isn't available.", false, "/").into_response();
    }
    let username = f.username.trim().to_string();
    let problem = if !auth::valid_username(&username) {
        Some("Usernames are 3–32 letters, digits, '_', '.' or '-'.".to_string())
    } else {
        new_password_problem(&f.password, &f.confirm)
    };
    if let Some(p) = problem {
        return render::setup_page(&f.token, &username, Some(&p)).into_response();
    }
    let created = match hash(f.password).await {
        Ok(h) => st
            .auth
            .db
            .create_account(&username, &h, Role::Sysadmin, auth::now())
            .await
            .map_err(|e| e.to_string()),
        Err(e) => Err(e),
    };
    match created {
        Ok(id) => {
            *st.auth.setup_token.lock().unwrap() = None;
            tracing::info!("first-run setup: created sysadmin {username}");
            match st.auth.start_session(id).await {
                Ok(cookie) => with_cookie("/admin/users", cookie),
                Err(_) => Redirect::to("/login").into_response(),
            }
        }
        Err(e) => render::setup_page(
            &f.token,
            &username,
            Some(&format!("Couldn't create it: {e}")),
        )
        .into_response(),
    }
}

// ---------------------------------------------------------------- invites

async fn invite_get(State(st): State<AppState>, Path(token): Path<String>) -> Response {
    if let Some(r) = native_only(&st) {
        return r;
    }
    let live = st
        .auth
        .db
        .live_link(
            &auth::token_hash(&token),
            &[LinkKind::Invite, LinkKind::Community],
            auth::now(),
        )
        .await;
    match live {
        Some(_) => render::invite_page(&token, "", None).into_response(),
        None => dead_link(),
    }
}

fn dead_link() -> Response {
    render::notice(
        "This link is invalid, expired, used up or revoked. Ask an admin for a new one.",
        false,
        "/",
    )
    .into_response()
}

#[derive(Deserialize)]
struct InviteForm {
    username: String,
    password: String,
    confirm: String,
}

async fn invite_post(
    State(st): State<AppState>,
    Path(token): Path<String>,
    Form(f): Form<InviteForm>,
) -> Response {
    if let Some(r) = native_only(&st) {
        return r;
    }
    let username = f.username.trim().to_string();
    let problem = if !auth::valid_username(&username) {
        Some("Usernames are 3–32 letters, digits, '_', '.' or '-'.".to_string())
    } else {
        new_password_problem(&f.password, &f.confirm)
    };
    if let Some(p) = problem {
        return render::invite_page(&token, &username, Some(&p)).into_response();
    }
    let h = match hash(f.password).await {
        Ok(h) => h,
        Err(e) => return render::invite_page(&token, &username, Some(&e)).into_response(),
    };
    match st
        .auth
        .db
        .redeem_invite(&auth::token_hash(&token), &username, &h, auth::now())
        .await
    {
        Ok(id) => {
            tracing::info!("account {username} created via invite");
            match st.auth.start_session(id).await {
                Ok(cookie) => with_cookie("/", cookie),
                Err(_) => Redirect::to("/login").into_response(),
            }
        }
        Err(RedeemError::BadLink) => dead_link(),
        Err(RedeemError::UsernameTaken) => {
            render::invite_page(&token, &username, Some("That username is taken.")).into_response()
        }
        Err(RedeemError::Db(e)) => {
            render::invite_page(&token, &username, Some(&format!("Couldn't create it: {e}")))
                .into_response()
        }
    }
}

// ---------------------------------------------------------------- resets

async fn reset_get(State(st): State<AppState>, Path(token): Path<String>) -> Response {
    if let Some(r) = native_only(&st) {
        return r;
    }
    let live = st
        .auth
        .db
        .live_link(&auth::token_hash(&token), &[LinkKind::Reset], auth::now())
        .await;
    match live {
        Some(l) => {
            render::reset_page(&token, l.username.as_deref().unwrap_or("?"), None).into_response()
        }
        None => dead_link(),
    }
}

#[derive(Deserialize)]
struct ResetForm {
    password: String,
    confirm: String,
}

async fn reset_post(
    State(st): State<AppState>,
    Path(token): Path<String>,
    Form(f): Form<ResetForm>,
) -> Response {
    if let Some(r) = native_only(&st) {
        return r;
    }
    let th = auth::token_hash(&token);
    let Some(link) = st
        .auth
        .db
        .live_link(&th, &[LinkKind::Reset], auth::now())
        .await
    else {
        return dead_link();
    };
    let username = link.username.unwrap_or_default();
    if let Some(p) = new_password_problem(&f.password, &f.confirm) {
        return render::reset_page(&token, &username, Some(&p)).into_response();
    }
    let h = match hash(f.password).await {
        Ok(h) => h,
        Err(e) => return render::reset_page(&token, &username, Some(&e)).into_response(),
    };
    match st.auth.db.redeem_reset(&th, &h, auth::now()).await {
        Ok(Some(id)) => {
            st.auth.throttle.forgive(&username);
            tracing::info!("password reset for {username}");
            match st.auth.start_session(id).await {
                Ok(cookie) => with_cookie("/", cookie),
                Err(_) => Redirect::to("/login").into_response(),
            }
        }
        Ok(None) => dead_link(),
        Err(e) => render::reset_page(&token, &username, Some(&format!("Couldn't reset: {e}")))
            .into_response(),
    }
}

// ---------------------------------------------------------------- console

/// A just-minted link, shown once (only its hash is stored).
pub struct Minted {
    pub what: String,
    pub path: String,
}

async fn console(st: &AppState, me: &Me, minted: Option<Minted>, err: Option<&str>) -> Response {
    let accounts = st.auth.db.accounts().await;
    let links = st.auth.db.links().await;
    let me_id = me.principal().map(|p| p.account_id).unwrap_or(0);
    let idp_roles = st.auth.mode == AuthMode::ForwardAuth && !st.auth.fwd.role_groups.is_empty();
    render::users_page(&render::UsersView {
        accounts: &accounts,
        links: &links,
        me_id,
        native: st.auth.mode == AuthMode::Native,
        idp_roles,
        minted: minted.as_ref(),
        err,
        now: auth::now(),
    })
    .into_response()
}

fn forbidden() -> Response {
    render::notice("Sysadmins only.", false, "/").into_response()
}

async fn users_get(State(st): State<AppState>, me: Me) -> Response {
    if !me.can(Action::ManageInstance) {
        return forbidden();
    }
    console(&st, &me, None, None).await
}

#[derive(Deserialize)]
struct RoleForm {
    account_id: i64,
    role: String,
}

async fn users_role(State(st): State<AppState>, me: Me, Form(f): Form<RoleForm>) -> Response {
    if !me.can(Action::ManageInstance) {
        return forbidden();
    }
    if st.auth.mode == AuthMode::ForwardAuth && !st.auth.fwd.role_groups.is_empty() {
        return console(
            &st,
            &me,
            None,
            Some("Roles come from the identity provider's groups here."),
        )
        .await;
    }
    let Some(role) = Role::try_parse(&f.role) else {
        return console(&st, &me, None, Some("Unknown role.")).await;
    };
    let accounts = st.auth.db.accounts().await;
    let Some(target) = accounts.iter().find(|a| a.id == f.account_id) else {
        return console(&st, &me, None, Some("No such account.")).await;
    };
    let demoting_admin = target.role == "sysadmin" && role != Role::Sysadmin && !target.disabled;
    if demoting_admin && st.auth.db.active_sysadmins().await <= 1 {
        return console(
            &st,
            &me,
            None,
            Some("That's the last sysadmin — promote someone else first."),
        )
        .await;
    }
    match st.auth.db.set_role(f.account_id, role).await {
        Ok(()) => Redirect::to("/admin/users").into_response(),
        Err(e) => console(&st, &me, None, Some(&e.to_string())).await,
    }
}

#[derive(Deserialize)]
struct DisableForm {
    account_id: i64,
    disabled: bool,
}

async fn users_disable(State(st): State<AppState>, me: Me, Form(f): Form<DisableForm>) -> Response {
    if !me.can(Action::ManageInstance) {
        return forbidden();
    }
    if me.principal().map(|p| p.account_id) == Some(f.account_id) && f.disabled {
        return console(&st, &me, None, Some("You can't disable yourself.")).await;
    }
    let accounts = st.auth.db.accounts().await;
    let is_live_admin = accounts
        .iter()
        .any(|a| a.id == f.account_id && a.role == "sysadmin" && !a.disabled);
    if f.disabled && is_live_admin && st.auth.db.active_sysadmins().await <= 1 {
        return console(&st, &me, None, Some("That's the last sysadmin.")).await;
    }
    match st.auth.db.set_disabled(f.account_id, f.disabled).await {
        Ok(()) => Redirect::to("/admin/users").into_response(),
        Err(e) => console(&st, &me, None, Some(&e.to_string())).await,
    }
}

#[derive(Deserialize)]
struct ResetLinkForm {
    account_id: i64,
}

async fn links_reset(State(st): State<AppState>, me: Me, Form(f): Form<ResetLinkForm>) -> Response {
    if !me.can(Action::ManageInstance) {
        return forbidden();
    }
    let accounts = st.auth.db.accounts().await;
    let Some(target) = accounts
        .iter()
        .find(|a| a.id == f.account_id && a.has_password)
    else {
        return console(
            &st,
            &me,
            None,
            Some("Reset links are for Bifrost (password) accounts."),
        )
        .await;
    };
    let username = target.username.clone();
    match mint_reset(&st.auth, f.account_id, me.principal().map(|p| p.account_id)).await {
        Ok(path) => {
            let what = format!("Reset link for {username} — single use, valid 24 h");
            console(&st, &me, Some(Minted { what, path }), None).await
        }
        Err(e) => console(&st, &me, None, Some(&e)).await,
    }
}

/// Mint a single-use, 24 h password-reset link; returns its path. Shared with
/// the `bifrost admin reset-link` CLI.
pub async fn mint_reset(
    auth: &auth::AuthInner,
    account_id: i64,
    created_by: Option<i64>,
) -> Result<String, String> {
    let token = auth::new_token();
    let now = auth::now();
    auth.db
        .create_link(
            &auth::token_hash(&token),
            LinkKind::Reset,
            Some(account_id),
            "",
            Some(1),
            now + RESET_TTL_SECS,
            created_by,
            now,
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(format!("/reset/{token}"))
}

#[derive(Deserialize)]
struct InviteLinkForm {
    #[serde(default)]
    label: String,
    days: i64,
}

async fn links_invite(
    State(st): State<AppState>,
    me: Me,
    Form(f): Form<InviteLinkForm>,
) -> Response {
    if !me.can(Action::ManageInstance) {
        return forbidden();
    }
    if let Some(r) = native_only(&st) {
        return r;
    }
    let days = f.days.clamp(1, INVITE_MAX_DAYS);
    let what = format!("Single-use invite — valid {days} day(s)");
    mint_invite(
        &st,
        &me,
        LinkKind::Invite,
        &f.label,
        Some(1),
        days * 86400,
        what,
    )
    .await
}

#[derive(Deserialize)]
struct CommunityLinkForm {
    #[serde(default)]
    label: String,
    hours: i64,
}

async fn links_community(
    State(st): State<AppState>,
    me: Me,
    Form(f): Form<CommunityLinkForm>,
) -> Response {
    if !me.can(Action::ManageInstance) {
        return forbidden();
    }
    if let Some(r) = native_only(&st) {
        return r;
    }
    let hours = f.hours.clamp(1, INVITE_MAX_DAYS * 24);
    let what = format!("Community invite — anyone with it can join for the next {hours} h");
    mint_invite(
        &st,
        &me,
        LinkKind::Community,
        &f.label,
        None,
        hours * 3600,
        what,
    )
    .await
}

async fn mint_invite(
    st: &AppState,
    me: &Me,
    kind: LinkKind,
    label: &str,
    max_uses: Option<i64>,
    ttl: i64,
    what: String,
) -> Response {
    let token = auth::new_token();
    let now = auth::now();
    let label: String = label.trim().chars().take(80).collect();
    let r = st
        .auth
        .db
        .create_link(
            &auth::token_hash(&token),
            kind,
            None,
            &label,
            max_uses,
            now + ttl,
            me.principal().map(|p| p.account_id),
            now,
        )
        .await;
    match r {
        Ok(_) => {
            let minted = Minted {
                what,
                path: format!("/invite/{token}"),
            };
            console(st, me, Some(minted), None).await
        }
        Err(e) => console(st, me, None, Some(&e.to_string())).await,
    }
}

#[derive(Deserialize)]
struct RevokeForm {
    id: i64,
}

async fn links_revoke(State(st): State<AppState>, me: Me, Form(f): Form<RevokeForm>) -> Response {
    if !me.can(Action::ManageInstance) {
        return forbidden();
    }
    match st.auth.db.revoke_link(f.id).await {
        Ok(()) => Redirect::to("/admin/users").into_response(),
        Err(e) => console(&st, &me, None, Some(&e.to_string())).await,
    }
}
