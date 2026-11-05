//! The season signup wizard.

use super::*;

/// `BIFROST_COOP="Cam:Abi,Foo:Bar"` — map an AP slot name to an extra Bifrost
/// slot name (a co-op seat) for the current season's seeding.
pub(crate) fn parse_coop() -> Vec<(String, String)> {
    config::var("BIFROST_COOP")
        .unwrap_or_default()
        .split(',')
        .filter_map(|p| p.split_once(':'))
        .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
        .filter(|(a, b)| !a.is_empty() && !b.is_empty())
        .collect()
}

/// Season `worlds/` file stems Bifrost itself stages (`generate.rs`).
pub(crate) fn slugify(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    if out.is_empty() { "slot".into() } else { out }
}

pub(crate) async fn signup_get(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
) -> Html<String> {
    let (me, is_admin) = ctx.ident(&me);
    let subs = st.db.submissions(ctx.id).await.unwrap_or_default();
    let open = ctx.state().accepts_signups();
    let worlds = render::SignupWorlds {
        custom_allowed: sandbox::get().custom_worlds_allowed(),
        manual: st.db.approval().await == db::Approval::Manual,
        catalog: st.db.catalog().await,
    };
    render::signup_page(
        &ctx.row.name,
        &subs,
        &me,
        is_admin,
        open,
        matches!(ctx.state(), db::State::Draft | db::State::Signup),
        ctx.row.max_games,
        &worlds,
    )
}

/// The seasons open for signup, each leading to its own page.
pub(crate) async fn signups_list(State(st): State<AppState>, me: auth::Me) -> Html<String> {
    let mut open = Vec::new();
    for s in st.db.seasons().await {
        if !s.state().accepts_signups() {
            continue;
        }
        let mods = st.db.season_mods(s.id).await;
        if scope::can_view(&st, &s, &mods, &me).await {
            let subs = st.db.submissions(s.id).await.unwrap_or_default();
            open.push((s.id, s.name, subs));
        }
    }
    render::signups_list(&open)
}

const NOT_TEXT: &str = "That YAML isn't UTF-8 text. Save it as UTF-8 and try again.";

/// The most a game's notes can hold, in characters.
const MAX_NOTES: usize = 2000;

#[derive(Deserialize)]
pub(crate) struct GameQuery {
    /// Just signed up, so say so.
    #[serde(default)]
    new: bool,
    /// What was just changed, so say so.
    #[serde(default)]
    saved: String,
}

/// A game's page: who's in it and its notes, with forms by viewer and stage.
pub(crate) async fn signup_game(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Path(id): Path<Id>,
    Query(q): Query<GameQuery>,
) -> Response {
    let Some(sub) = st.db.submission(ctx.id, id).await else {
        return Redirect::to(&ctx.url("/signup")).into_response();
    };
    let is_mod = ctx.can_manage(&me);
    let is_host = sub.account_id == me.id();
    let state = ctx.state();
    let pre_gen = matches!(state, db::State::Draft | db::State::Signup);
    let seats = st.db.seats_of(id).await;
    let my_seat = seats.iter().find(|s| s.account_id == me.id()).cloned();
    let replace = (is_host && state.accepts_signups()) || (is_mod && pre_gen);
    let welcome = (q.new && is_host && pre_gen).then(|| match &sub.held_for {
        Some(w) => format!(
            "Signed up: {} — held until a moderator approves {w}; your YAML is checked then. If you're sharing this game, invite your co-op partners below.",
            sub.slot_name
        ),
        None => format!(
            "Signed up: {} — {}. If you're sharing this game, invite your co-op partners below.",
            sub.slot_name, sub.game
        ),
    });
    let saved = match q.saved.as_str() {
        "notes" => Some("Notes saved.".to_string()),
        "yaml" => Some(format!("YAML replaced: {} — {}.", sub.slot_name, sub.game)),
        _ => None,
    };
    let invites =
        (is_host && pre_gen).then(|| (format!("signup/game/{id}"), format!("/s/{}/seat/", ctx.id)));
    let links = if invites.is_some() {
        st.db.seat_links_of_signup(id).await
    } else {
        Vec::new()
    };
    render::signup_game_page(&render::SignupGame {
        sub: &sub,
        seats: &seats,
        my_seat: my_seat.as_ref().filter(|_| pre_gen),
        is_host,
        pre_gen,
        edit_notes: is_mod || (pre_gen && (is_host || my_seat.is_some())),
        show_reason: is_mod || is_host || my_seat.is_some(),
        replace,
        welcome,
        saved,
        invite: invites
            .as_ref()
            .map(|(action, base)| render::invite_box(action, base, &links))
            .unwrap_or_default(),
        max_notes: MAX_NOTES,
        live: ctx.state().is_live(),
    })
    .into_response()
}

