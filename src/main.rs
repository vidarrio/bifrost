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
    Router,
    extract::{Form, State},
    http::HeaderMap,
    response::{Html, Redirect},
    routing::get,
};
use serde::Deserialize;
use tower_http::trace::TraceLayer;

mod ap;
mod checker;
mod db;
mod meta;
mod render;

type Shared = Arc<RwLock<ap::Board>>;

#[derive(Clone)]
struct AppState {
    board: Shared,
    db: db::Db,
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

    let board: Shared = Arc::new(RwLock::new(ap::Board::default()));
    match ap::ApConfig::from_env() {
        Some(cfg) => {
            let coop = parse_coop();
            tracing::info!("AP tracker: watching {} as slot {}", cfg.url, cfg.slot);
            ap::spawn(board.clone(), cfg, db.clone(), coop);
        }
        None => tracing::info!("AP tracker idle (set BIFROST_SLOT to enable the live board)"),
    }

    let state = AppState { board, db };
    let app = Router::new()
        .route("/health", get(health))
        .route("/", get(landing))
        .route("/check", get(check_form).post(check_run))
        .route("/board", get(board_view))
        .route("/claim", get(claim_get).post(claim_post))
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

async fn landing() -> Html<&'static str> {
    Html(LANDING)
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

async fn board_view(State(st): State<AppState>) -> Html<String> {
    let snap = match st.board.read() {
        Ok(b) => b.snapshot(),
        Err(_) => return Html("board unavailable".into()),
    };
    let season = snap.seed.clone().unwrap_or_default();
    let slots = st.db.slots(&season).await.unwrap_or_default();
    render::board_page(&snap, &slots)
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
}

async fn claim_post(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ClaimForm>,
) -> Redirect {
    let (me, is_admin) = identity(&headers);
    match f.op.as_str() {
        "claim" => {
            let _ = st.db.claim(f.slot_id, &me).await;
        }
        "unclaim" => {
            let _ = st.db.unclaim(f.slot_id, &me, is_admin).await;
        }
        _ => {}
    }
    Redirect::to("/claim")
}
