//! Minesweeper: boards, moves, stats and leaderboards.

use super::*;

pub(crate) async fn minesweeper_page(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Query(q): Query<KindQuery>,
) -> Html<String> {
    let my_id = me.id();
    let (_me, _is_admin) = ctx.ident(&me);
    let free = q.free;
    // Freeplay (minigame section) is always available and ignores the admin
    // toggle, which only gates the seasonal bonus-hint board.
    let enabled = free || ctx.row.minesweeper_enabled;
    let season = ctx.id;
    // Freeplay: no bonus hints; stats are all-time. Seasonal: credits + seasonal.
    let credits = if free {
        0
    } else {
        st.db.credits(season, my_id).await
    };
    let stat_scope: Option<i64> = if free { None } else { Some(season) };
    let standard = st.db.mine_stats(stat_scope, my_id, "standard").await;
    let expert = st.db.mine_stats(stat_scope, my_id, "expert").await;
    let back = ctx.back(q.back.as_deref(), "/board");
    render::minesweeper_page(enabled, free, credits, &standard, &expert, &back)
}

pub(crate) fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[derive(Deserialize)]
pub(crate) struct KindQuery {
    #[serde(default)]
    pub(crate) kind: String,
    /// Freeplay (minigame section) vs seasonal (game board).
    #[serde(default)]
    pub(crate) free: bool,
    /// Where the back link returns to (the game page minesweeper was opened
    /// from); only read by `minesweeper_page`.
    #[serde(default)]
    pub(crate) back: Option<String>,
}

/// Map key for a player's in-progress game: separate per difficulty AND per mode
/// (freeplay vs seasonal), so the two can be in progress at once without
/// clobbering each other.
pub(crate) fn mine_key(account: i64, free: bool, season: i64, kind: mine::Kind) -> (i64, String) {
    // A seasonal game belongs to its season: finishing it under another
    // season's URL mustn't credit that one.
    let mode = if free {
        "free:".to_string()
    } else {
        format!("s{season}:")
    };
    (account, format!("{mode}{}", kind.id()))
}

/// Bonus hints a player can earn per season in any 24 hours.
pub(crate) const DAILY_BONUS_HINTS: i64 = 5;

#[derive(Deserialize)]
pub(crate) struct NewForm {
    #[serde(default)]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) free: bool,
}

pub(crate) async fn mine_new(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<NewForm>,
) -> Json<serde_json::Value> {
    let my_id = me.id();
    let (_me, _) = ctx.ident(&me);
    // Freeplay (the standalone minigame) is always available; the admin toggle
    // only gates the seasonal, bonus-hint-earning board.
    if !f.free && !ctx.row.minesweeper_enabled {
        return Json(serde_json::json!({"ok": false, "error": "Minesweeper is disabled."}));
    }
    let kind = mine::Kind::parse(&f.kind);
    let game = mine::Game::new(kind);
    let (w, h, mines) = (game.w, game.h, game.mines_n);
    st.with_mine_games(|games| {
        let now = now_nanos();
        // Drop stale games (>12h old) so the map stays small, then replace this
        // player's game of this difficulty+mode with the fresh one.
        games.retain(|_, g| now.saturating_sub(g.created) < 12 * 3600 * 1_000_000_000);
        games.insert(mine_key(my_id, f.free, ctx.id, kind), game);
    });
    Json(serde_json::json!({"ok": true, "w": w, "h": h, "mines": mines, "kind": kind.id()}))
}

/// The player's in-progress game for a difficulty (if any), so the board + timer
/// survive closing/reopening the tab and switching difficulty tabs. Loading it
/// pauses the clock (`repose`); it resumes on the next reveal.
pub(crate) async fn mine_state(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Query(q): Query<KindQuery>,
) -> Json<serde_json::Value> {
    let my_id = me.id();
    let (_me, _) = me.ident();
    let kind = mine::Kind::parse(&q.kind);
    st.with_mine_games(
        |games| match games.get_mut(&mine_key(my_id, q.free, ctx.id, kind)) {
            Some(g) if !g.over => {
                g.repose();
                let cells: Vec<_> = g
                    .revealed_cells()
                    .iter()
                    .map(|(r, c, n)| serde_json::json!({"r": r, "c": c, "n": n}))
                    .collect();
                Json(serde_json::json!({
                    "active": true, "w": g.w, "h": g.h, "mines": g.mines_n,
                    "cells": cells, "elapsed_ms": g.active_ms(), "running": g.running
                }))
            }
            _ => Json(serde_json::json!({"active": false})),
        },
    )
}

/// Minimum games to qualify for the completion-rate leaderboard.
pub(crate) const LEADERBOARD_MIN_GAMES: i64 = 5;
/// Rows shown on each leaderboard.
pub(crate) const LEADERBOARD_TOP: i64 = 10;

