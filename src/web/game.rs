//! A slot's game page: PopTracker, hints, items, files, finishing.

use super::*;

/// Whether the viewer may upload/remove the slot's pack: a claimant of the slot
/// or an admin.
pub(crate) async fn pt_can_manage(
    st: &AppState,
    season: i64,
    ap_slot: i64,
    account: i64,
    is_admin: bool,
) -> bool {
    if is_admin {
        return true;
    }
    st.db
        .slots(season)
        .await
        .unwrap_or_default()
        .iter()
        .any(|r| r.ap_slot == ap_slot && r.claimed_by_id == Some(account))
}

/// Claimant (or admin) uploads a PopTracker pack (.zip) for this slot.
pub(crate) async fn pt_upload(
    Path(ap_slot): Path<i64>,
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    mut mp: Multipart,
) -> Response {
    let my_id = me.id();
    let (_me, is_admin) = ctx.ident(&me);
    let season = ctx.id;
    // Land back on the PopTracker tab (the pack-management tab), not the default.
    let back = ctx.url(&format!("/game/{ap_slot}#tab-pt"));
    if !ctx.state().is_live() || !pt_can_manage(&st, season, ap_slot, my_id, is_admin).await {
        return render::notice("You can't upload a pack for this game.", false, &back)
            .into_response();
    }
    let mut bytes: Option<Vec<u8>> = None;
    while let Ok(Some(field)) = mp.next_field().await {
        if field.name() == Some("pack") {
            bytes = field.bytes().await.ok().map(|b| b.to_vec());
        } else {
            let _ = field.bytes().await;
        }
    }
    let Some(bytes) = bytes.filter(|b| !b.is_empty()) else {
        return render::notice("No file received.", false, &back).into_response();
    };
    if bytes.len() < 2 || &bytes[..2] != b"PK" {
        return render::notice("That doesn't look like a .zip pack file.", false, &back)
            .into_response();
    }
    // Validation runs the pack's Lua, in a worker; a pack that fails it, or runs
    // too long, is never installed.
    let Some(_claim) = ptrack::claim_upload(my_id) else {
        return render::notice("You're already uploading a pack.", false, &back).into_response();
    };
    let bytes = std::sync::Arc::new(bytes);
    let summary = match ptrack::validate(bytes.clone()).await {
        Ok(summary) => summary,
        Err(e) => {
            return render::notice(&format!("Pack rejected: {e}"), false, &back).into_response();
        }
    };
    match ptrack::install_pack(season, ap_slot, bytes, summary.variant).await {
        Ok(()) => Redirect::to(&back).into_response(),
        Err(e) => render::notice(&format!("Pack rejected: {e}"), false, &back).into_response(),
    }
}

/// Claimant (or admin) removes the slot's pack.
pub(crate) async fn pt_remove(
    Path(ap_slot): Path<i64>,
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
) -> Response {
    let my_id = me.id();
    let (_me, is_admin) = ctx.ident(&me);
    let season = ctx.id;
    let back = ctx.url(&format!("/game/{ap_slot}#tab-pt"));
    if ctx.state().is_live() && pt_can_manage(&st, season, ap_slot, my_id, is_admin).await {
        let s2 = season;
        let _ = tokio::task::spawn_blocking(move || ptrack::remove(s2, ap_slot)).await;
    }
    Redirect::to(&back).into_response()
}

struct SharedText(std::sync::Arc<String>);

