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

use axum::{Router, response::Html, routing::get};
use tower_http::trace::TraceLayer;

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

    let app = Router::new()
        .route("/health", get(health))
        .route("/", get(landing))
        .layer(TraceLayer::new_for_http());

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
