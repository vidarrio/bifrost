//! Bifrost — the hub for all things Archipelago.
//!
//! Phase 0 is the service shell: a landing page and a `/health` probe. Later
//! phases layer on the YAML checker (shelling out to the local AP venv), the
//! live multiworld board (a websocket client to the Archipelago MultiServer),
//! kanidm SSO, and the Discord bot/Discord integration.
//!
//! Runs alongside the Archipelago MultiServer on the Bifrost host; fronted by Traefik at
//! bifrost.example.org (public shell now; `/admin` + generate routes get `lan-only`
//! once they exist).

use std::sync::{Arc, RwLock};

use axum::{
    Router,
    extract::{Form, State},
    response::Html,
    routing::get,
};
use serde::Deserialize;
use tower_http::trace::TraceLayer;

mod ap;
mod checker;

type Shared = Arc<RwLock<ap::Board>>;

/// The landing page. Kept in its own file so Phase 1 can flesh out the hub
/// without touching the server wiring.
const LANDING: &str = include_str!("../static/index.html");

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bifrost=info,tower_http=info".into()),
        )
        .init();

    // Port is overridable for local runs; the service defaults to 8080 (Traefik
    // proxies bifrost.example.org here).
    let port: u16 = std::env::var("BIFROST_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);

    // Shared live-board state, fed by the AP tracker task (when a slot is
    // configured for the current season).
    let board: Shared = Arc::new(RwLock::new(ap::Board::default()));
    match ap::ApConfig::from_env() {
        Some(cfg) => {
            tracing::info!("AP tracker: watching {} as slot {}", cfg.url, cfg.slot);
            ap::spawn(board.clone(), cfg);
        }
        None => tracing::info!("AP tracker idle (set BIFROST_SLOT to enable the live board)"),
    }

    let app = Router::new()
        .route("/health", get(health))
        .route("/", get(landing))
        .route("/check", get(check_form).post(check_run))
        .route("/board", get(board_page))
        .layer(TraceLayer::new_for_http())
        .with_state(board);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    tracing::info!("Bifrost listening on {addr}");
    axum::serve(listener, app).await.expect("serve");
}

/// Liveness probe used by the deploy script and the container's health checks.
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

async fn board_page(State(board): State<Shared>) -> Html<String> {
    ap::board_page(&board)
}
