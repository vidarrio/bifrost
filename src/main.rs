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
    extract::{Form, Multipart, Path, State},
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
        .route("/game/{ap_slot}", get(game_view))
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
        .route("/admin/clear", post(admin_clear))
        .route("/admin/delete-player", post(admin_delete_player))
        .route("/admin/delete-apworld", post(admin_delete_apworld))
        .route("/admin/console", post(admin_console))
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
    let prios = st.db.priorities(&season).await.unwrap_or_default();
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
    let prios = st.db.priorities(&season).await.unwrap_or_default();

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
    )
}

#[derive(Deserialize)]
struct PrioForm {
    finding: i64,
    location: i64,
    item: i64,
    back: Option<String>,
}

async fn prioritize_post(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<PrioForm>,
) -> Redirect {
    let (_me, _is_admin) = identity(&headers);
    let season = st
        .board
        .read()
        .ok()
        .and_then(|b| b.seed.clone())
        .unwrap_or_default();
    let _ = st
        .db
        .toggle_priority(&season, f.finding, f.location, f.item)
        .await;
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
    back: Option<String>,
}

async fn hint_post(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<HintForm>,
) -> Html<String> {
    let (me, _is_admin) = identity(&headers);
    let back = safe_back(f.back.as_deref(), "/board");
    let detail = st.db.slot_detail(f.slot_id).await.ok().flatten();
    let Some((ap_slot, claimant)) = detail else {
        return render::hint_result("Unknown slot.", false, &back);
    };
    if claimant.as_deref() != Some(me.as_str()) {
        return render::hint_result(
            "Only the player who claimed this slot can hint on it.",
            false,
            &back,
        );
    }
    let connect_name = st
        .board
        .read()
        .ok()
        .and_then(|b| b.slots.get(&ap_slot).map(|s| s.name.clone()));
    let Some(name) = connect_name else {
        return render::hint_result("That slot isn't live right now.", false, &back);
    };
    let Some(cfg) = st.ap_cfg.as_ref() else {
        return render::hint_result("Hinting isn't configured.", false, &back);
    };
    match ap::send_hint(&cfg.url, &cfg.password, &name, f.item.trim()).await {
        Ok(parts) => {
            let text = st
                .board
                .read()
                .map(|b| ap::resolve_parts(&b, &parts))
                .unwrap_or_default();
            render::hint_result(&text, true, &back)
        }
        Err(e) => render::hint_result(&format!("Hint failed: {e}"), false, &back),
    }
}

const UPLOAD_DIR: &str = "/var/lib/bifrost/uploads";
const CUSTOM_WORLDS: &str = "/srv/archipelago/custom_worlds";

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
                        "(command ran; no output)".to_string()
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
