//! Bifrost — the hub for all things Archipelago.
//!
//! Public: the hub (`/`), the YAML checker (`/check`), and the live board
//! (`/board`). Behind SSO (oauth2-proxy forward-auth, `archipelago_users`):
//! the claim page (`/claim`), where a logged-in user claims their Bifrost slot.
//! Identity arrives as `X-Auth-Request-*` headers set by oauth2-proxy.
//!
//! Runs alongside the Archipelago MultiServer on the Bifrost host; fronted by Traefik at
//! bifrost.example.org.

use std::sync::{Arc, RwLock};

use axum::{
    Json, Router,
    extract::{Form, Multipart, Path, Query, State},
    http::HeaderMap,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use tower_http::trace::TraceLayer;

mod ap;
mod checker;
mod db;
mod generate;
mod meta;
mod mine;
mod render;

use std::collections::HashMap;
use std::sync::Mutex;

type Shared = Arc<RwLock<ap::Board>>;

#[derive(Clone)]
struct AppState {
    board: Shared,
    db: db::Db,
    ap_cfg: Option<ap::ApConfig>,
    genstate: generate::GenState,
    /// Active minesweeper games keyed by (player sso, board kind). One
    /// in-progress board per player *per difficulty*, held server-side so each
    /// difficulty tab survives a tab close/reopen and difficulty switches.
    mine_games: Arc<Mutex<HashMap<(String, String), mine::Game>>>,
}

const LANDING: &str = include_str!("../static/index.html");

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bifrost=info,tower_http=info".into()),
        )
        .init();

    let port: u16 = std::env::var("BIFROST_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);

    let db_path =
        std::env::var("BIFROST_DB").unwrap_or_else(|_| "/var/lib/bifrost/bifrost.db".into());
    let db = db::Db::open(&db_path)
        .await
        .unwrap_or_else(|e| panic!("open db {db_path}: {e}"));

    let ap_cfg = ap::ApConfig::from_env();
    let board: Shared = Arc::new(RwLock::new(ap::Board::default()));
    match ap_cfg.clone() {
        Some(cfg) => {
            let coop = parse_coop();
            tracing::info!("AP tracker: watching {} as slot {}", cfg.url, cfg.slot);
            ap::spawn(board.clone(), cfg, db.clone(), coop);
        }
        None => tracing::info!("AP tracker idle (set BIFROST_SLOT to enable the live board)"),
    }

    let genstate: generate::GenState = Arc::new(std::sync::Mutex::new((false, "idle".into())));
    let state = AppState {
        board,
        db,
        ap_cfg,
        genstate,
        mine_games: Arc::new(Mutex::new(HashMap::new())),
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/", get(landing))
        .route("/favicon.svg", get(favicon))
        .route("/check", get(check_form).post(check_run))
        .route("/board", get(board_view))
        .route("/game/{ap_slot}", get(game_view))
        .route("/game/{ap_slot}/hints", get(game_hints))
        .route("/claim", get(claim_get).post(claim_post))
        .route("/hint", post(hint_post))
        .route("/prioritize", post(prioritize_post))
        .route("/signup", get(signup_get).post(signup_post))
        .route("/signup/apworld", post(signup_apworld))
        .route("/signup/check", post(signup_check))
        .route("/signup/delete", post(signup_delete))
        .route("/admin", get(admin_view))
        .route("/admin/signups", post(admin_signups))
        .route("/admin/generate", post(admin_generate))
        .route("/admin/regenerate", post(admin_regenerate))
        .route("/admin/clear", post(admin_clear))
        .route("/admin/delete-player", post(admin_delete_player))
        .route("/admin/delete-apworld", post(admin_delete_apworld))
        .route("/admin/console", post(admin_console))
        .route("/admin/connect-password", post(admin_connect_password))
        .route("/admin/minesweeper", post(admin_minesweeper))
        .route("/minesweeper", get(minesweeper_page))
        .route("/minesweeper/new", post(mine_new))
        .route("/minesweeper/reveal", post(mine_reveal))
        .route("/minesweeper/state", get(mine_state))
        .route("/minesweeper/pause", post(mine_pause))
        .route("/api/me", get(api_me))
        .layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    tracing::info!("Bifrost listening on {addr}");
    axum::serve(listener, app).await.expect("serve");
}

/// `BIFROST_COOP="Cam:Abi,Foo:Bar"` — map an AP slot name to an extra Bifrost
/// slot name (a co-op seat) for the current season's seeding.
fn parse_coop() -> Vec<(String, String)> {
    std::env::var("BIFROST_COOP")
        .unwrap_or_default()
        .split(',')
        .filter_map(|p| p.split_once(':'))
        .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
        .filter(|(a, b)| !a.is_empty() && !b.is_empty())
        .collect()
}

/// Read the SSO identity oauth2-proxy forwarded. Returns (username, is_admin).
fn identity(h: &HeaderMap) -> (String, bool) {
    let user = h
        .get("x-auth-request-preferred-username")
        .or_else(|| h.get("x-auth-request-user"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();
    let is_admin = h
        .get("x-auth-request-groups")
        .and_then(|v| v.to_str().ok())
        .map(|g| g.contains("archipelago_admins"))
        .unwrap_or(false);
    (user, is_admin)
}

async fn health() -> &'static str {
    "ok"
}

async fn favicon() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "image/svg+xml")],
        render::FAVICON,
    )
}