#[derive(Deserialize)]
pub(crate) struct NotesForm {
    notes: String,
}

pub(crate) async fn signup_game_notes(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Path(id): Path<Id>,
    Form(f): Form<NotesForm>,
) -> Response {
    // Browsers send line breaks as CRLF but count them as one.
    let notes = f.notes.replace("\r\n", "\n");
    let notes = notes.trim();
    if notes.chars().count() > MAX_NOTES {
        return render::signup_result(
            &format!("Notes can be at most {MAX_NOTES} characters."),
            false,
        )
        .into_response();
    }
    match st
        .db
        .set_submission_notes(ctx.id, id, me.id(), ctx.can_manage(&me), notes)
        .await
    {
        Ok(true) => {
            Redirect::to(&ctx.url(&format!("/signup/game/{id}?saved=notes"))).into_response()
        }
        _ => render::signup_result("You can't change these notes.", false).into_response(),
    }
}

/// Replace a game's YAML: the new file has to pass like a new signup's. Its
/// name and game can change; everything else about the game stays.
pub(crate) async fn signup_game_yaml(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Path(id): Path<Id>,
    mut mp: Multipart,
) -> Response {
    let closed = "You can't replace this YAML: it isn't yours, or signups are closed.";
    let refuse = |text: &str| render::signup_result(text, false).into_response();
    let Some(sub) = st.db.submission(ctx.id, id).await else {
        return refuse(closed);
    };
    let is_mod = ctx.can_manage(&me);
    let pre_gen = matches!(ctx.state(), db::State::Draft | db::State::Signup);
    if !((sub.account_id == me.id() && ctx.state().accepts_signups()) || (is_mod && pre_gen)) {
        return refuse(closed);
    }
    let mut yaml: Option<Vec<u8>> = None;
    while let Ok(Some(field)) = mp.next_field().await {
        if field.name() == Some("yaml") {
            yaml = field.bytes().await.ok().map(|b| b.to_vec());
        } else {
            let _ = field.bytes().await;
        }
    }
    let Some(yaml) = yaml.filter(|b| !b.is_empty()) else {
        return refuse("No YAML uploaded.");
    };
    let Ok(yaml_str) = String::from_utf8(yaml.clone()) else {
        return refuse(NOT_TEXT);
    };
    // Its world still waiting: it can't be checked yet, so it gets the first look
    // and stays held. Otherwise it has to pass like a new signup.
    let waiting = match &sub.held_for {
        Some(world) => st.db.world_request(ctx.id, world).await,
        None => None,
    };
    let (slot_name, game) = if let Some(request) = &waiting {
        if yaml.len() > checker::MAX_YAML {
            return refuse("That YAML is too large.");
        }
        let look = match checker::first_look(&ctx.dirs(), &yaml_str).await {
            Ok(look) => look,
            Err(e) => return refuse(&format!("Checker error: {e}")),
        };
        match worlds::judge_first_look(&look, &request.game) {
            Ok(found) => found,
            Err(e) => return refuse(&e),
        }
    } else {
        let result = match checker::check_yaml(&ctx.dirs(), &yaml_str).await {
            Ok(r) => r,
            Err(e) => return refuse(&format!("Checker error: {e}")),
        };
        if !result.ok {
            return refuse(&format!(
                "That YAML didn't pass: {}.",
                result.errors.join("; ")
            ));
        }
        if result.players.len() > 1 {
            return refuse(worlds::ONE_GAME_PER_FILE);
        }
        let Some(p0) = result.players.first() else {
            return refuse("Couldn't read a player from that YAML.");
        };
        match worlds::slot_name_of_player(p0) {
            Ok(name) => (name, p0.game.clone().unwrap_or_else(|| "?".into())),
            Err(e) => return refuse(&e),
        }
    };
    // Its own name is fine, in any case.
    if slot_name.to_lowercase() != sub.slot_name.to_lowercase()
        && st.db.slot_name_taken(ctx.id, &slot_name).await
    {
        return refuse(&slot_taken(&slot_name));
    }
    let Some(yaml_path) = store_yaml(&ctx, &slot_name, &yaml).await else {
        return refuse("Couldn't store the YAML.");
    };
    let replaced = st
        .db
        .replace_submission(
            ctx.id,
            id,
            me.id(),
            is_mod,
            waiting.is_some(),
            &slot_name,
            &game,
            &sub.yaml_file,
            &yaml_path,
        )
        .await;
    let page = match replaced {
        Ok(true) => {
            remove_yaml(&sub.yaml_file).await;
            if let Some(req) = &waiting {
                worlds::settle_held(&st.db, ctx.id, id, &yaml_path, &req.file).await;
            }
            if sub.account_id != me.id() {
                tracing::info!(
                    "season {}: {} replaced the YAML of {} ({} → {})",
                    ctx.id,
                    me.username(),
                    sub.username,
                    sub.slot_name,
                    slot_name
                );
            }
            return Redirect::to(&ctx.url(&format!("/signup/game/{id}?saved=yaml")))
                .into_response();
        }
        Ok(false) => refuse(closed),
        Err(e) if db::name_clash(&e) => refuse(&slot_taken(&slot_name)),
        Err(e) => {
            tracing::warn!("replacing a signup in season {}: {e}", ctx.id);
            refuse("Couldn't store the signup.")
        }
    };
    remove_yaml(&yaml_path).await;
    page
}