impl AsRef<[u8]> for SharedText {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

/// Computed tracker state (maps + markers + items) for the slot's pack, as JSON
/// for the PopTracker tab + its poller. `{pack:false}` when no pack is installed.
pub(crate) async fn pt_state(Path(ap_slot): Path<i64>, ctx: scope::SeasonCtx) -> Response {
    let season = ctx.id;
    let Some((game, received, checked, meta)) = ctx.rt.with_board(|b| {
        let (game, received, checked) = b
            .slots
            .get(&ap_slot)
            .map(|s| {
                (
                    s.game.clone(),
                    s.received_items.clone(),
                    s.checked_locations.clone(),
                )
            })
            .unwrap_or_default();
        (game, received, checked, b.meta.clone())
    }) else {
        return Json(serde_json::json!({"pack": false})).into_response();
    };
    if !ctx.state().is_live() || !ptrack::has_pack(season, ap_slot) {
        return Json(serde_json::json!({"pack": false})).into_response();
    }
    let name_of = |item: i64| {
        meta.as_ref()
            .and_then(|m| m.item_name(&game, item))
            .map(str::to_string)
            .unwrap_or_else(|| item.to_string())
    };
    // The slot's starting inventory comes first (as it does on an AP connect),
    // then the items it has since received. Precollected items aren't in the
    // savefile's received list, so without this, starting gates (e.g. a season)
    // read false and their checks show blocked when they shouldn't.
    let precollected: Vec<i64> = meta
        .as_ref()
        .and_then(|m| m.totals(ap_slot))
        .map(|s| s.precollected.clone())
        .unwrap_or_default();
    let items: Vec<(i64, String)> = precollected
        .iter()
        .map(|item| (*item, name_of(*item)))
        .chain(
            received
                .iter()
                .map(|(item, _loc, _finder)| (*item, name_of(*item))),
        )
        .collect();
    // The slot's full location set (static per season) drives `visibility_rules`
    // so the tracker hides checks this slot's options never generated.
    let valid_locations: Vec<i64> = meta
        .as_ref()
        .and_then(|m| m.totals(ap_slot))
        .map(|s| s.all_locations.clone())
        .unwrap_or_default();
    let version = ptrack::pack_version(season, ap_slot);
    // What AP tells the slot's clients on connect: its slot data and the
    // multiworld's players.
    let connect = (meta.as_deref())
        .map(|m| ptrack::Connect::from_meta(m, ap_slot))
        .unwrap_or_default();
    let feed = ptrack::Feed {
        items: &items,
        checked: &checked,
        valid_locations: &valid_locations,
        player: ap_slot,
        connect,
    };
    match ptrack::compute(season, ap_slot, version, feed).await {
        // The state is shared by every response, not copied into each: a client
        // that doesn't read its response shouldn't hold memory of its own.
        Ok(state) => {
            let parts = [
                axum::body::Bytes::from(format!(r#"{{"pack":true,"version":{version},"state":"#)),
                axum::body::Bytes::from_owner(SharedText(state)),
                axum::body::Bytes::from_static(b"}"),
            ];
            let chunks = futures_util::stream::iter(parts.map(Ok::<_, std::convert::Infallible>));
            (
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                axum::body::Body::from_stream(chunks),
            )
                .into_response()
        }
        Err(ptrack::ComputeError::Busy) => {
            Json(serde_json::json!({"pack": true, "busy": true})).into_response()
        }
        Err(ptrack::ComputeError::Failed(e)) => {
            Json(serde_json::json!({"pack": true, "error": e})).into_response()
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct PtAssetQuery {
    pub(crate) file: String,
    /// Pack version stamp for cache-busting (see `ptrack::pack_version`). When
    /// present the URL is unique per pack install, so the asset is immutable;
    /// when absent (an old cached page) we force revalidation so a stale image
    /// (e.g. a palette PNG's magenta key) can't keep showing.
    #[serde(default)]
    pub(crate) v: Option<String>,
}

/// Serve a file (e.g. a map image) from the slot's installed pack.
pub(crate) async fn pt_asset(
    Path(ap_slot): Path<i64>,
    ctx: scope::SeasonCtx,
    Query(q): Query<PtAssetQuery>,
) -> Response {
    let season = ctx.id;
    let file = q.file.clone();
    let season2 = season;
    let bytes =
        tokio::task::spawn_blocking(move || ptrack::read_pack_asset(season2, ap_slot, &file))
            .await
            .ok()
            .flatten();
    let cache = if q.v.is_some() {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    match bytes {
        Some(b) => (
            [
                (axum::http::header::CONTENT_TYPE, pt_content_type(&q.file)),
                (axum::http::header::CACHE_CONTROL, cache),
            ],
            b,
        )
            .into_response(),
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

pub(crate) fn pt_content_type(f: &str) -> &'static str {
    let l = f.to_ascii_lowercase();
    if l.ends_with(".png") {
        "image/png"
    } else if l.ends_with(".jpg") || l.ends_with(".jpeg") {
        "image/jpeg"
    } else if l.ends_with(".gif") {
        "image/gif"
    } else if l.ends_with(".webp") {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}

pub(crate) async fn game_view(
    Path(ap_slot): Path<i64>,
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
) -> Html<String> {
    let my_id = me.id();
    let (me, is_admin) = ctx.ident(&me);
    let (snap, game, item_names, connect_name) = match ctx.rt.with_board(|b| {
        let (game, cn) = b
            .slots
            .get(&ap_slot)
            .map(|s| (s.game.clone(), s.name.clone()))
            .unwrap_or_default();
        // Only the slot's own items that still have an unhinted/unfound copy.
        let items = b.hint_suggestions(ap_slot, &game);
        (b.snapshot(), game, items, cn)
    }) {
        Some(v) => v,
        None => return Html("board unavailable".into()),
    };
    if game.is_empty() {
        return Html("<p>Unknown game slot.</p>".into());
    }
    let season = ctx.id;
    let slots = st.db.slots(season).await.unwrap_or_default();
    let prios = live_priorities(&st, &snap, season).await;

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
        .any(|r| r.ap_slot == ap_slot && r.claimed_by_id == Some(my_id));
    // The common game shows its own shared points to everyone; personal slots
    // only to their claimant.
    let is_common_slot = slots.iter().any(|r| r.ap_slot == ap_slot && r.is_common);
    // Communal hinting requires skin in the game: the viewer must own a regular
    // (non-common) slot this season to spend the common game's shared points.
    let i_have_claim = slots
        .iter()
        .any(|r| !r.is_common && r.claimed_by_id == Some(my_id));
    let points = match (i_claim || is_common_slot, &ctx.rt.ap_cfg()) {
        (true, Some(cfg)) if !connect_name.is_empty() => {
            ap::slot_points(&cfg.url, &cfg.password, &connect_name).await
        }
        _ => None,
    };
    let credits = if i_claim {
        st.db.credits(season, my_id).await
    } else {
        0
    };
    // Generated files for THIS slot, shown only to a claimant of it (or admin).
    // Don't even read the zip for anyone else.
    let my_files = if i_claim || is_admin {
        tokio::task::spawn_blocking(move || generate::list_outputs(season))
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|o| o.player == Some(ap_slot))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    // The viewer's own registered Minecraft username, for the common-game page's
    // whitelist form (only read/used there).
    let my_mc = st.db.mc_username(my_id).await;

    // Cached Universal-Tracker in-logic locations for this slot (computed by the
    // inlogic recomputer; may be absent until the first run completes).
    let inlogic = st.db.inlogic_get(season, ap_slot).await;

    // This slot's own activity (sent/received/goaled…) for its paginated "Log"
    // tab: first page of rows + the total for the pager (unfiltered).
    let log = st
        .db
        .activity_for_participant(season, &connect_name, "", 0, render::GAME_LOG_RPP)
        .await;
    let log_total = st
        .db
        .activity_count_for_participant(season, &connect_name, "")
        .await;

    // PopTracker: is a pack installed, and may the viewer manage it?
    let pt_has = ptrack::has_pack(season, ap_slot);
    let pt_can_manage = i_claim || is_admin;

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
        i_have_claim,
        my_mc.as_deref(),
        mc_addr().as_deref(),
        &my_files,
        inlogic.as_ref(),
        &log,
        log_total,
        pt_has,
        pt_can_manage,
    )
}

#[derive(Deserialize)]
pub(crate) struct LogPageQuery {
    #[serde(default)]
    pub(crate) page: Option<i64>,
    #[serde(default)]
    pub(crate) rpp: Option<i64>,
    #[serde(default)]
    pub(crate) q: Option<String>,
}

/// Live-refresh / pagination / search fragment for a game page's Log tab: one
/// page of the `#gamelog` inner HTML (feed + pager), scoped to that slot's
/// participant (sender or receiver) and narrowed by the search term. Public like
/// the board/log fragments.
pub(crate) async fn game_log_fragment(
    Path(ap_slot): Path<i64>,
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    Query(q): Query<LogPageQuery>,
) -> Html<String> {
    let season = ctx.id;
    let Some(name) = ctx.rt.with_board(|b| {
        b.slots
            .get(&ap_slot)
            .map(|s| s.name.clone())
            .unwrap_or_default()
    }) else {
        return Html(String::new());
    };
    if !ctx.state().is_live() || name.is_empty() {
        return Html(String::new());
    }
    let term = q.q.as_deref().unwrap_or("").trim();
    let rpp = q
        .rpp
        .unwrap_or(render::GAME_LOG_RPP)
        .clamp(render::GAME_LOG_RPP_MIN, render::GAME_LOG_RPP_MAX);
    let total = st
        .db
        .activity_count_for_participant(season, &name, term)
        .await;
    let pages = render::game_log_pages(total, rpp);
    let page = q.page.unwrap_or(1).clamp(1, pages);
    let offset = (page - 1) * rpp;
    let rows = st
        .db
        .activity_for_participant(season, &name, term, offset, rpp)
        .await;
    let qs = if term.is_empty() {
        String::new()
    } else {
        format!("q={}", pct(term))
    };
    Html(render::game_log_feed(ap_slot, &rows, page, pages, rpp, &qs))
}

/// The "Hints in this world" rows for one AP slot, as an HTML fragment — used by
/// the game page to live-refresh the list after a hint (no full reload).
pub(crate) async fn game_hints(
    Path(ap_slot): Path<i64>,
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
) -> Html<String> {
    let my_id = me.id();
    let (_me, is_admin) = ctx.ident(&me);
    let season = ctx.id;
    let Some(snap) = ctx.rt.with_board(|b| b.snapshot()) else {
        return Html(String::new());
    };
    let prios = live_priorities(&st, &snap, season).await;
    let can_prioritize = is_admin || st.db.claims_ap_slot(season, my_id, ap_slot).await;
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

/// The hint-search suggestion list for one slot, as JSON — the game page polls
/// this after a hint so a name whose last copy was just hinted drops out.
pub(crate) async fn game_items(
    Path(ap_slot): Path<i64>,
    ctx: scope::SeasonCtx,
) -> Json<Vec<String>> {
    let items = ctx
        .rt
        .with_board(|b| {
            let game = b
                .slots
                .get(&ap_slot)
                .map(|s| s.game.clone())
                .unwrap_or_default();
            b.hint_suggestions(ap_slot, &game)
        })
        .unwrap_or_default();
    Json(items)
}

#[derive(Deserialize)]
pub(crate) struct FinishForm {
    pub(crate) back: Option<String>,
}

/// Admin-only: mark a slot "finished" by registering its goal (collects/releases
/// its items per the server modes), as its game client would have. Separate from
/// a manual `/release` in the console.
pub(crate) async fn game_finish(
    Path(ap_slot): Path<i64>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<FinishForm>,
) -> Response {
    let (_me, is_admin) = ctx.ident(&me);
    let back = ctx.back(f.back.as_deref(), &format!("/game/{ap_slot}"));
    if !is_admin {
        return render::notice("Admins only.", false, &back).into_response();
    }
    let slot = ctx
        .rt
        .with_board(|b| {
            b.slots
                .get(&ap_slot)
                .map(|s| (s.name.clone(), s.game.clone()))
        })
        .flatten();
    let Some((name, game)) = slot.filter(|(n, _)| !n.is_empty()) else {
        return render::notice("That slot isn't live right now.", false, &back).into_response();
    };
    let Some(cfg) = ctx.rt.ap_cfg() else {
        return render::notice("AP connection not configured.", false, &back).into_response();
    };
    match ap::mark_goal(&cfg.url, &cfg.password, &name, &game).await {
        Ok(()) => Redirect::to(&back).into_response(),
        Err(e) => {
            render::notice(&format!("Mark-finished failed: {e}"), false, &back).into_response()
        }
    }
}

/// A claimant (or admin) downloading their own slot's generated file from the
/// game page. Gated to someone who claims THIS `ap_slot` (or admin), and the
/// requested file must actually belong to this slot (its `P<n>` == `ap_slot`),
/// so a claimant can't pull another game's file.
pub(crate) async fn game_output(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Path(ap_slot): Path<i64>,
    Query(q): Query<OutputQuery>,
) -> Response {
    let my_id = me.id();
    let (_me, is_admin) = ctx.ident(&me);
    let season = ctx.id;
    let slots = st.db.slots(season).await.unwrap_or_default();
    let i_claim = slots
        .iter()
        .any(|r| r.ap_slot == ap_slot && r.claimed_by_id == Some(my_id));
    let back = format!("/game/{ap_slot}");
    if !(i_claim || is_admin) {
        return render::notice("Claim this game to download its files.", false, &back)
            .into_response();
    }
    if generate::member_player(&q.file) != Some(ap_slot) {
        return render::notice("That file isn't for this game.", false, &back).into_response();
    }
    let member = q.file.clone();
    let id = ctx.id;
    let bytes = tokio::task::spawn_blocking(move || generate::read_output(id, &member))
        .await
        .ok()
        .flatten();
    match bytes {
        Some(data) => download_response(&q.file, data),
        None => {
            render::notice("No such output file in the current seed.", false, &back).into_response()
        }
    }
}
