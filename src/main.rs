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
    extract::{Form, Multipart, State},
    http::HeaderMap,
    response::{Html, Redirect},
    routing::{get, post},
};
use serde::Deserialize;
use tower_http::trace::TraceLayer;

mod ap;
mod checker;
mod db;
mod generate;
mod meta;
mod render;

type Shared = Arc<RwLock<ap::Board>>;

#[derive(Clone)]
struct AppState {
    board: Shared,
    db: db::Db,
    ap_cfg: Option<ap::ApConfig>,
    genstate: generate::GenState,
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
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/", get(landing))
        .route("/check", get(check_form).post(check_run))
        .route("/board", get(board_view))
        .route("/claim", get(claim_get).post(claim_post))
        .route("/hint", post(hint_post))
        .route("/signup", get(signup_get).post(signup_post))
        .route("/signup/delete", post(signup_delete))
        .route("/generate", get(generate_get).post(generate_post))
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

#[derive(Deserialize)]
struct HintForm {
    slot_id: i64,
    item: String,
}

async fn hint_post(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<HintForm>,
) -> Html<String> {
    let (me, is_admin) = identity(&headers);
    let detail = st.db.slot_detail(f.slot_id).await.ok().flatten();
    let Some((ap_slot, claimant)) = detail else {
        return render::hint_result("Unknown slot.", false);
    };
    if !(is_admin || claimant.as_deref() == Some(me.as_str())) {
        return render::hint_result("You haven't claimed that slot.", false);
    }
    let connect_name = st
        .board
        .read()
        .ok()
        .and_then(|b| b.slots.get(&ap_slot).map(|s| s.name.clone()));
    let Some(name) = connect_name else {
        return render::hint_result("That slot isn't live right now.", false);
    };
    let Some(cfg) = st.ap_cfg.as_ref() else {
        return render::hint_result("Hinting isn't configured.", false);
    };
    match ap::send_hint(&cfg.url, &cfg.password, &name, f.item.trim()).await {
        Ok(parts) => {
            let text = st
                .board
                .read()
                .map(|b| ap::resolve_parts(&b, &parts))
                .unwrap_or_default();
            render::hint_result(&text, true)
        }
        Err(e) => render::hint_result(&format!("Hint failed: {e}"), false),
    }
}

const UPLOAD_DIR: &str = "/var/lib/bifrost/uploads";

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
    render::signup_page(&subs, &me, is_admin)
}

async fn signup_post(
    State(st): State<AppState>,
    headers: HeaderMap,
    mut mp: Multipart,
) -> Html<String> {
    let (me, _) = identity(&headers);
    let mut yaml: Option<Vec<u8>> = None;
    let mut apworld: Option<Vec<u8>> = None;
    let mut is_custom = false;
    let mut coop = String::new();
    while let Ok(Some(field)) = mp.next_field().await {
        match field.name().unwrap_or("") {
            "yaml" => yaml = field.bytes().await.ok().map(|b| b.to_vec()),
            "apworld" => {
                apworld = field
                    .bytes()
                    .await
                    .ok()
                    .filter(|b| !b.is_empty())
                    .map(|b| b.to_vec());
            }
            "is_custom" => {
                let _ = field.text().await;
                is_custom = true;
            }
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
    let result = match checker::check_yaml(&yaml_str).await {
        Ok(r) => r,
        Err(e) => return render::signup_result(&format!("Checker error: {e}"), false),
    };
    let Some(p0) = result.players.first() else {
        return render::signup_result("Couldn't read a player from that YAML.", false);
    };
    let slot_name = p0.name.clone();
    let game = p0.game.clone().unwrap_or_else(|| "?".into());
    if !result.ok && !is_custom {
        return render::signup_result(
            &format!("That YAML didn't pass: {}", result.errors.join("; ")),
            false,
        );
    }
    if is_custom && apworld.is_none() {
        return render::signup_result("Custom game selected but no .apworld uploaded.", false);
    }

    let slug = slugify(&slot_name);
    let _ = tokio::fs::create_dir_all(UPLOAD_DIR).await;
    let yaml_path = format!("{UPLOAD_DIR}/{slug}.yaml");
    if tokio::fs::write(&yaml_path, &yaml).await.is_err() {
        return render::signup_result("Couldn't store the YAML.", false);
    }
    let apworld_path = if let Some(aw) = &apworld {
        let p = format!("{UPLOAD_DIR}/{slug}.apworld");
        if tokio::fs::write(&p, aw).await.is_err() {
            return render::signup_result("Couldn't store the apworld.", false);
        }
        Some(p)
    } else {
        None
    };
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
            apworld_path.as_deref(),
        )
        .await
    {
        Ok(_) => {
            let note = if !result.ok && is_custom {
                " (custom — validates when the season is generated)"
            } else {
                ""
            };
            render::signup_result(&format!("Signed up: {slot_name} — {game}.{note}"), true)
        }
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

async fn generate_get(State(st): State<AppState>, headers: HeaderMap) -> Html<String> {
    let (_me, is_admin) = identity(&headers);
    let (running, msg) = st
        .genstate
        .lock()
        .map(|g| (g.0, g.1.clone()))
        .unwrap_or((false, "unknown".into()));
    let count = st.db.submissions().await.map(|s| s.len()).unwrap_or(0);
    render::generate_page(&msg, running, count, is_admin)
}

async fn generate_post(State(st): State<AppState>, headers: HeaderMap) -> Redirect {
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
    Redirect::to("/generate")
}