/// Identity for the header bar, filled client-side. Gated behind oauth2-proxy,
/// so a real user is always present; `user` is null only if the headers are
/// somehow missing, which the page renders as a "sign in" link.
async fn api_me(headers: HeaderMap) -> Json<serde_json::Value> {
    let (user, is_admin) = identity(&headers);
    let user = if user == "unknown" || user.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(user)
    };
    Json(serde_json::json!({ "user": user, "admin": is_admin }))
}

async fn landing(State(st): State<AppState>) -> Html<String> {
    // The season roster is rendered live from the current game so it never needs
    // hand-editing between seasons: AP slot names come from the live board, the
    // people (incl. co-op) from the DB.
    let (season, ap_names) = match st.board.read() {
        Ok(b) => {
            let season = b.seed.clone().unwrap_or_default();
            let names: std::collections::HashMap<i64, String> =
                b.slots.iter().map(|(k, s)| (*k, s.name.clone())).collect();
            (season, names)
        }
        Err(_) => (String::new(), std::collections::HashMap::new()),
    };
    let slots = st.db.slots(&season).await.unwrap_or_default();
    let roster = render::landing_roster(&slots, &ap_names);
    Html(LANDING.replace("<!--ROSTER-->", &roster))
}

#[derive(Deserialize)]
struct CheckForm {
    yaml: String,
}

async fn check_form() -> Html<String> {
    checker::form_page()
}

async fn check_run(Form(form): Form<CheckForm>) -> Html<String> {
    match checker::check_yaml(&form.yaml).await {
        Ok(res) => checker::result_page(&res),
        Err(e) => checker::error_page(&e),
    }
}

/// Fetch the season's prioritized hints, first dropping the star from any hint
/// whose item has since been found (a found hint is non-prioritizable, so a
/// pre-found one is cleaned up in the DB and excluded from the returned set).
async fn live_priorities(
    st: &AppState,
    snap: &ap::BoardSnapshot,
    season: &str,
) -> std::collections::HashSet<(i64, i64, i64)> {
    let mut prios = st.db.priorities(season).await.unwrap_or_default();
    if prios.is_empty() {
        return prios;
    }
    let stale: Vec<(i64, i64, i64)> = snap
        .ap
        .values()
        .flat_map(|a| a.hint_views.iter())
        .filter(|h| h.found)
        .map(|h| (h.finding, h.location, h.item))
        .filter(|id| prios.contains(id))
        .collect();
    if !stale.is_empty() {
        st.db.unprioritize_many(season, &stale).await;
        for id in &stale {
            prios.remove(id);
        }
    }
    prios
}

async fn board_view(State(st): State<AppState>) -> Html<String> {
    let snap = match st.board.read() {
        Ok(b) => b.snapshot(),
        Err(_) => return Html("board unavailable".into()),
    };
    let season = snap.seed.clone().unwrap_or_default();
    let slots = st.db.slots(&season).await.unwrap_or_default();
    let prios = live_priorities(&st, &snap, &season).await;
    render::board_page(&snap, &slots, &prios)
}

/// A safe local redirect target (must be an absolute path, not protocol-relative).
fn safe_back(b: Option<&str>, default: &str) -> String {
    match b {
        Some(p) if p.starts_with('/') && !p.starts_with("//") => p.to_string(),
        _ => default.to_string(),
    }
}