/// The season leaderboards for a difficulty (best time + best completion rate),
/// as an HTML fragment the minesweeper page loads when a 🏆 tab is opened.
pub(crate) async fn mine_leaderboard(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    Query(q): Query<KindQuery>,
) -> Html<String> {
    let kind = mine::Kind::parse(&q.kind);
    // Freeplay shows the all-time (permanent) board; the seasonal board scopes to
    // the season.
    let scope: Option<i64> = if q.free { None } else { Some(ctx.id) };
    let time = st
        .db
        .mine_leaders_time(scope, kind.id(), LEADERBOARD_TOP)
        .await;
    let rate = st
        .db
        .mine_leaders_rate(scope, kind.id(), LEADERBOARD_MIN_GAMES, LEADERBOARD_TOP)
        .await;
    let kind_label = if kind == mine::Kind::Expert {
        "Expert"
    } else {
        "Standard"
    };
    let label = if q.free {
        format!("All-time {kind_label}")
    } else {
        kind_label.to_string()
    };
    Html(render::mine_leaderboard_html(
        &label,
        &time,
        &rate,
        LEADERBOARD_MIN_GAMES,
    ))
}

/// Pause a difficulty's timer (called when the tab is hidden/closed or when
/// switching to the other difficulty). Idempotent.
pub(crate) async fn mine_pause(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Query(q): Query<KindQuery>,
) -> Json<serde_json::Value> {
    let my_id = me.id();
    let (_me, _) = me.ident();
    let kind = mine::Kind::parse(&q.kind);
    st.with_mine_games(|games| {
        if let Some(g) = games.get_mut(&mine_key(my_id, q.free, ctx.id, kind)) {
            g.pause();
        }
    });
    Json(serde_json::json!({"ok": true}))
}

#[derive(Deserialize)]
pub(crate) struct RevealForm {
    pub(crate) r: usize,
    pub(crate) c: usize,
    #[serde(default)]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) free: bool,
}

pub(crate) async fn mine_reveal(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<RevealForm>,
) -> Json<serde_json::Value> {
    let my_id = me.id();
    let (_me, _) = ctx.ident(&me);
    let key = mine_key(my_id, f.free, ctx.id, mine::Kind::parse(&f.kind));
    // Reveal under the lock, then release it before any await.
    let outcome = st.with_mine_games(|games| {
        let g = games.get_mut(&key)?;
        let rv = g.reveal(f.r, f.c);
        let (won, over) = (g.won, g.over);
        let (elapsed_ms, running) = (g.active_ms(), g.running);
        let kind = g.kind;
        if over {
            games.remove(&key);
        }
        Some((rv, won, over, elapsed_ms, running, kind))
    });
    let Some((rv, won, over, elapsed_ms, running, kind)) = outcome else {
        return Json(serde_json::json!({"ok": false, "error": "No active game."}));
    };
    let mut credits: Option<i64> = None;
    // On a finished game (win or loss), record the result and return fresh stats.
    // Seasonal games record under the season, count toward the seasonal
    // leaderboard/stats, and a win earns a bonus hint. Freeplay (minigame
    // section) records outside any season, earns no hint, and stays out of the
    // seasonal board — but both feed the all-time (permanent) board, and the
    // stats panel shows the matching scope.
    let stats_html = if over {
        let scope: Option<i64> = if f.free { None } else { Some(ctx.id) };
        st.db
            .record_mine_result(scope, my_id, kind, won, elapsed_ms)
            .await;
        // At most DAILY_BONUS_HINTS a day: every board is solvable without
        // guessing, so a script could otherwise win them without end.
        if won
            && !f.free
            && ctx.row.minesweeper_enabled
            && ctx.state() != db::State::Archived
            && st.db.recent_mine_wins(ctx.id, my_id, 24 * 3600).await <= DAILY_BONUS_HINTS
        {
            credits = st.db.add_credits(ctx.id, my_id, 1).await.ok();
        }
        // Freeplay stats are all-time; seasonal stats scope to the season.
        let stat_scope = scope;
        let standard = st.db.mine_stats(stat_scope, my_id, "standard").await;
        let expert = st.db.mine_stats(stat_scope, my_id, "expert").await;
        Some(render::mine_stats_html(&standard, &expert))
    } else {
        None
    };
    let cells_json = |cells: &[(usize, usize, u8)]| {
        cells
            .iter()
            .map(|(r, c, n)| serde_json::json!({"r": r, "c": c, "n": n}))
            .collect::<Vec<_>>()
    };
    match rv {
        mine::Reveal::Play(cells) => Json(serde_json::json!({
            "ok": true, "state": "play", "cells": cells_json(&cells),
            "elapsed_ms": elapsed_ms, "running": running
        })),
        mine::Reveal::Won(cells) => Json(serde_json::json!({
            "ok": true, "state": "won", "cells": cells_json(&cells),
            "credits": credits, "stats": stats_html, "elapsed_ms": elapsed_ms, "running": running
        })),
        mine::Reveal::Lost(mines) => {
            let ms: Vec<_> = mines
                .iter()
                .map(|(r, c)| serde_json::json!({"r": r, "c": c}))
                .collect();
            Json(serde_json::json!({
                "ok": true, "state": "lost", "mines": ms, "stats": stats_html,
                "elapsed_ms": elapsed_ms, "running": running
            }))
        }
        mine::Reveal::Invalid => Json(serde_json::json!({"ok": false, "error": "Invalid move."})),
    }
}
