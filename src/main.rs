//! Bifrost — the hub for all things Archipelago.
//!
//! Public: the hub (`/`), the YAML checker (`/check`), and the live board
//! (`/board`). Behind sign-in: claiming slots, signups, game pages, hints; and
//! `/admin` for sysadmins. Who's signed in, and which routes need it, is
//! decided once per request by the `auth` middleware.
//!
//! This file is startup and shutdown; the handlers and the route table live in
//! `web`, the HTML in `render`.

use std::sync::Arc;

mod account;
mod activity;
mod ap;
mod ap_adapter;
mod apinstall;
mod apupdate;
mod apserver;
mod archive;
mod auth;
mod backup;
mod bundle;
mod checker;
mod cli;
mod config;
mod db;
mod generate;
mod inlogic;
mod load;
mod meta;
mod ptrack;
mod mine;
mod net;
mod ratelimit;
mod render;
mod runtime;
mod sandbox;
mod scope;
mod season;
mod web;
mod worlds;

use std::collections::HashMap;
use std::sync::Mutex;


#[derive(Clone)]
struct AppState {
    db: db::Db,
    /// Per-season boards, generation state and live workers.
    seasons: Arc<runtime::Runtimes>,
    /// Active minesweeper games keyed by (player account, board kind). One
    /// in-progress board per player *per difficulty*, held server-side so each
    /// difficulty tab survives a tab close/reopen and difficulty switches.
    mine_games: Arc<Mutex<HashMap<(i64, String), mine::Game>>>,
    auth: auth::Auth,
}

impl AppState {
    /// Work on the minesweeper games under their lock, outside any async body.
    /// A poisoned lock is recovered: the games are plain data.
    fn with_mine_games<R>(&self, f: impl FnOnce(&mut HashMap<(i64, String), mine::Game>) -> R) -> R {
        f(&mut self.mine_games.lock().unwrap_or_else(|p| p.into_inner()))
    }
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

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli::run_early(&args) {
        std::process::exit(code);
    }

    let cfg = config::init(config::Config::load());
    cfg.ensure_dirs()
        .unwrap_or_else(|e| panic!("create data dirs under {}: {e}", cfg.data_dir.display()));
    let port = cfg.port;
    if let Some(code) = cli::run_offline(&args) {
        std::process::exit(code);
    }

    let db_path = cfg.db_path();
    let mut db = db::Db::open(&db_path.to_string_lossy())
        .await
        .unwrap_or_else(|e| panic!("open db {}: {e}", db_path.display()));
    if cfg.ap_managed {
        db = db.with_ap_ports(cfg.ap_ports.clone());
    }

    if let Some(code) = cli::run(&args, &db).await {
        std::process::exit(code);
    }
    match db.recover_interrupted_generations().await {
        Ok(0) => {}
        Ok(n) => tracing::warn!("{n} season(s) were mid-generation when Bifrost stopped; put back to where they were"),
        Err(e) => tracing::error!("recovering interrupted generations: {e}"),
    }

    // How AP helpers (which run apworld code) are contained on this host.
    let sb = sandbox::init(sandbox::Sandbox::detect(&cfg.ap_python).await);
    if sb.strong() {
        tracing::info!("isolation: {}", sb.posture());
    } else {
        tracing::warn!("isolation: {}", sb.posture());
    }

    let auth = init_auth(&db).await;

    // An AP server run elsewhere (BIFROST_AP_MODE=external); managed mode
    // starts one per live season instead.
    let external = if cfg.ap_managed { None } else { ap::ApConfig::from_env() };
    let seasons = Arc::new(runtime::Runtimes::new(db.clone(), external, web::parse_coop()));
    if cfg.ap_managed {
        // Ports are held only by live seasons, inside the configured range.
        for name in db.reconcile_ap_ports().await {
            tracing::error!("season “{name}” is live but every AP port is taken; widen BIFROST_AP_PORTS");
        }
    }
    // Which Archipelago install is active, and which one each season's seed
    // was generated with (it keeps running on that one).
    let _ = std::fs::remove_dir_all(cfg.tmp_dir());
    apinstall::load(
        db.ap_active().await,
        db.seasons()
            .await
            .into_iter()
            .filter(|s| s.state() != db::State::Archived)
            .filter_map(|s| Some((s.id, s.ap_version?)))
            .collect(),
    );
    let bundled = apinstall::bundled();
    tracing::info!("Archipelago {} bundled at {}", bundled.version, bundled.dir.display());
    if bundle::present()
        && let Some(current) = bundled.dir.parent()
    {
        bundle::cleanup(&cfg.data_dir, current, &apinstall::pinned_versions());
    }
    let active = apinstall::active();
    if !active.bundled {
        tracing::info!("Archipelago {} active (new seasons use it), at {}", active.version, active.dir.display());
    }
    for season in db.seasons().await {
        if season.state() != db::State::Archived {
            let _ = config::get().season(season.id).ensure();
        }
        if season.state() == db::State::Live {
            match apinstall::missing_pin(season.id) {
                // Its version isn't here any more (an image upgrade replaced
                // the bundled install): reinstall that release before its
                // server starts, rather than run the game on another version.
                Some(v) => {
                    let (seasons, id) = (seasons.clone(), season.id);
                    tokio::spawn(async move {
                        tracing::warn!("season {id} runs Archipelago {v}, which isn't installed; installing it first");
                        if let Err(e) = apupdate::ensure(&v).await {
                            tracing::error!("season {id}: couldn't install Archipelago {v} ({e}); starting it on the active version");
                        }
                        seasons.start_live(id).await;
                    });
                }
                None => seasons.start_live(season.id).await,
            }
        }
        if season.state().accepts_signups() {
            let (db, id) = (db.clone(), season.id);
            tokio::spawn(async move { worlds::recheck_stale(&db, id).await });
        }
    }