/// Whether this player already has as many games as the season allows. An early
/// answer, so a refused player installs nothing and spends no check; the insert
/// in `add_submission` is what holds the limit.
async fn at_games_limit(st: &AppState, ctx: &scope::SeasonCtx, account: i64) -> Option<String> {
    let limit = ctx.row.max_games;
    (limit > 0 && st.db.games_of(ctx.id, account).await >= limit).then(|| limit_message(limit))
}

fn limit_message(limit: i64) -> String {
    format!("You've reached this season's limit of {limit} game(s) per player.")
}

/// Step 1 of signup: put a custom world in the season — a catalog pick
/// (`catalog` = its hash) or an uploaded `.apworld` — so step 2 can check a
/// YAML against it. An upload may instead wait for approval (`pending`), in
/// which case the YAML is held rather than checked. Returns JSON for the
/// wizard.
pub(crate) async fn signup_apworld(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    mut mp: Multipart,
) -> Json<serde_json::Value> {
    let fail = |e: &str| Json(serde_json::json!({"ok": false, "error": e}));
    if let Some(e) = at_games_limit(&st, &ctx, me.id()).await {
        return fail(&e);
    }
    // Worlds change only while the season takes signups — never under a
    // season that's being played.
    if !ctx.state().accepts_signups() {
        return fail("Signups are closed right now.");
    }
    let mut bytes: Option<Vec<u8>> = None;
    let mut fname = String::new();
    let mut catalog = String::new();
    while let Ok(Some(field)) = mp.next_field().await {
        match field.name() {
            Some("apworld") => {
                fname = field.file_name().unwrap_or("").to_string();
                bytes = field.bytes().await.ok().map(|b| b.to_vec());
            }
            Some("catalog") => catalog = field.text().await.unwrap_or_default(),
            _ => {
                let _ = field.bytes().await;
            }
        }
    }
    if !sandbox::get().custom_worlds_allowed() {
        return fail(
            "Custom apworlds are disabled on this server (no sandbox available to run them safely). Ask an admin.",
        );
    }
    let (uploader, _) = ctx.ident(&me);
    if !catalog.is_empty() {
        return match worlds::install_from_catalog(&st.db, ctx.id, catalog.trim(), &uploader).await {
            Ok(file) => Json(serde_json::json!({"ok": true, "world": file, "pending": false})),
            Err(e) => fail(&e),
        };
    }
    let Some(bytes) = bytes.filter(|b| !b.is_empty()) else {
        return fail("No file received.");
    };
    if !worlds::looks_like_apworld(&bytes) {
        return fail("That doesn't look like a valid .apworld (zip) file.");
    }
    let file = match worlds::upload_file(&fname) {
        Ok(f) => f,
        Err(e) => return fail(e),
    };
    match worlds::place_upload(&st.db, ctx.id, &file, bytes, &uploader).await {
        Ok(placed) => Json(serde_json::json!({
            "ok": true, "world": file, "pending": placed == worlds::Placed::Pending
        })),
        Err(e) => {
            tracing::info!("apworld upload refused: {e}");
            fail(&e)
        }
    }
}

