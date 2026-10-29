//! Claiming slots, hinting and prioritizing hints.

use super::*;

#[derive(Deserialize)]
pub(crate) struct PrioForm {
    pub(crate) finding: i64,
    pub(crate) location: i64,
    pub(crate) item: i64,
    pub(crate) receiving: i64,
    pub(crate) back: Option<String>,
}

pub(crate) async fn prioritize_post(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<PrioForm>,
) -> Redirect {
    let my_id = me.id();
    let (_me, is_admin) = ctx.ident(&me);
    let season = ctx.id;
    // Already-found hints are non-prioritizable.
    let is_found = ctx.rt.with_board(|b| {
        b.snapshot().ap.values().any(|a| {
            a.hint_views.iter().any(|h| {
                h.found && h.finding == f.finding && h.location == f.location && h.item == f.item
            })
        })
    }) == Some(true);
    // A hint is prioritized by the item's owner: a claimant of the receiving
    // slot (whose items the hint is about), or an admin. The finder's world can
    // be someone else's, so we gate on `receiving`, not `finding`.
    if !is_found && (is_admin || st.db.claims_ap_slot(season, my_id, f.receiving).await) {
        let _ = st
            .db
            .toggle_priority(season, f.finding, f.location, f.item)
            .await;
    }
    Redirect::to(&ctx.back(f.back.as_deref(), "/board"))
}

pub(crate) async fn claim_get(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
) -> Html<String> {
    let (me, is_admin) = ctx.ident(&me);
    let Some(snap) = ctx.rt.with_board(|b| b.snapshot()) else {
        return Html("board unavailable".into());
    };
    let season = ctx.id;
    let slots = st.db.slots(season).await.unwrap_or_default();
    render::claim_page(&snap, &slots, &me, is_admin)
}

#[derive(Deserialize)]
pub(crate) struct ClaimForm {
    pub(crate) slot_id: i64,
    pub(crate) op: String,
    pub(crate) back: Option<String>,
}

