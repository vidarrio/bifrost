//! The public pages: home, health, favicon, `/api/me`, the YAML checker, season summaries.

use super::*;

pub(crate) async fn health() -> &'static str {
    "ok"
}

pub(crate) async fn favicon() -> impl IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "image/svg+xml"),
            // Every page header shows it, so let browsers keep it for a day.
            (axum::http::header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        render::FAVICON,
    )
}

/// Identity for the header bar, filled client-side. `/api` is a signed-in
/// tier, so anonymous callers get the middleware's 401 JSON (with `sign_in`).
pub(crate) async fn api_me(State(st): State<AppState>, me: auth::Me) -> Json<serde_json::Value> {
    let (user, _) = me.ident();
    // The header's optional "home" pill (instance settings).
    let home = st.db.instance().await.home_url;
    Json(serde_json::json!({
        "user": user,
        "home": home,
        // Moderators and up: the header's way into season administration.
        "admin": me.can(auth::Action::CreateSeason),
        "sign_in": st.auth.sign_in_url(),
        "sign_out": st.auth.sign_out_url(),
    }))
}

pub(crate) async fn landing(State(st): State<AppState>, me: auth::Me) -> Html<String> {
    // Only what this viewer can see: a private season stays off the page.
    let mut seen = Vec::new();
    for season in st.db.seasons().await {
        let mods = st.db.season_mods(season.id).await;
        if scope::can_view(&st, &season, &mods, &me).await {
            let manage = me.can(auth::Action::ManageSeason { mods: &mods });
            seen.push((season, manage));
        }
    }
    // The signed-in player's own games in live seasons, linked from their cards.
    let mut mine = if me.id() == 0 {
        Vec::new()
    } else {
        st.db.live_games_of(me.id()).await
    };
    let mut cards = Vec::new();
    for (s, manage) in seen
        .iter()
        .filter(|(s, _)| s.state() != db::State::Archived)
    {
        let subs = st.db.submissions(s.id).await.unwrap_or_default();
        let players = subs
            .iter()
            .map(|s| s.account_id)
            .collect::<std::collections::HashSet<_>>()
            .len();
        let href = match s.state() {
            db::State::Live => format!("/s/{}/board", s.id),
            db::State::Draft if *manage => format!("/s/{}/admin", s.id),
            _ => format!("/s/{}/signup", s.id),
        };
        let member = me.id() != 0 && st.db.is_season_member(s.id, me.id()).await;
        let (own, rest): (Vec<_>, Vec<_>) = mine.into_iter().partition(|g| g.0 == s.id);
        mine = rest;
        cards.push(render::SeasonCard {
            id: s.id,
            name: s.name.clone(),
            state: s.state(),
            href,
            games: subs.len(),
            players,
            member,
            mine: own
                .into_iter()
                .map(|(season, game_id, slot, game)| render::MyGame {
                    season,
                    game_id,
                    slot,
                    game,
                })
                .collect(),
        });
    }
    // Live first, then what's coming; the newest first within each.
    let rank = |s: db::State| match s {
        db::State::Live => 0,
        db::State::Generating => 1,
        db::State::Signup => 2,
        _ => 3,
    };
    let created: std::collections::HashMap<Id, i64> =
        seen.iter().map(|(s, _)| (s.id, s.created_at)).collect();
    cards.sort_by_key(|c| (rank(c.state), std::cmp::Reverse(created[&c.id])));
    // A visitor who can still sign in may turn out to be in a private season.
    let sign_in = (me.principal().is_none() && st.auth.mode == auth::AuthMode::Native)
        .then(|| st.auth.sign_in_url());
    let past: Vec<(Id, String, i64)> = seen
        .iter()
        .map(|(s, _)| s)
        .filter(|s| s.state() == db::State::Archived)
        .cloned()
        .map(|s| (s.id, s.name, s.archived_at.unwrap_or(s.created_at)))
        .collect();
    let past_html = render::past_seasons_section(&past);
    // Everything operator-specific comes from the instance settings.
    let inst = st.db.instance().await;
    Html(
        LANDING
            // Constant first, so no data inserted below can be read as it.
            .replace("<!--IDENTITYJS-->", render::IDENTITY_JS)
            .replace("<!--SEASONS-->", &render::landing_seasons(&cards, sign_in))
            .replace("<!--PASTSEASONS-->", &past_html)
            .replace("<!--HOMEPILL-->", &render::home_pill(&inst.home_url))
            .replace("<!--TAG-->", &render::landing_tag(&inst.domain))
            .replace("<!--DISCORDS-->", &render::landing_discords(&inst.discords)),
    )
}

#[derive(Deserialize)]
pub(crate) struct CheckForm {
    pub(crate) yaml: String,
}

pub(crate) async fn check_form(_ctx: Option<scope::SeasonCtx>) -> Html<String> {
    checker::form_page()
}

/// Check a YAML against the season's worlds (core worlds only when there's no
/// season at all).
pub(crate) async fn check_run(
    ctx: Option<scope::SeasonCtx>,
    Form(form): Form<CheckForm>,
) -> Html<String> {
    let dirs = ctx
        .map(|c| c.dirs())
        .unwrap_or_else(|| config::get().core_season());
    match checker::check_yaml(&dirs, &form.yaml).await {
        Ok(res) => checker::result_page(&res),
        Err(e) => checker::error_page(&e),
    }
}

/// Render an archived season's detail page.
/// An archived season's frozen summary + its replayed activity log.
pub(crate) async fn season_view(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    Query(q): Query<LogQuery>,
) -> Response {
    let Some(json) = st.db.season_summary(ctx.id).await else {
        return render::notice("This season hasn't been archived yet.", false, "board")
            .into_response();
    };
    match serde_json::from_str::<season::SeasonSummary>(&json) {
        Ok(summary) => {
            // The season's activity log, replayed with "x ago" frozen to the
            // season-end time rather than now.
            let kinds = selected_kinds(&q);
            let classes = selected_classes(&q);
            let filter = act_filter(&q, &kinds, &classes);
            let total = st.db.activity_count(ctx.id, &filter).await;
            let (rpp, page, pages, offset) = paginate(&q, total);
            let rows = st.db.activity_page(ctx.id, &filter, offset, rpp).await;
            let participants = st.db.activity_participants(ctx.id).await;
            let qs = filter_qs(&q);
            let lf = log_filter(&q, &participants, &qs);
            render::season_page(&summary, &rows, page, pages, rpp, &lf).into_response()
        }
        Err(_) => {
            render::notice("That season's archive is unreadable.", false, "/").into_response()
        }
    }
}