    backup::spawn_nightly(db.clone());
    load::spawn_sampler(cfg.data_dir.clone());

    let shutdown_seasons = seasons.clone();
    let shutdown_db = db.clone();
    let state = AppState {
        db,
        seasons,
        mine_games: Arc::new(Mutex::new(HashMap::new())),
        auth: auth.clone(),
    };
    let app = web::router(state, auth, cfg.trusted_proxies.clone());

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    tracing::info!("Bifrost listening on {addr}");
    // `/s/{id}/…` → season-scoped routes: rewritten before routing, so it
    // wraps the whole router. Peer addresses feed the trusted-proxy checks.
    let app = tower::Layer::layer(&axum::middleware::from_fn(scope::rewrite), app);
    axum::serve(
        listener,
        axum::ServiceExt::<axum::extract::Request>::into_make_service_with_connect_info::<
            std::net::SocketAddr,
        >(app),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("serve");
    // Let live seasons' AP servers save and exit before Bifrost does — all at
    // once, so shutdown takes one server's grace period, not one per season.
    let stops = shutdown_db
        .seasons()
        .await
        .into_iter()
        .filter(|s| s.state() == db::State::Live)
        .map(|s| shutdown_seasons.stop(s.id));
    futures_util::future::join_all(stops).await;
    tracing::info!("Bifrost stopped");
}

/// SIGTERM (service manager / `docker stop`) or Ctrl-C.
async fn shutdown_signal() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    // Not `tokio::select!`: rust-analyzer misreads its expansion as making the
    // future non-`Send`, and this one is handed to `axum::serve`.
    let term = std::pin::pin!(term.recv());
    let ctrl_c = std::pin::pin!(tokio::signal::ctrl_c());
    futures_util::future::select(term, ctrl_c).await;
    tracing::info!("shutting down");
}

/// Build the auth state for the configured mode; on a fresh native instance,
/// print the one-time first-run setup link.
async fn init_auth(db: &db::Db) -> auth::Auth {
    let mode = db.auth_mode().await;
    let fwd = match auth::ForwardAuth::from_env(config::get().trusted_proxies.clone()) {
        Ok(f) => f,
        Err(e) if mode == auth::AuthMode::ForwardAuth => panic!("forward-auth config: {e}"),
        Err(_) => auth::ForwardAuth::default(),
    };
    if mode == auth::AuthMode::ForwardAuth && !fwd.configured() {
        tracing::error!(
            "auth mode is forward-auth but no trusted proxy is configured \
             (BIFROST_FWD_SECRET_HEADER/BIFROST_FWD_SECRET or BIFROST_TRUSTED_PROXIES): \
             nobody can sign in"
        );
    }
    let setup_token = (mode == auth::AuthMode::Native && db.account_count().await == 0).then(|| {
        let t = auth::new_token();
        tracing::warn!("no accounts yet — first-run setup: open /setup?token={t}");
        t
    });
    tracing::info!("auth mode: {}", mode.as_str());
    let auth = Arc::new(auth::AuthInner {
        db: db.clone(),
        mode,
        fwd,
        secure_cookies: config::var("BIFROST_INSECURE_COOKIES").as_deref() != Some("1"),
        throttle: auth::Throttle::default(),
        setup_token: Mutex::new(setup_token),
    });
    // Drop expired sessions now and then.
    let purge = db.clone();
    tokio::spawn(async move {
        loop {
            purge.purge_expired_sessions(auth::now()).await;
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });
    auth
}