async fn game_view(
    Path(ap_slot): Path<i64>,
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Html<String> {
    let (me, is_admin) = identity(&headers);
    let (snap, game, item_names, connect_name) = match st.board.read() {
        Ok(b) => {
            let (game, cn) = b
                .slots
                .get(&ap_slot)
                .map(|s| (s.game.clone(), s.name.clone()))
                .unwrap_or_default();
            let items = b
                .meta
                .as_ref()
                .and_then(|m| m.item_names.get(&game))
                .map(|nm| {
                    let mut v: Vec<String> = nm.values().cloned().collect();
                    v.sort();
                    v.dedup();
                    v
                })
                .unwrap_or_default();
            (b.snapshot(), game, items, cn)
        }
        Err(_) => return Html("board unavailable".into()),
    };
    if game.is_empty() {
        return Html("<p>Unknown game slot.</p>".into());
    }
    let season = snap.seed.clone().unwrap_or_default();
    let slots = st.db.slots(&season).await.unwrap_or_default();
    let prios = live_priorities(&st, &snap, &season).await;

    // Cost to hint (per AP: hint_cost% of a world's locations).
    let world_size = snap.ap.get(&ap_slot).map(|a| a.world_size).unwrap_or(0) as i64;
    let cost = if snap.hint_cost > 0 && world_size > 0 {
        (snap.hint_cost * world_size / 100).max(1)
    } else {
        0
    };
    // Live points for the viewer's own slot (fetched by connecting as it). Only
    // an actual claimant sees the hint form, so only fetch points for them.
    let i_claim = slots
        .iter()
        .any(|r| r.ap_slot == ap_slot && r.claimed_by.as_deref() == Some(me.as_str()));
    let points = match (i_claim, &st.ap_cfg) {
        (true, Some(cfg)) if !connect_name.is_empty() => {
            ap::slot_points(&cfg.url, &cfg.password, &connect_name).await
        }
        _ => None,
    };
    let credits = if i_claim {
        st.db.credits(&season, &me).await
    } else {
        0
    };

    render::game_page(
        ap_slot,
        &game,
        &snap,
        &slots,
        &prios,
        &item_names,
        &me,
        is_admin,
        cost,
        points,
        credits,
    )
}

#[derive(Deserialize)]
struct PrioForm {
    finding: i64,
    location: i64,
    item: i64,
    receiving: i64,
    back: Option<String>,
}

async fn prioritize_post(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<PrioForm>,
) -> Redirect {
    let (me, is_admin) = identity(&headers);
    let (season, is_found) = match st.board.read() {
        Ok(b) => {
            let season = b.seed.clone().unwrap_or_default();
            // Already-found hints are non-prioritizable.
            let found = b.snapshot().ap.values().any(|a| {
                a.hint_views.iter().any(|h| {
                    h.found && h.finding == f.finding && h.location == f.location && h.item == f.item
                })
            });
            (season, found)
        }
        Err(_) => (String::new(), false),
    };
    // A hint is prioritized by the item's owner: a claimant of the receiving
    // slot (whose items the hint is about), or an admin. The finder's world can
    // be someone else's, so we gate on `receiving`, not `finding`.
    if !is_found && (is_admin || st.db.claims_ap_slot(&season, &me, f.receiving).await) {
        let _ = st
            .db
            .toggle_priority(&season, f.finding, f.location, f.item)
            .await;
    }
    Redirect::to(&safe_back(f.back.as_deref(), "/board"))
}

async fn claim_get(State(st): State<AppState>, headers: HeaderMap) -> Html<String> {
    let (me, is_admin) = identity(&headers);
    let snap = match st.board.read() {
        Ok(b) => b.snapshot(),
        Err(_) => return Html("board unavailable".into()),
    };
    let season = snap.seed.clone().unwrap_or_default();
    let slots = st.db.slots(&season).await.unwrap_or_default();
    render::claim_page(&snap, &slots, &me, is_admin)
}

#[derive(Deserialize)]
struct ClaimForm {
    slot_id: i64,
    op: String,
    back: Option<String>,
}

async fn claim_post(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ClaimForm>,
) -> Response {
    let (me, is_admin) = identity(&headers);
    let back = safe_back(f.back.as_deref(), "/claim");
    match f.op.as_str() {
        "claim" => match st.db.claim_one(f.slot_id, &me).await {
            Ok(db::ClaimOutcome::AlreadyHave(name)) => render::notice(
                &format!("You already claim {name} — one slot per player. Unclaim it first if you want to move."),
                false,
                &back,
            )
            .into_response(),
            Ok(db::ClaimOutcome::SlotTaken) => {
                render::notice("That slot is already claimed.", false, &back).into_response()
            }
            _ => Redirect::to(&back).into_response(),
        },
        "unclaim" => {
            let _ = st.db.unclaim(f.slot_id, &me, is_admin).await;
            Redirect::to(&back).into_response()
        }
        _ => Redirect::to(&back).into_response(),
    }
}

#[derive(Deserialize)]
struct HintForm {
    slot_id: i64,
    item: String,
}

/// The server's `!hint` reply is informational text, so infer whether the hint
/// was actually placed (vs. rejected for points / not found) to drive the UI.
fn hint_rejected(text: &str) -> bool {
    let t = text.to_lowercase();
    [
        "not enough",
        "do not have",
        "unable to find",
        "did you mean",
        "afford", // "can't afford" / "cannot afford"
        "sorry",
    ]
    .iter()
    .any(|k| t.contains(k))
}

async fn hint_post(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<HintForm>,
) -> Json<serde_json::Value> {
    let (me, _is_admin) = identity(&headers);
    let reject = |msg: &str| Json(serde_json::json!({"ok": false, "message": msg}));
    let detail = st.db.slot_detail(f.slot_id).await.ok().flatten();
    let Some((ap_slot, claimant)) = detail else {
        return reject("Unknown slot.");
    };
    if claimant.as_deref() != Some(me.as_str()) {
        return reject("Only the player who claimed this slot can hint on it.");
    }
    let connect_name = st
        .board
        .read()
        .ok()
        .and_then(|b| b.slots.get(&ap_slot).map(|s| s.name.clone()));
    let Some(name) = connect_name else {
        return reject("That slot isn't live right now.");
    };
    let Some(cfg) = st.ap_cfg.as_ref() else {
        return reject("Hinting isn't configured.");
    };
    let season = st
        .board
        .read()
        .ok()
        .and_then(|b| b.seed.clone())
        .unwrap_or_default();
    let creds = st.db.credits(&season, &me).await;
    match ap::send_hint(&cfg.url, &cfg.password, &name, f.item.trim()).await {
        Ok(parts) => {
            let text = st
                .board
                .read()
                .map(|b| ap::resolve_parts(&b, &parts))
                .unwrap_or_default();
            if !hint_rejected(&text) {
                // Placed with AP points — report the new balance.
                let points = ap::slot_points(&cfg.url, &cfg.password, &name).await;
                return Json(serde_json::json!({
                    "ok": true, "message": text, "points": points, "credits": creds
                }));
            }
            // Rejected. If it was only a points shortfall and the player holds a
            // bonus-hint credit, place the hint as admin (free) and spend one.
            if rejected_for_points(&text) && creds > 0 {
                let admin_cmd = format!("/hint {name} {}", f.item.trim());
                match ap::send_admin(
                    &cfg.url,
                    &cfg.slot,
                    &cfg.password,
                    &cfg.admin_password,
                    &admin_cmd,
                )
                .await
                {
                    Ok(lists) => {
                        let msg = st
                            .board
                            .read()
                            .map(|b| {
                                lists
                                    .iter()
                                    .map(|p| ap::resolve_parts(&b, p))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            })
                            .unwrap_or_default();
                        if hint_rejected(&msg) {
                            return Json(serde_json::json!({
                                "ok": false, "message": msg, "credits": creds
                            }));
                        }
                        st.db.spend_credit(&season, &me).await;
                        let newc = st.db.credits(&season, &me).await;
                        let points = ap::slot_points(&cfg.url, &cfg.password, &name).await;
                        // The admin /hint returns no text on success, so confirm
                        // it ourselves; the new hint shows in the list below.
                        let confirm = if msg.trim().is_empty() {
                            format!(
                                "Out of points — spent a bonus hint ({newc} left). It's in this world's hints below."
                            )
                        } else {
                            format!("Out of points — spent a bonus hint ({newc} left). {msg}")
                        };
                        return Json(serde_json::json!({
                            "ok": true, "message": confirm, "points": points, "credits": newc
                        }));
                    }
                    Err(e) => {
                        return Json(serde_json::json!({
                            "ok": false, "message": format!("Bonus hint failed: {e}"), "credits": creds
                        }));
                    }
                }
            }
            Json(serde_json::json!({"ok": false, "message": text, "credits": creds}))
        }
        Err(e) => Json(serde_json::json!({
            "ok": false, "message": format!("Hint failed: {e}"), "credits": creds
        })),
    }
}

/// A `!hint` rejection that is specifically a points shortfall (vs. a bad item).
fn rejected_for_points(text: &str) -> bool {
    let t = text.to_lowercase();
    ["not enough", "do not have", "afford"]
        .iter()
        .any(|k| t.contains(k))
}

/// The "Hints in this world" rows for one AP slot, as an HTML fragment — used by
/// the game page to live-refresh the list after a hint (no full reload).
async fn game_hints(
    Path(ap_slot): Path<i64>,
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Html<String> {
    let (me, is_admin) = identity(&headers);
    let (snap, season) = match st.board.read() {
        Ok(b) => (b.snapshot(), b.seed.clone().unwrap_or_default()),
        Err(_) => return Html(String::new()),
    };
    let prios = live_priorities(&st, &snap, &season).await;
    let can_prioritize = is_admin || st.db.claims_ap_slot(&season, &me, ap_slot).await;
    let (views, found_items) = snap
        .ap
        .get(&ap_slot)
        .map(|a| (a.hint_views.clone(), a.found_items.clone()))
        .unwrap_or_default();
    Html(render::hint_rows(
        &views,
        &found_items,
        &prios,
        &format!("/game/{ap_slot}"),
        can_prioritize,
    ))
}

const UPLOAD_DIR: &str = "/var/lib/bifrost/uploads";
const CUSTOM_WORLDS: &str = "/srv/archipelago/custom_worlds";
/// Bifrost writes the requested connect password here; a root `bifrost-connpw`
/// path unit persists it to host.yaml + the service env and restarts bifrost.
const CONNPW_REQUEST: &str = "/var/lib/bifrost/connect_password.request";

fn slugify(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    if out.is_empty() { "slot".into() } else { out }
}

async fn signup_get(State(st): State<AppState>, headers: HeaderMap) -> Html<String> {
    let (me, is_admin) = identity(&headers);
    let subs = st.db.submissions().await.unwrap_or_default();
    let open = st.db.signups_open().await;
    render::signup_page(&subs, &me, is_admin, open)
}

/// Step 1 of signup: install an uploaded `.apworld` into the server's
/// `custom_worlds/` right away, so the checker can validate a custom-game YAML
/// against it in step 2. Returns JSON for the wizard.
async fn signup_apworld(mut mp: Multipart) -> Json<serde_json::Value> {
    let mut bytes: Option<Vec<u8>> = None;
    let mut fname = String::new();
    while let Ok(Some(field)) = mp.next_field().await {
        if field.name() == Some("apworld") {
            fname = field.file_name().unwrap_or("").to_string();
            bytes = field.bytes().await.ok().map(|b| b.to_vec());
        } else {
            let _ = field.bytes().await;
        }
    }
    let Some(bytes) = bytes.filter(|b| !b.is_empty()) else {
        return Json(serde_json::json!({"ok": false, "error": "No file received."}));
    };
    // .apworld files are zips; a quick magic-byte check rejects obvious mistakes.
    if bytes.len() < 4 || &bytes[..2] != b"PK" {
        return Json(serde_json::json!({
            "ok": false, "error": "That doesn't look like a valid .apworld (zip) file."
        }));
    }
    let stem = std::path::Path::new(&fname)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("world");
    let base = slugify(stem);
    let dst = format!("{CUSTOM_WORLDS}/{base}.apworld");
    if tokio::fs::write(&dst, &bytes).await.is_err() {
        return Json(serde_json::json!({
            "ok": false, "error": "Couldn't install the apworld on the server."
        }));
    }
    Json(serde_json::json!({"ok": true, "world": format!("{base}.apworld")}))
}

/// Step 2 of signup: validate a YAML live against the installed worlds (incl. an
/// apworld just installed in step 1). Returns JSON for the wizard.
async fn signup_check(mut mp: Multipart) -> Json<serde_json::Value> {
    let mut yaml: Option<Vec<u8>> = None;
    while let Ok(Some(field)) = mp.next_field().await {
        if field.name() == Some("yaml") {
            yaml = field.bytes().await.ok().map(|b| b.to_vec());
        } else {
            let _ = field.bytes().await;
        }
    }
    let Some(yaml) = yaml.filter(|b| !b.is_empty()) else {
        return Json(serde_json::json!({"ok": false, "error": "No YAML received."}));
    };
    let yaml_str = String::from_utf8_lossy(&yaml).to_string();
    match checker::check_yaml(&yaml_str).await {
        Ok(r) => {
            let (name, game) = r
                .players
                .first()
                .map(|p| (p.name.clone(), p.game.clone().unwrap_or_default()))
                .unwrap_or_default();
            Json(serde_json::json!({
                "ok": r.ok, "name": name, "game": game, "errors": r.errors
            }))
        }
        Err(e) => Json(serde_json::json!({"ok": false, "error": e})),
    }
}

async fn signup_post(
    State(st): State<AppState>,
    headers: HeaderMap,
    mut mp: Multipart,
) -> Html<String> {
    let (me, _) = identity(&headers);
    if !st.db.signups_open().await {
        return render::signup_result("Signups are closed right now.", false);
    }
    let mut yaml: Option<Vec<u8>> = None;
    let mut is_custom = false;
    let mut coop = String::new();
    while let Ok(Some(field)) = mp.next_field().await {
        match field.name().unwrap_or("") {
            "yaml" => yaml = field.bytes().await.ok().map(|b| b.to_vec()),
            "is_custom" => is_custom = field.text().await.unwrap_or_default() == "1",
            "coop" => coop = field.text().await.unwrap_or_default(),
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    let Some(yaml) = yaml.filter(|b| !b.is_empty()) else {
        return render::signup_result("No YAML uploaded.", false);
    };
    let yaml_str = String::from_utf8_lossy(&yaml).to_string();
    // Trust-but-verify: the wizard already checked, but re-roll server-side so a
    // hand-crafted POST can't slip an invalid YAML through. Custom worlds are
    // installed by now, so everything must pass.
    let result = match checker::check_yaml(&yaml_str).await {
        Ok(r) => r,
        Err(e) => return render::signup_result(&format!("Checker error: {e}"), false),
    };
    let Some(p0) = result.players.first() else {
        return render::signup_result("Couldn't read a player from that YAML.", false);
    };
    let slot_name = p0.name.clone();
    let game = p0.game.clone().unwrap_or_else(|| "?".into());
    if !result.ok {
        return render::signup_result(
            &format!(
                "That YAML didn't pass: {}. Custom game? Install its apworld in step 1 first.",
                result.errors.join("; ")
            ),
            false,
        );
    }

    let slug = slugify(&slot_name);
    let _ = tokio::fs::create_dir_all(UPLOAD_DIR).await;
    let yaml_path = format!("{UPLOAD_DIR}/{slug}.yaml");
    if tokio::fs::write(&yaml_path, &yaml).await.is_err() {
        return render::signup_result("Couldn't store the YAML.", false);
    }
    let coop_clean: String = coop
        .split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ");

    match st
        .db
        .add_submission(
            &me,
            &slot_name,
            &game,
            is_custom,
            &coop_clean,
            &yaml_path,
            None,
        )
        .await
    {
        Ok(_) => render::signup_result(&format!("Signed up: {slot_name} — {game}."), true),
        Err(_) => render::signup_result(
            &format!("'{slot_name}' is already signed up — delete that entry first to resubmit."),
            false,
        ),
    }
}

#[derive(Deserialize)]
struct DelForm {
    id: i64,
}

async fn signup_delete(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<DelForm>,
) -> Redirect {
    let (me, is_admin) = identity(&headers);
    if let Ok(Some((yaml_file, apworld_file))) = st.db.delete_submission(f.id, &me, is_admin).await
    {
        let _ = tokio::fs::remove_file(&yaml_file).await;
        if let Some(a) = apworld_file {
            let _ = tokio::fs::remove_file(&a).await;
        }
    }
    Redirect::to("/signup")
}

/// Gather everything the admin page shows and render it.
async fn render_admin(st: &AppState) -> Html<String> {
    let (connected, seed, acdata) = st
        .board
        .read()
        .map(|b| {
            (
                b.connected,
                b.seed.clone(),
                b.autocomplete_data().to_string(),
            )
        })
        .unwrap_or((false, None, "{}".into()));
    let signups_open = st.db.signups_open().await;
    let minesweeper_on = st.db.minesweeper_enabled().await;
    let connect_pw = st
        .ap_cfg
        .as_ref()
        .map(|c| c.password.clone())
        .unwrap_or_default();
    let (running, gen_msg) = st
        .genstate
        .lock()
        .map(|g| (g.0, g.1.clone()))
        .unwrap_or((false, "unknown".into()));
    let subs = st.db.submissions().await.unwrap_or_default();
    let apworlds = generate::list_apworlds().await;
    render::admin_page(
        connected,
        seed.as_deref(),
        signups_open,
        minesweeper_on,
        &connect_pw,
        running,
        &gen_msg,
        &subs,
        &apworlds,
        &acdata,
    )
}

async fn admin_view(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let (_me, is_admin) = identity(&headers);
    if !is_admin {
        return render::notice("Admins only.", false, "/board").into_response();
    }
    render_admin(&st).await.into_response()
}

#[derive(Deserialize)]
struct SignupsForm {
    open: String,
}

async fn admin_signups(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<SignupsForm>,
) -> Redirect {
    let (_me, is_admin) = identity(&headers);
    if is_admin {
        let _ = st.db.set_signups_open(f.open == "1").await;
    }
    Redirect::to("/admin")
}

async fn admin_generate(State(st): State<AppState>, headers: HeaderMap) -> Redirect {
    let (_me, is_admin) = identity(&headers);
    if is_admin {
        let start = match st.genstate.lock() {
            Ok(mut g) if !g.0 => {
                g.0 = true;
                g.1 = "Starting…".into();
                true
            }
            _ => false,
        };
        if start {
            let (db, genstate) = (st.db.clone(), st.genstate.clone());
            tokio::spawn(async move { generate::run(db, genstate).await });
        }
    }
    Redirect::to("/admin")
}

async fn admin_regenerate(State(st): State<AppState>, headers: HeaderMap) -> Redirect {
    let (_me, is_admin) = identity(&headers);
    if is_admin {
        let old_season = st
            .board
            .read()
            .ok()
            .and_then(|b| b.seed.clone())
            .unwrap_or_default();
        // Need the live board's current seed to carry claims over; refuse if the
        // tracker hasn't populated it yet (e.g. right after a restart).
        if old_season.is_empty() {
            if let Ok(mut g) = st.genstate.lock()
                && !g.0
            {
                g.1 = "Regenerate needs the live board connected first — try again in a moment."
                    .into();
            }
            return Redirect::to("/admin");
        }
        let start = match st.genstate.lock() {
            Ok(mut g) if !g.0 => {
                g.0 = true;
                g.1 = "Starting…".into();
                true
            }
            _ => false,
        };
        if start {
            let (db, genstate) = (st.db.clone(), st.genstate.clone());
            tokio::spawn(async move { generate::regenerate(db, genstate, old_season).await });
        }
    }
    Redirect::to("/admin")
}

/// Delete the signup queue: all `submission` rows and their uploaded files.
async fn clear_signups(st: &AppState) {
    if let Ok(files) = st.db.clear_submissions().await {
        for f in files {
            let _ = tokio::fs::remove_file(&f).await;
        }
    }
}

/// Delete the current season's players: every Bifrost slot (claims cascade) and
/// the staged Players/ YAMLs.
async fn clear_current_players(st: &AppState) {
    let _ = st.db.clear_all_slots().await;
    let _ = generate::clear_player_yamls().await;
}

#[derive(Deserialize)]
struct ClearForm {
    what: String,
}

async fn admin_clear(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ClearForm>,
) -> Response {
    let (_me, is_admin) = identity(&headers);
    if !is_admin {
        return render::notice("Admins only.", false, "/board").into_response();
    }
    match f.what.as_str() {
        "apworlds" => {
            let _ = generate::clear_apworlds().await;
        }
        "signups" => clear_signups(&st).await,
        "players" => clear_current_players(&st).await,
        "save" => generate::clear_save().await,
        "world" => generate::clear_world().await,
        "reset" => {
            let _ = generate::clear_apworlds().await;
            clear_signups(&st).await;
            clear_current_players(&st).await;
            generate::clear_save().await;
            generate::clear_world().await;
            let _ = st.db.set_signups_open(false).await;
        }
        _ => {}
    }
    Redirect::to("/admin").into_response()
}

async fn admin_delete_player(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<DelForm>,
) -> Redirect {
    let (me, is_admin) = identity(&headers);
    if is_admin
        && let Ok(Some((yaml_file, apworld_file))) = st.db.delete_submission(f.id, &me, true).await
    {
        let _ = tokio::fs::remove_file(&yaml_file).await;
        if let Some(a) = apworld_file {
            let _ = tokio::fs::remove_file(&a).await;
        }
    }
    Redirect::to("/admin")
}

#[derive(Deserialize)]
struct ApworldForm {
    name: String,
}

async fn admin_delete_apworld(headers: HeaderMap, Form(f): Form<ApworldForm>) -> Redirect {
    let (_me, is_admin) = identity(&headers);
    if is_admin {
        let _ = generate::delete_apworld(&f.name).await;
    }
    Redirect::to("/admin")
}

#[derive(Deserialize)]
struct ConsoleForm {
    cmd: String,
}

/// Run a server command as admin and return its output as JSON, so the admin
/// page can append it to a live log without a page reload.
async fn admin_console(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ConsoleForm>,
) -> Json<serde_json::Value> {
    let (_me, is_admin) = identity(&headers);
    if !is_admin {
        return Json(serde_json::json!({"ok": false, "output": "Admins only."}));
    }
    let cmd = f.cmd.trim().to_string();
    let output = match st.ap_cfg.as_ref() {
        Some(cfg) => {
            match ap::send_admin(
                &cfg.url,
                &cfg.slot,
                &cfg.password,
                &cfg.admin_password,
                &cmd,
            )
            .await
            {
                Ok(lists) => {
                    let text = st
                        .board
                        .read()
                        .map(|b| {
                            lists
                                .iter()
                                .map(|p| ap::resolve_parts(&b, p))
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default();
                    if text.trim().is_empty() {
                        // Several server commands (/hint, /send, /option, /collect,
                        // /release …) succeed silently — they act on the room and
                        // broadcast to players but return no console text. An empty
                        // reply here is normal, not a failure.
                        "(command accepted — no console output; some commands like /hint and /send are silent on success and just broadcast to the room)".to_string()
                    } else {
                        text
                    }
                }
                Err(e) => format!("error: {e}"),
            }
        }
        None => "AP connection not configured.".to_string(),
    };
    Json(serde_json::json!({"ok": true, "cmd": cmd, "output": output}))
}

#[derive(Deserialize)]
struct ConnPwForm {
    password: String,
}

async fn admin_connect_password(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ConnPwForm>,
) -> Response {
    let (_me, is_admin) = identity(&headers);
    if !is_admin {
        return render::notice("Admins only.", false, "/board").into_response();
    }
    let pw = f.password.trim().to_string();
    if !(3..=64).contains(&pw.len())
        || !pw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return render::notice(
            "Password must be 3–64 characters: letters, numbers, - or _ only.",
            false,
            "/admin",
        )
        .into_response();
    }
    let Some(cfg) = st.ap_cfg.as_ref() else {
        return render::notice("AP connection isn't configured.", false, "/admin").into_response();
    };
    // Set it on the running server first (bifrost's env is still the old one, so
    // this connection still authenticates).
    if let Err(e) = ap::send_admin(
        &cfg.url,
        &cfg.slot,
        &cfg.password,
        &cfg.admin_password,
        &format!("/option password {pw}"),
    )
    .await
    {
        return render::notice(
            &format!("Couldn't set it on the running server: {e}"),
            false,
            "/admin",
        )
        .into_response();
    }
    // Persist after a short delay so this response reaches the browser before the
    // root helper restarts bifrost (writing the request file triggers it).
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let _ = tokio::fs::write(CONNPW_REQUEST, format!("{pw}\n")).await;
    });
    render::notice(
        "Connect password updated. Bifrost is restarting to apply it everywhere — give it a few seconds, then reload.",
        true,
        "/admin",
    )
    .into_response()
}

async fn admin_minesweeper(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<SignupsForm>,
) -> Redirect {
    let (_me, is_admin) = identity(&headers);
    if is_admin {
        let _ = st.db.set_minesweeper_enabled(f.open == "1").await;
    }
    Redirect::to("/admin")
}

async fn minesweeper_page(State(st): State<AppState>, headers: HeaderMap) -> Html<String> {
    let (me, _is_admin) = identity(&headers);
    let enabled = st.db.minesweeper_enabled().await;
    let season = st
        .board
        .read()
        .ok()
        .and_then(|b| b.seed.clone())
        .unwrap_or_default();
    let credits = st.db.credits(&season, &me).await;
    let standard = st.db.mine_stats(&season, &me, "standard").await;
    let expert = st.db.mine_stats(&season, &me, "expert").await;
    render::minesweeper_page(enabled, credits, &standard, &expert)
}

fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[derive(Deserialize)]
struct KindQuery {
    #[serde(default)]
    kind: String,
}

/// Map key for a player's game of a given difficulty.
fn mine_key(me: &str, kind: mine::Kind) -> (String, String) {
    (me.to_string(), kind.id().to_string())
}

#[derive(Deserialize)]
struct NewForm {
    #[serde(default)]
    kind: String,
}

async fn mine_new(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<NewForm>,
) -> Json<serde_json::Value> {
    let (me, _) = identity(&headers);
    if !st.db.minesweeper_enabled().await {
        return Json(serde_json::json!({"ok": false, "error": "Minesweeper is disabled."}));
    }
    let kind = mine::Kind::parse(&f.kind);
    let game = mine::Game::new(kind);
    let (w, h, mines) = (game.w, game.h, game.mines_n);
    if let Ok(mut games) = st.mine_games.lock() {
        let now = now_nanos();
        // Drop stale games (>12h old) so the map stays small, then replace this
        // player's game of this difficulty with the fresh one.
        games.retain(|_, g| now.saturating_sub(g.created) < 12 * 3600 * 1_000_000_000);
        games.insert(mine_key(&me, kind), game);
    }
    Json(serde_json::json!({"ok": true, "w": w, "h": h, "mines": mines, "kind": kind.id()}))
}

/// The player's in-progress game for a difficulty (if any), so the board + timer
/// survive closing/reopening the tab and switching difficulty tabs. Loading it
/// pauses the clock (`repose`); it resumes on the next reveal.
async fn mine_state(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<KindQuery>,
) -> Json<serde_json::Value> {
    let (me, _) = identity(&headers);
    let kind = mine::Kind::parse(&q.kind);
    let mut games = match st.mine_games.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    match games.get_mut(&mine_key(&me, kind)) {
        Some(g) if !g.over => {
            g.repose();
            let cells: Vec<_> = g
                .revealed_cells()
                .iter()
                .map(|(r, c, n)| serde_json::json!({"r": r, "c": c, "n": n}))
                .collect();
            Json(serde_json::json!({
                "active": true, "w": g.w, "h": g.h, "mines": g.mines_n,
                "cells": cells, "elapsed_ms": g.active_ms(), "running": g.running
            }))
        }
        _ => Json(serde_json::json!({"active": false})),
    }
}

/// Pause a difficulty's timer (called when the tab is hidden/closed or when
/// switching to the other difficulty). Idempotent.
async fn mine_pause(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<KindQuery>,
) -> Json<serde_json::Value> {
    let (me, _) = identity(&headers);
    let kind = mine::Kind::parse(&q.kind);
    if let Ok(mut games) = st.mine_games.lock() {
        if let Some(g) = games.get_mut(&mine_key(&me, kind)) {
            g.pause();
        }
    }
    Json(serde_json::json!({"ok": true}))
}

#[derive(Deserialize)]
struct RevealForm {
    r: usize,
    c: usize,
    #[serde(default)]
    kind: String,
}

async fn mine_reveal(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<RevealForm>,
) -> Json<serde_json::Value> {
    let (me, _) = identity(&headers);
    let key = mine_key(&me, mine::Kind::parse(&f.kind));
    // Reveal under the lock, then release it before any await.
    let outcome = {
        let mut games = match st.mine_games.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        match games.get_mut(&key) {
            Some(g) => {
                let rv = g.reveal(f.r, f.c);
                let (won, over) = (g.won, g.over);
                let (elapsed_ms, running) = (g.active_ms(), g.running);
                let kind = g.kind;
                if over {
                    games.remove(&key);
                }
                Some((rv, won, over, elapsed_ms, running, kind))
            }
            None => return Json(serde_json::json!({"ok": false, "error": "No active game."})),
        }
    };
    let Some((rv, won, over, elapsed_ms, running, kind)) = outcome else {
        return Json(serde_json::json!({"ok": false, "error": "No active game."}));
    };
    let mut credits: Option<i64> = None;
    // On a finished game (win or loss), record the result (for this board kind)
    // and return fresh stats for both kinds. A win is worth 1 bonus hint on
    // either difficulty.
    let stats_html = if over {
        let season = st
            .board
            .read()
            .ok()
            .and_then(|b| b.seed.clone())
            .unwrap_or_default();
        st.db
            .record_mine_result(&season, &me, kind, won, elapsed_ms)
            .await;
        if won {
            credits = st.db.add_credits(&season, &me, 1).await.ok();
        }
        let standard = st.db.mine_stats(&season, &me, "standard").await;
        let expert = st.db.mine_stats(&season, &me, "expert").await;
        Some(render::mine_stats_html(&standard, &expert))
    } else {
        None
    };
    let cells_json = |cells: &[(usize, usize, u8)]| {
        cells
            .iter()
            .map(|(r, c, n)| serde_json::json!({"r": r, "c": c, "n": n}))
            .collect::<Vec<_>>()
    };
    match rv {
        mine::Reveal::Play(cells) => Json(serde_json::json!({
            "ok": true, "state": "play", "cells": cells_json(&cells),
            "elapsed_ms": elapsed_ms, "running": running
        })),
        mine::Reveal::Won(cells) => Json(serde_json::json!({
            "ok": true, "state": "won", "cells": cells_json(&cells),
            "credits": credits, "stats": stats_html, "elapsed_ms": elapsed_ms, "running": running
        })),
        mine::Reveal::Lost(mines) => {
            let ms: Vec<_> = mines
                .iter()
                .map(|(r, c)| serde_json::json!({"r": r, "c": c}))
                .collect();
            Json(serde_json::json!({
                "ok": true, "state": "lost", "mines": ms, "stats": stats_html,
                "elapsed_ms": elapsed_ms, "running": running
            }))
        }
        mine::Reveal::Invalid => Json(serde_json::json!({"ok": false, "error": "Invalid move."})),
    }
}
