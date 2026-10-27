//! The public pages: home, health, favicon, `/api/me`, the YAML checker, season summaries.

use super::*;

pub(crate) async fn health() -> &'static str {
    "ok"
}

pub(crate) async fn favicon() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "image/svg+xml")],
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

pub(crate) async fn landing(State(st): State<AppState>) -> Html<String> {
    // The roster is rendered live so it never needs hand-editing between
    // seasons: AP slot names from the default season's live board, the people
    // (incl. co-op) from the DB. Before its seed exists, the section shows the
    // season's sign-up queue, filling in live as people register.
    let (label, roster) = match st.db.default_season().await {
        Some(season) if season.state() != db::State::Archived => {
            let slots = st.db.slots(season.id).await.unwrap_or_default();
            if slots.is_empty() {
                let subs = st.db.submissions(season.id).await.unwrap_or_default();
                (
                    format!("{} — who's signed up", season.name),
                    render::landing_signups(&subs),
                )
            } else {
                let ap_names: std::collections::HashMap<i64, String> = st
                    .seasons
                    .get(season.id)
                    .with_board(|b| b.slots.iter().map(|(k, s)| (*k, s.name.clone())).collect())
                    .unwrap_or_default();
                (
                    format!("{} — who's playing what", season.name),
                    render::landing_roster(&slots, &ap_names),
                )
            }
        }
        _ => ("No season under way".to_string(), String::new()),
    };
    let past: Vec<(i64, String, i64)> = st
        .db
        .seasons()
        .await
        .into_iter()
        .filter(|s| s.state() == db::State::Archived)
        .map(|s| (s.id, s.name, s.archived_at.unwrap_or(s.created_at)))
        .collect();
    let past_html = render::past_seasons_section(&past);
    // Everything operator-specific comes from the instance settings; the AP
    // version from the install Bifrost actually runs.
    let inst = st.db.instance().await;
    let live: Vec<(String, Option<String>)> = st
        .db
        .seasons()
        .await
        .into_iter()
        .filter(|s| s.state() == db::State::Live)
        .map(|s| {
            let addr = st.seasons.connect_address(&s, &inst.domain);
            (s.name, addr)
        })
        .collect();
    Html(
        LANDING
            .replace("<!--SEASONLABEL-->", &render::escape(&label))
            .replace("<!--ROSTER-->", &roster)
            .replace("<!--PASTSEASONS-->", &past_html)
            .replace(
                "<!--CONNECT-->",
                &render::landing_connect(&live, &inst.password_note),
            )
            .replace("<!--HOMEPILL-->", &render::home_pill(&inst.home_url))
            .replace("<!--TAG-->", &render::landing_tag(&inst.domain))
            .replace(
                "<!--APVERSION-->",
                &render::escape(&format!("Archipelago {}", apinstall::active().upstream())),
            )
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