pub(crate) async fn claim_post(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<ClaimForm>,
) -> Response {
    let my_id = me.id();
    let (_me, is_admin) = ctx.ident(&me);
    let back = ctx.back(f.back.as_deref(), "/claim");
    let season = ctx.id;
    if ctx.state() == db::State::Archived {
        return render::notice("That season is archived.", false, &back).into_response();
    }
    match f.op.as_str() {
        "claim" => match st.db.claim_one(season, f.slot_id, my_id).await {
            Ok(db::ClaimOutcome::AlreadyHave(name)) => render::notice(
                &format!("You already claim {name} — one slot per player. Unclaim it first if you want to move."),
                false,
                &back,
            )
            .into_response(),
            Ok(db::ClaimOutcome::SlotTaken) => {
                render::notice("That slot is already claimed.", false, &back).into_response()
            }
            _ => {
                // Claimant set changed → refresh the MC whitelist export.
                export_mc_whitelist(&st).await;
                Redirect::to(&back).into_response()
            }
        },
        "unclaim" => {
            let _ = st.db.unclaim(season, f.slot_id, my_id, is_admin).await;
            export_mc_whitelist(&st).await;
            Redirect::to(&back).into_response()
        }
        _ => Redirect::to(&back).into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct HintForm {
    pub(crate) slot_id: i64,
    pub(crate) item: String,
}

/// The server's `!hint` reply is informational text, so infer whether the hint
/// was actually placed (vs. rejected for points / not found) to drive the UI.
pub(crate) fn hint_rejected(text: &str) -> bool {
    let t = text.to_lowercase();
    [
        "not enough",
        "do not have",
        "unable to find",
        "did you mean",
        "afford", // "can't afford" / "cannot afford"
        "sorry",
    ]
    .iter()
    .any(|k| t.contains(k))
}

pub(crate) async fn hint_post(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<HintForm>,
) -> Json<serde_json::Value> {
    let my_id = me.id();
    let (me, _is_admin) = ctx.ident(&me);
    let reject = |msg: &str| Json(serde_json::json!({"ok": false, "message": msg}));
    let detail = st.db.slot_detail(ctx.id, f.slot_id).await.ok().flatten();
    let Some((ap_slot, claimant, is_common)) = detail else {
        return reject("Unknown slot.");
    };
    let season = ctx.id;
    // The common game is shared: any signed-in player who has CLAIMED a game this
    // season may hint, spending the slot's own (communal) AP points — requiring a
    // claim keeps the shared pool to players with skin in the game. Every other
    // slot is claimant-only.
    if is_common {
        if me == "unknown" {
            return reject("Sign in to hint on the common game.");
        }
        if !st.db.has_claimed_slot(season, my_id).await {
            return reject("Claim one of the season's games before hinting on the common game.");
        }
    } else if claimant != Some(my_id) {
        return reject("Only the player who claimed this slot can hint on it.");
    }
    let connect_name = ctx
        .rt
        .with_board(|b| b.slots.get(&ap_slot).map(|s| s.name.clone()))
        .flatten();
    let Some(name) = connect_name else {
        return reject("That slot isn't live right now.");
    };
    let Some(cfg) = ctx.rt.ap_cfg() else {
        return reject("Hinting isn't configured.");
    };
    let creds = st.db.credits(season, my_id).await;
    match ap::send_hint(&cfg.url, &cfg.password, &name, f.item.trim()).await {
        Ok(parts) => {
            let text = ctx
                .rt
                .with_board(|b| ap::resolve_parts(b, &parts))
                .unwrap_or_default();
            if !hint_rejected(&text) {
                // Placed with AP points — report the new balance.
                let points = ap::slot_points(&cfg.url, &cfg.password, &name).await;
                return Json(serde_json::json!({
                    "ok": true, "message": text, "points": points, "credits": creds
                }));
            }
            // Rejected. If it was only a points shortfall and the player holds a
            // bonus-hint credit, place the hint as admin (free) and spend one.
            // Communal slots have no personal credits — they just use their own
            // shared points, so skip the bonus fallback there.
            // The credit is spent first (atomically), so parallel requests
            // can't place several hints for one; it's refunded on failure.
            // One bonus hint per player at a time, so two requests don't pick
            // the same location from the board before either is placed.
            let wants_bonus = !is_common && rejected_for_points(&text) && creds > 0;
            let _one_at_a_time = if wants_bonus {
                Some(ap::keyed_lock(&format!("bonus:{season}:{my_id}")).await)
            } else {
                None
            };
            if wants_bonus && st.db.spend_credit(season, my_id).await {
                // Hint exactly ONE copy — the first unhinted location of this
                // item — by its location id, so the server matches nothing
                // fuzzily and no item group or second copy comes along. An item
                // that can't be pinned down this way isn't hinted for free.
                let refund = || async {
                    let _ = st.db.add_credits(season, my_id, 1).await;
                };
                let admin_cmd = ctx
                    .rt
                    .with_board(|b| {
                        let game = b.slots.get(&ap_slot)?.game.clone();
                        let m = b.meta.as_ref()?;
                        let item_id = m.item_id_by_name(&game, f.item.trim())?;
                        let (finder, loc) = b.first_unhinted_location(ap_slot, item_id)?;
                        Some(format!(
                            "/hint_location {} {loc}",
                            ap::quote_arg(&b.name_of(finder))
                        ))
                    })
                    .flatten();
                let Some(admin_cmd) = admin_cmd else {
                    refund().await;
                    return Json(serde_json::json!({
                        "ok": false,
                        "message": "Out of points, and a bonus hint needs an exact item name from this world that still has an unhinted copy.",
                        "credits": creds
                    }));
                };
                // Success is judged by the server's reply to us alone: a hint
                // that's placed is only broadcast, a refusal answers. Once the
                // command has gone out, a lost connection isn't refunded.
                match ap::send_admin_results(
                    &cfg.url,
                    &cfg.slot,
                    &cfg.password,
                    &cfg.admin_password,
                    &admin_cmd,
                )
                .await
                {
                    Ok(replies) if !replies.is_empty() => {
                        refund().await;
                        return Json(serde_json::json!({
                            "ok": false, "message": replies.join(" "), "credits": creds
                        }));
                    }
                    Ok(_) => {
                        let newc = st.db.credits(season, my_id).await;
                        let points = ap::slot_points(&cfg.url, &cfg.password, &name).await;
                        return Json(serde_json::json!({
                            "ok": true,
                            "message": format!(
                                "Out of points — spent a bonus hint ({newc} left). It's in this world's hints below."
                            ),
                            "points": points,
                            "credits": newc
                        }));
                    }
                    Err(e) => {
                        if !e.sent {
                            refund().await;
                        }
                        let newc = st.db.credits(season, my_id).await;
                        return Json(serde_json::json!({
                            "ok": false,
                            "message": if e.sent {
                                format!("The bonus hint was sent, but the server didn't confirm it ({}); check this world's hints.", e.message)
                            } else {
                                format!("Bonus hint failed: {}", e.message)
                            },
                            "credits": newc
                        }));
                    }
                }
            }
            Json(serde_json::json!({"ok": false, "message": text, "credits": creds}))
        }
        Err(e) => Json(serde_json::json!({
            "ok": false, "message": format!("Hint failed: {e}"), "credits": creds
        })),
    }
}

/// A `!hint` rejection that is specifically a points shortfall (vs. a bad item).
pub(crate) fn rejected_for_points(text: &str) -> bool {
    let t = text.to_lowercase();
    ["not enough", "do not have", "afford"]
        .iter()
        .any(|k| t.contains(k))
}