/// Step 2 of signup: validate a YAML live against the installed worlds (incl. an
/// apworld just installed in step 1). Returns JSON for the wizard.
pub(crate) async fn signup_check(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    mut mp: Multipart,
) -> Json<serde_json::Value> {
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
    let Ok(yaml_str) = String::from_utf8(yaml) else {
        return Json(serde_json::json!({"ok": false, "error": NOT_TEXT}));
    };
    match checker::check_yaml(&ctx.dirs(), &yaml_str).await {
        Ok(r) => {
            let (name, game) = r
                .players
                .first()
                .map(|p| (p.name.clone(), p.game.clone().unwrap_or_default()))
                .unwrap_or_default();
            let name = name.trim().to_string();
            let taken = r.ok && st.db.slot_name_taken(ctx.id, &name).await;
            let refusal = if r.ok && r.players.len() > 1 {
                Some(worlds::ONE_GAME_PER_FILE.to_string())
            } else if let (true, Some(Err(e))) =
                (r.ok, r.players.first().map(worlds::slot_name_of_player))
            {
                Some(e)
            } else {
                taken.then(|| slot_taken(&name))
            };
            match refusal {
                Some(e) => {
                    Json(serde_json::json!({"ok": false, "name": name, "game": game, "error": e}))
                }
                None => Json(serde_json::json!({
                    "ok": r.ok, "name": name, "game": game, "errors": r.errors
                })),
            }
        }
        Err(e) => Json(serde_json::json!({"ok": false, "error": e})),
    }
}

/// Write a signup's YAML to a new file of its own in the season's folder.
/// Returns how the database names it: by its place in the data dir (see
/// `Config::data_file`).
async fn store_yaml(ctx: &scope::SeasonCtx, slot_name: &str, yaml: &[u8]) -> Option<String> {
    // Names can be long, and file names can't.
    let slug: String = slugify(slot_name).chars().take(64).collect();
    let uploads = ctx.dirs().submissions;
    let _ = tokio::fs::create_dir_all(&uploads).await;
    let nonce = &auth::new_token()[..16];
    let yaml_path = uploads.join(format!("{slug}-{nonce}.yaml"));
    let name = config::get().data_name(&yaml_path)?;
    // `create_new`: what's already there isn't ours, so leave it alone.
    let mut f = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&yaml_path)
        .await
        .ok()?;
    if tokio::io::AsyncWriteExt::write_all(&mut f, yaml)
        .await
        .is_err()
    {
        let _ = tokio::fs::remove_file(&yaml_path).await;
        return None;
    }
    Some(name)
}

/// Remove a signup's stored YAML, by the name the database has for it.
pub(crate) async fn remove_yaml(name: &str) {
    if let Some(p) = config::get().data_file(name) {
        let _ = tokio::fs::remove_file(p).await;
    }
}

fn slot_taken(name: &str) -> String {
    format!(
        "The slot name '{name}' is already taken in this season. Each game needs its own: change name: in the YAML and try again. To replace your own signup, upload the new file on its page."
    )
}

