//! The live board.

use super::*;

/// Fetch the season's prioritized hints, first dropping the star from any hint
/// whose item has since been found (a found hint is non-prioritizable, so a
/// pre-found one is cleaned up in the DB and excluded from the returned set).
pub(crate) async fn live_priorities(
    st: &AppState,
    snap: &ap::BoardSnapshot,
    season: i64,
) -> std::collections::HashSet<(i64, i64, i64)> {
    let mut prios = st.db.priorities(season).await.unwrap_or_default();
    if prios.is_empty() {
        return prios;
    }
    let stale: Vec<(i64, i64, i64)> = snap
        .ap
        .values()
        .flat_map(|a| a.hint_views.iter())
        .filter(|h| h.found)
        .map(|h| (h.finding, h.location, h.item))
        .filter(|id| prios.contains(id))
        .collect();
    if !stale.is_empty() {
        st.db.unprioritize_many(season, &stale).await;
        for id in &stale {
            prios.remove(id);
        }
    }
    prios
}

pub(crate) async fn board_view(State(st): State<AppState>, ctx: scope::SeasonCtx) -> Response {
    // A finished season's board is its frozen summary.
    if ctx.state() == db::State::Archived {
        return Redirect::to(&ctx.url("/season")).into_response();
    }
    let Some(snap) = ctx.rt.with_board(|b| b.snapshot()) else {
        return Html("board unavailable".to_string()).into_response();
    };
    let season = ctx.id;
    let slots = st.db.slots(season).await.unwrap_or_default();
    let prios = live_priorities(&st, &snap, season).await;
    let addr = mc_addr();
    // Board shows the latest slice of the persistent log (journal-sourced).
    let acts = st.db.activity_latest(season, BOARD_ACTIVITY).await;
    let inlogic = st.db.inlogic_counts(season).await;
    let connect = st
        .seasons
        .connect_address(&ctx.row, &st.db.instance().await.domain);
    render::board_page(
        &snap,
        &slots,
        &prios,
        addr.as_deref(),
        &acts,
        &inlogic,
        connect.as_deref(),
    )
    .into_response()
}

/// The board's live-update payload: the `.boardcols` inner HTML (`body`) + the
/// current connection line (`conn`), as JSON so the poller can update both
/// without a full refresh. Same data as `board_view`; public like `/board`.
pub(crate) async fn board_fragment_view(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
) -> Json<serde_json::Value> {
    let Some(snap) = ctx.rt.with_board(|b| b.snapshot()) else {
        return Json(serde_json::json!({}));
    };
    let season = ctx.id;
    let slots = st.db.slots(season).await.unwrap_or_default();
    let prios = live_priorities(&st, &snap, season).await;
    let addr = mc_addr();
    let acts = st.db.activity_latest(season, BOARD_ACTIVITY).await;
    let inlogic = st.db.inlogic_counts(season).await;
    Json(serde_json::json!({
        "conn": render::conn_line(&snap),
        "progress": render::season_progress(&snap, &slots),
        "body": render::board_fragment(&snap, &slots, &prios, addr.as_deref(), &acts, &inlogic),
    }))
}

/// How many activity rows the board previews; the full log lives at `/log`.
pub(crate) const BOARD_ACTIVITY: i64 = 15;