pub(crate) async fn signup_post(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    mut mp: Multipart,
) -> Response {
    let refuse = |text: &str| render::signup_result(text, false).into_response();
    let my_id = me.id();
    let (_me, _) = ctx.ident(&me);
    if !ctx.state().accepts_signups() {
        return refuse("Signups are closed right now.");
    }
    if let Some(e) = at_games_limit(&st, &ctx, my_id).await {
        return refuse(&e);
    }
    let mut yaml: Option<Vec<u8>> = None;
    let mut is_custom = false;
    let mut pending_world = String::new();
    while let Ok(Some(field)) = mp.next_field().await {
        match field.name().unwrap_or("") {
            "yaml" => yaml = field.bytes().await.ok().map(|b| b.to_vec()),
            "is_custom" => is_custom = field.text().await.unwrap_or_default() == "1",
            "pending_world" => pending_world = field.text().await.unwrap_or_default(),
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    let Some(yaml) = yaml.filter(|b| !b.is_empty()) else {
        return refuse("No YAML uploaded.");
    };
    let Ok(yaml_str) = String::from_utf8(yaml.clone()) else {
        return refuse(NOT_TEXT);
    };
    let held_for = pending_world.trim().to_string();
    let (slot_name, game) = if !held_for.is_empty() {
        // Its world waits for approval, so the YAML can't be checked yet: hold
        // the signup under the name it declares; it's checked on approval.
        let Some(request) = st.db.world_request(ctx.id, &held_for).await else {
            return refuse("That apworld isn't waiting for approval any more — start over.");
        };
        if yaml.len() > checker::MAX_YAML {
            return refuse("That YAML is too large.");
        }
        // A first look, so the player hears now; the check on approval decides.
        let look = match checker::first_look(&ctx.dirs(), &yaml_str).await {
            Ok(look) => look,
            Err(e) => return refuse(&format!("Checker error: {e}")),
        };
        match worlds::judge_first_look(&look, &request.game) {
            Ok(found) => found,
            Err(e) => return refuse(&e),
        }
    } else {
        // Trust-but-verify: the wizard already checked, but re-roll server-side so a
        // hand-crafted POST can't slip an invalid YAML through. Custom worlds are
        // installed by now, so everything must pass.
        let result = match checker::check_yaml(&ctx.dirs(), &yaml_str).await {
            Ok(r) => r,
            Err(e) => return refuse(&format!("Checker error: {e}")),
        };
        let Some(p0) = result.players.first() else {
            return refuse("Couldn't read a player from that YAML.");
        };
        if !result.ok {
            return refuse(&format!(
                "That YAML didn't pass: {}. Custom game? Install its apworld in step 1 first.",
                result.errors.join("; ")
            ));
        }
        // Every document in a file becomes a slot, but a signup counts as one game.
        if result.players.len() > 1 {
            return refuse(worlds::ONE_GAME_PER_FILE);
        }
        match worlds::slot_name_of_player(p0) {
            Ok(name) => (name, p0.game.clone().unwrap_or_else(|| "?".into())),
            Err(e) => return refuse(&e),
        }
    };

    if st.db.slot_name_taken(ctx.id, &slot_name).await {
        return refuse(&slot_taken(&slot_name));
    }

    // A file of its own, never another signup's: names that slugify alike
    // (`Ann B`, `Ann-B`) or a refused duplicate must not touch an existing one.
    let Some(yaml_path) = store_yaml(&ctx, &slot_name, &yaml).await else {
        return refuse("Couldn't store the YAML.");
    };
    match st
        .db
        .add_submission(
            ctx.id,
            my_id,
            &slot_name,
            &game,
            is_custom,
            &yaml_path,
            (!held_for.is_empty()).then_some(held_for.as_str()),
            ctx.row.max_games,
        )
        .await
    {
        Ok(None) => {
            remove_yaml(&yaml_path).await;
            refuse(&limit_message(ctx.row.max_games))
        }
        // The game's page is where the rest happens: its invite links, notes and YAML.
        Ok(Some(id)) => {
            if !held_for.is_empty() {
                worlds::settle_held(&st.db, ctx.id, id, &yaml_path, &held_for).await;
            }
            Redirect::to(&ctx.url(&format!("/signup/game/{id}?new=true"))).into_response()
        }
        Err(e) => {
            remove_yaml(&yaml_path).await;
            if db::name_clash(&e) {
                refuse(&slot_taken(&slot_name))
            } else {
                tracing::warn!("storing a signup in season {}: {e}", ctx.id);
                refuse("Couldn't store the signup.")
            }
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct DelForm {
    pub(crate) id: Id,
}

pub(crate) async fn signup_delete(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<DelForm>,
) -> Redirect {
    let my_id = me.id();
    let (_me, is_admin) = ctx.ident(&me);
    if let Ok(Some(yaml_file)) = st.db.delete_submission(ctx.id, f.id, my_id, is_admin).await {
        remove_yaml(&yaml_file).await;
    }
    Redirect::to(&ctx.url("/signup"))
}

#[derive(Deserialize)]
pub(crate) struct TokenForm {
    token: String,
}

/// The host's game before generation, or a redirect back if it isn't theirs.
async fn hosted(
    st: &AppState,
    ctx: &scope::SeasonCtx,
    me: &auth::Me,
    id: Id,
) -> Option<db::SubmissionRow> {
    st.db
        .submission(ctx.id, id)
        .await
        .filter(|s| s.account_id == me.id())
        .filter(|_| matches!(ctx.state(), db::State::Draft | db::State::Signup))
}

pub(crate) async fn signup_game_invite(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Path(id): Path<Id>,
) -> Redirect {
    if hosted(&st, &ctx, &me, id).await.is_some() {
        let _ = st
            .db
            .add_seat_link(ctx.id, &auth::new_token(), Some(id), None, me.id())
            .await;
    }
    Redirect::to(&ctx.url(&format!("/signup/game/{id}")))
}

pub(crate) async fn signup_game_revoke(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Path(id): Path<Id>,
    Form(f): Form<TokenForm>,
) -> Redirect {
    if hosted(&st, &ctx, &me, id).await.is_some()
        && st.db.seat_links_of_signup(id).await.contains(&f.token)
    {
        let _ = st.db.revoke_seat_link(ctx.id, &f.token).await;
    }
    Redirect::to(&ctx.url(&format!("/signup/game/{id}")))
}

#[derive(Deserialize)]
pub(crate) struct SeatForm {
    seat: Id,
    #[serde(default)]
    name: String,
}

pub(crate) async fn signup_seat_remove(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Path(id): Path<Id>,
    Form(f): Form<SeatForm>,
) -> Redirect {
    let _ = st.db.remove_seat(ctx.id, f.seat, me.id()).await;
    Redirect::to(&ctx.url(&format!("/signup/game/{id}")))
}

pub(crate) async fn signup_seat_leave(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Path(id): Path<Id>,
) -> Redirect {
    let _ = st.db.leave_seat(ctx.id, id, me.id()).await;
    // Not back to the game: leaving may mean they can't see this season any more.
    Redirect::to("/signups")
}

pub(crate) async fn signup_seat_rename(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Path(id): Path<Id>,
    Form(f): Form<SeatForm>,
) -> Response {
    let name = f.name.trim();
    if let Some(e) = db::seat_name_error(name) {
        return render::signup_result(e, false).into_response();
    }
    // Its own name is fine, in any case.
    let own = st
        .db
        .seats_of(id)
        .await
        .iter()
        .any(|s| s.id == f.seat && s.name.to_lowercase() == name.to_lowercase());
    if !own && st.db.slot_name_taken(ctx.id, name).await {
        return render::signup_result(&name_taken(name), false).into_response();
    }
    match st.db.rename_seat(ctx.id, f.seat, me.id(), name).await {
        Ok(db::JoinOutcome::Joined) => {
            Redirect::to(&ctx.url(&format!("/signup/game/{id}"))).into_response()
        }
        Ok(db::JoinOutcome::NameTaken) => {
            render::signup_result(&name_taken(name), false).into_response()
        }
        _ => render::signup_result("You can't rename that seat now.", false).into_response(),
    }
}

fn name_taken(name: &str) -> String {
    format!("The name '{name}' is already taken in this season. Pick another.")
}

/// What a seat link joins: its host's name, the game, and on a live season the
/// AP slot a new seat goes on.
struct Invite {
    host: String,
    game: String,
    signup: Option<db::SubmissionRow>,
    /// On a live season: the AP slot and game a new seat goes on.
    live: Option<(i64, Id)>,
}

async fn invite_of(st: &AppState, ctx: &scope::SeasonCtx, link: &db::SeatLink) -> Option<Invite> {
    let signup = match link.submission_id {
        Some(id) => Some(st.db.submission(ctx.id, id).await?),
        None => None,
    };
    let slot_name = signup
        .as_ref()
        .map(|s| s.slot_name.clone())
        .or_else(|| link.slot_name.clone())?;
    if ctx.state().is_live() {
        let slots = st.db.slots(ctx.id).await.ok()?;
        let row = slots.iter().find(|r| r.name == slot_name)?;
        return Some(Invite {
            host: row.claimed_by.clone().unwrap_or(slot_name),
            game: row.game.clone(),
            signup: None,
            live: Some((row.ap_slot, row.game_id)),
        });
    }
    if !matches!(ctx.state(), db::State::Draft | db::State::Signup) {
        return None;
    }
    let sub = signup?;
    Some(Invite {
        host: sub.username.clone(),
        game: sub.game.clone(),
        signup: Some(sub),
        live: None,
    })
}

/// A seat link: who's inviting you to what, and a box for your seat's name.
pub(crate) async fn seat_get(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    _me: auth::Me,
    Path(token): Path<String>,
) -> Html<String> {
    let gone = "This link has been used or revoked, or the season can't take new players right now. Ask for a new one.";
    let Some(link) = st.db.seat_link(ctx.id, &token).await else {
        return render::signup_result(gone, false);
    };
    match invite_of(&st, &ctx, &link).await {
        Some(i) => render::seat_page(&i.host, &i.game, &ctx.row.name, &token),
        None => render::signup_result(gone, false),
    }
}

#[derive(Deserialize)]
pub(crate) struct JoinForm {
    name: String,
}

pub(crate) async fn seat_post(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Path(token): Path<String>,
    Form(f): Form<JoinForm>,
) -> Response {
    let gone = "This link has been used or revoked, or the season can't take new players right now. Ask for a new one.";
    let refuse = |text: &str| render::signup_result(text, false).into_response();
    let name = f.name.trim();
    if let Some(e) = db::seat_name_error(name) {
        return refuse(e);
    }
    let Some(link) = st.db.seat_link(ctx.id, &token).await else {
        return refuse(gone);
    };
    let Some(invite) = invite_of(&st, &ctx, &link).await else {
        return refuse(gone);
    };
    if st.db.slot_name_taken(ctx.id, name).await {
        return refuse(&name_taken(name));
    }
    let joined = match (&invite.signup, invite.live) {
        (Some(sub), _) => st.db.join_signup(ctx.id, &token, sub, me.id(), name).await,
        (None, Some((ap, game_id))) => {
            st.db
                .join_live(ctx.id, &token, ap, game_id, &invite.game, me.id(), name)
                .await
        }
        _ => return refuse(gone),
    };
    match joined {
        Ok(db::JoinOutcome::Joined) => {
            let to = match (&invite.signup, invite.live) {
                (Some(sub), _) => format!("/signup/game/{}", sub.id),
                (_, Some((_, game_id))) => format!("/game/{game_id}"),
                _ => "/signup".to_string(),
            };
            Redirect::to(&ctx.url(&to)).into_response()
        }
        Ok(db::JoinOutcome::NameTaken) => refuse(&name_taken(name)),
        Ok(db::JoinOutcome::AlreadyIn) => refuse("You're already playing this game."),
        Ok(db::JoinOutcome::LinkGone) => refuse(gone),
        Err(e) => {
            tracing::warn!("joining through a seat link in season {}: {e}", ctx.id);
            refuse("Couldn't add you to the game.")
        }
    }
}

/// A private season's signup link: it lets you in, and on to its signup page.
pub(crate) async fn signup_join(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Path(token): Path<String>,
) -> Response {
    let valid = ctx.row.private_signup
        && matches!(ctx.state(), db::State::Draft | db::State::Signup)
        && ctx.row.signup_token.as_deref() == Some(token.as_str());
    if !valid {
        return render::signup_result(
            "This signup link isn't valid any more. Ask for a new one.",
            false,
        )
        .into_response();
    }
    if st.db.add_season_invited(ctx.id, me.id()).await.is_err() {
        return render::signup_result("Couldn't open the season's signup.", false).into_response();
    }
    Redirect::to(&ctx.url("/signup")).into_response()
}
