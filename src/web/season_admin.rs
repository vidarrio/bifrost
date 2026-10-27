//! The season console: lifecycle, players, worlds, moderators, connection, archive.

use super::*;

/// Gather everything the admin page shows and render it.
pub(crate) async fn render_admin(
    ctx: &scope::SeasonCtx,
    st: &AppState,
    me: &auth::Me,
) -> Html<String> {
    let (connected, seed, acdata) = ctx
        .rt
        .with_board(|b| {
            (
                b.connected,
                b.seed.clone(),
                b.autocomplete_data().to_string(),
            )
        })
        .unwrap_or((false, None, "{}".into()));
    let signups_open = ctx.state().accepts_signups();
    let minesweeper_on = ctx.row.minesweeper_enabled;
    let mc_on = ctx.row.mc_enabled;
    let connect_pw = if config::get().ap_managed {
        st.db.ap_passwords(ctx.id).await.0
    } else {
        ctx.rt.ap_cfg().map(|c| c.password).unwrap_or_default()
    };
    let (running, gen_msg) = ctx
        .rt
        .with_genstate(|g| (g.0, g.1.clone()))
        .unwrap_or((false, "unknown".into()));
    let subs = st.db.submissions(ctx.id).await.unwrap_or_default();
    let apworlds = generate::list_apworlds(ctx.id).await;
    // Per-player files the generator produced, with each slot's game resolved
    // from the Bifrost slots for the current seed (filename carries the slot
    // name; db carries name→game).
    let id = ctx.id;
    let outputs = tokio::task::spawn_blocking(move || generate::list_outputs(id))
        .await
        .unwrap_or_default();
    let out_slots = st.db.slots(ctx.id).await.unwrap_or_default();
    let archived = (ctx.state() == db::State::Archived).then(|| ctx.row.name.clone());
    let owner = match ctx.row.created_by {
        Some(id) => st.db.username(id).await,
        None => None,
    };
    let mods = render::SeasonMods {
        owner,
        co_mods: st.db.co_mods(ctx.id).await,
    };
    render::admin_page(
        &ctx.row,
        &mods,
        connected,
        seed.as_deref(),
        signups_open,
        minesweeper_on,
        mc_on,
        &connect_pw,
        running,
        &gen_msg,
        &subs,
        &apworlds,
        &st.db.world_requests(ctx.id).await,
        me.can(auth::Action::ManageInstance),
        &acdata,
        archived.as_deref(),
        &outputs,
        &out_slots,
        st.seasons
            .connect_address(&ctx.row, &st.db.instance().await.domain)
            .as_deref(),
        config::get().ap_managed,
    )
}

pub(crate) async fn admin_view(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
) -> Response {
    let (_me, is_admin) = ctx.ident(&me);
    if !is_admin {
        return render::notice("Admins only.", false, "board").into_response();
    }
    render_admin(&ctx, &st, &me).await.into_response()
}

#[derive(Deserialize)]
pub(crate) struct SignupsForm {
    pub(crate) open: String,
}

/// Open (draft → signup) or close (signup → draft) the season's signups.
pub(crate) async fn admin_signups(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<SignupsForm>,
) -> Response {
    let (_me, is_admin) = ctx.ident(&me);
    if is_admin {
        let to = if f.open == "1" {
            db::State::Signup
        } else {
            db::State::Draft
        };
        if ctx.state() != to
            && let Err(e) = st.db.transition(ctx.id, to, auth::now()).await
        {
            return render::notice(&e.to_string(), false, "admin").into_response();
        }
    }
    Redirect::to(&ctx.url("/admin")).into_response()
}

pub(crate) async fn admin_generate(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
) -> Redirect {
    let (_me, is_admin) = ctx.ident(&me);
    if is_admin {
        let start = ctx.rt.with_genstate(|g| {
            if g.0 {
                return false;
            }
            g.0 = true;
            g.1 = "Starting…".into();
            true
        }) == Some(true);
        if start {
            let (db, rts, id) = (st.db.clone(), st.seasons.clone(), ctx.id);
            tokio::spawn(async move { generate::run(db, rts, id).await });
        }
    }
    Redirect::to(&ctx.url("/admin"))
}

pub(crate) async fn admin_regenerate(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
) -> Redirect {
    let (_me, is_admin) = ctx.ident(&me);
    if is_admin {
        // A re-roll replaces a live season's seed.
        if !ctx.state().is_live() {
            ctx.rt.with_genstate(|g| {
                if !g.0 {
                    g.1 = "Regenerate re-rolls a live season — generate it first.".into();
                }
            });
            return Redirect::to(&ctx.url("/admin"));
        }
        let start = ctx.rt.with_genstate(|g| {
            if g.0 {
                return false;
            }
            g.0 = true;
            g.1 = "Starting…".into();
            true
        }) == Some(true);
        if start {
            let (db, rts, id) = (st.db.clone(), st.seasons.clone(), ctx.id);
            tokio::spawn(async move { generate::regenerate(db, rts, id).await });
        }
    }
    Redirect::to(&ctx.url("/admin"))
}

/// Delete the season's signup queue: its `submission` rows and their files.
pub(crate) async fn clear_signups(st: &AppState, season: i64) {
    if let Ok(files) = st.db.clear_submissions(season).await {
        for f in files {
            let _ = tokio::fs::remove_file(&f).await;
        }
    }
}

/// Delete the current season's players: every Bifrost slot (claims cascade) and
/// the staged Players/ YAMLs.
pub(crate) async fn clear_current_players(st: &AppState, season: i64) {
    let _ = st.db.clear_all_slots(season).await;
    let _ = generate::clear_player_yamls(season).await;
}

#[derive(Deserialize)]
pub(crate) struct ClearForm {
    pub(crate) what: String,
}

pub(crate) async fn admin_clear(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<ClearForm>,
) -> Response {
    let (_me, is_admin) = ctx.ident(&me);
    if !is_admin {
        return render::notice("Admins only.", false, "board").into_response();
    }
    let id = ctx.id;
    match f.what.as_str() {
        "apworlds" => {
            let _ = generate::clear_apworlds(id).await;
            worlds::clear_pending(&st.db, id).await;
        }
        "signups" => clear_signups(&st, id).await,
        "players" => clear_current_players(&st, id).await,
        "save" => generate::clear_save(id).await,
        "world" => generate::clear_world(id).await,
        "reset" => {
            let _ = generate::clear_apworlds(id).await;
            worlds::clear_pending(&st.db, id).await;
            clear_signups(&st, id).await;
            clear_current_players(&st, id).await;
            generate::clear_save(id).await;
            generate::clear_world(id).await;
            if ctx.state() == db::State::Signup {
                let _ = st.db.transition(id, db::State::Draft, auth::now()).await;
            }
        }
        _ => {}
    }
    // Clearing players/claims empties the season's whitelist — refresh the export
    // so the host syncer drops the old names.
    if matches!(f.what.as_str(), "players" | "reset") {
        export_mc_whitelist(&st).await;
    }
    Redirect::to(&ctx.url("/admin")).into_response()
}

#[derive(Deserialize)]
pub(crate) struct ArchiveForm {
    pub(crate) name: Option<String>,
}

/// Archive a live season (live → archived): freeze its roster, per-game
/// standings (goaled + checks) and minesweeper leaderboards for the Past
/// Seasons page, write the raw replayable bundle, and stop its workers.
pub(crate) async fn admin_archive_season(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<ArchiveForm>,
) -> Response {
    let (_me, is_admin) = ctx.ident(&me);
    if !is_admin {
        return render::notice("Admins only.", false, "admin").into_response();
    }
    // Snapshot the live board under one lock: seed, per-slot figures, and the
    // AP slot -> primary player name map (to put the lead seat first).
    let Some((snap, ap_names, board_seed)) = ctx.rt.with_board(|b| {
        let names: HashMap<i64, String> =
            b.slots.iter().map(|(k, s)| (*k, s.name.clone())).collect();
        (b.snapshot(), names, b.seed.clone())
    }) else {
        return render::notice("Board is busy — try again.", false, "admin").into_response();
    };
    if !ctx.state().is_live() {
        return render::notice("Only a live season can be archived.", false, "admin")
            .into_response();
    }
    let seed = ctx.row.seed.clone().unwrap_or_default();
    // The standings come from the live board: refuse until it has seen this
    // season's room (just after a restart, or while the server is down or
    // loading, every game would be frozen at 0 checks — and archiving is final).
    if seed.is_empty() || board_seed.as_deref() != Some(seed.as_str()) || ap_names.is_empty() {
        return render::notice(
            "The live board isn't connected to this season's game yet — try again once it shows the game's progress.",
            false,
            "admin",
        )
        .into_response();
    }
    let slots = st.db.slots(ctx.id).await.unwrap_or_default();
    if slots.is_empty() {
        return render::notice(
            "No slots to archive for the current season.",
            false,
            "admin",
        )
        .into_response();
    }

    // Group the season's Bifrost slots by AP slot (one game each; co-op slots
    // share an ap_slot), gathering the seat names and final standing.
    let mut games: Vec<season::SeasonGame> = Vec::new();
    let mut order: Vec<i64> = Vec::new();
    let mut by_slot: HashMap<i64, (String, Vec<String>)> = HashMap::new();
    for s in &slots {
        let e = by_slot.entry(s.ap_slot).or_insert_with(|| {
            order.push(s.ap_slot);
            (s.game.clone(), Vec::new())
        });
        e.1.push(s.name.clone());
    }
    let mut player_total = 0usize;
    for ap_slot in &order {
        let (game, mut names) = by_slot.remove(ap_slot).unwrap();
        names.sort_by_key(|n| n.to_ascii_lowercase());
        // Primary (the AP slot's own player) first.
        if let Some(lead) = ap_names.get(ap_slot) {
            if let Some(pos) = names.iter().position(|n| n.eq_ignore_ascii_case(lead)) {
                let p = names.remove(pos);
                names.insert(0, p);
            }
        }
        player_total += names.len();
        let live = snap.ap.get(ap_slot);
        let checks = live.map(|a| a.sent).unwrap_or(0);
        let total = live.map(|a| a.world_size).unwrap_or(0);
        let pct = if total > 0 {
            ((checks * 100) / total as u64) as u32
        } else {
            0
        };
        // Same three-way status the live board shows (goaled / released / —),
        // so a dropped-out player reads "released" on the archive too.
        let (status, label) = live.map(render::card_status).unwrap_or(("idle", "—"));
        games.push(season::SeasonGame {
            game,
            players: names,
            status: status.to_string(),
            label: label.to_string(),
            checks,
            total,
            pct,
        });
    }
    games.sort_by(|a, b| {
        a.game
            .to_ascii_lowercase()
            .cmp(&b.game.to_ascii_lowercase())
    });

    // Freeze this season's minesweeper leaderboards (same shape as the live
    // seasonal board: top 10, rate needs ≥5 games).
    let mut minesweeper = Vec::new();
    for (kind, label) in [("standard", "Normal"), ("expert", "Expert")] {
        let time = st.db.mine_leaders_time(Some(ctx.id), kind, 10).await;
        let rate = st.db.mine_leaders_rate(Some(ctx.id), kind, 5, 10).await;
        minesweeper.push(season::MineBoard {
            kind: kind.to_string(),
            label: label.to_string(),
            time,
            rate,
        });
    }

    let name = f
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| ctx.row.name.clone());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let summary = season::SeasonSummary {
        name: name.clone(),
        seed: seed.clone(),
        finalized_at: now,
        player_count: player_total,
        games,
        minesweeper,
    };
    let json = serde_json::to_string(&summary).unwrap_or_default();
    if name != ctx.row.name {
        let _ = st.db.rename_season(ctx.id, &name).await;
    }
    if let Err(e) = st.db.set_season_summary(ctx.id, &json).await {
        return render::notice(&format!("Archive failed: {e}"), false, "admin").into_response();
    }

    // Also produce the raw replayable bundle (multidata, save, YAMLs, apworlds,
    // journal, roster) — Bifrost owns the dump; the host script only imports it
    // to Nextcloud. Best-effort: the displayable archive already succeeded.
    let season_txt = {
        let mut t = format!(
            "name: {}\nseed: {}\narchived_utc: {}\n\nroster (game — status — players):\n",
            summary.name,
            summary.seed,
            fmt_utc(now),
        );
        for g in &summary.games {
            t.push_str(&format!(
                "  {} — {} — {}\n",
                g.game,
                g.label,
                g.players.join(" & ")
            ));
        }
        t
    };
    let manifest = archive::Manifest::new(&ctx.row, &summary.name, &seed, now);
    match archive::bundle_season(ctx.id, &seed, &season_txt, &manifest, &summary).await {
        Ok(path) => tracing::info!("season bundle written: {path}"),
        Err(e) => tracing::warn!("season bundle failed (summary saved): {e}"),
    }
    if let Err(e) = st.db.transition(ctx.id, db::State::Archived, now).await {
        return render::notice(
            &format!("Summary saved, but archiving failed: {e}"),
            false,
            "admin",
        )
        .into_response();
    }
    st.seasons.stop(ctx.id).await;
    apinstall::pin(ctx.id, None);
    export_mc_whitelist(&st).await;

    Redirect::to(&ctx.url("/season")).into_response()
}

#[derive(Deserialize)]
pub(crate) struct ModAddForm {
    pub(crate) username: String,
}

/// Add a co-moderator (an account with the moderator role or higher).
pub(crate) async fn mods_add(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<ModAddForm>,
) -> Response {
    if !ctx.can_manage(&me) {
        return render::notice("Only this season's moderators can do that.", false, "admin")
            .into_response();
    }
    let target = st.db.account_by_name(f.username.trim()).await;
    match target {
        Some(a) if auth::Role::parse(&a.role) >= auth::Role::Moderator && !a.disabled => {
            let _ = st.db.add_co_mod(ctx.id, a.id).await;
            Redirect::to(&ctx.url("/admin")).into_response()
        }
        Some(_) => render::notice(
            "That account isn't a moderator — a sysadmin can change its role under users & links.",
            false,
            "admin",
        )
        .into_response(),
        None => render::notice("No such account.", false, "admin").into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct ModRemoveForm {
    pub(crate) account_id: i64,
}

pub(crate) async fn mods_remove(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<ModRemoveForm>,
) -> Response {
    if !ctx.can_manage(&me) {
        return render::notice("Only this season's moderators can do that.", false, "admin")
            .into_response();
    }
    let _ = st.db.remove_co_mod(ctx.id, f.account_id).await;
    Redirect::to(&ctx.url("/admin")).into_response()
}

pub(crate) async fn admin_delete_player(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<DelForm>,
) -> Redirect {
    let my_id = me.id();
    let (_me, is_admin) = ctx.ident(&me);
    if is_admin
        && let Ok(Some(yaml_file)) = st.db.delete_submission(ctx.id, f.id, my_id, true).await
    {
        let _ = tokio::fs::remove_file(&yaml_file).await;
    }
    Redirect::to(&ctx.url("/admin"))
}

#[derive(Deserialize)]
pub(crate) struct ApworldForm {
    pub(crate) name: String,
}

pub(crate) async fn admin_delete_apworld(
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<ApworldForm>,
) -> Redirect {
    let (_me, is_admin) = ctx.ident(&me);
    if is_admin {
        let _ = generate::delete_apworld(ctx.id, &f.name).await;
    }
    Redirect::to(&ctx.url("/admin"))
}

/// Approve a pending apworld upload (season moderator or sysadmin).
pub(crate) async fn admin_world_approve(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<ApworldForm>,
) -> Response {
    let (who, is_admin) = ctx.ident(&me);
    if !is_admin {
        return render::notice("Admins only.", false, "board").into_response();
    }
    match worlds::approve(&st.db, ctx.id, &f.name, &who).await {
        Ok(_) => Redirect::to(&ctx.url("/admin")).into_response(),
        Err(e) => render::notice(&e, false, "admin").into_response(),
    }
}

pub(crate) async fn admin_world_reject(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<ApworldForm>,
) -> Response {
    let (who, is_admin) = ctx.ident(&me);
    if !is_admin {
        return render::notice("Admins only.", false, "board").into_response();
    }
    match worlds::reject(&st.db, ctx.id, &f.name, &who).await {
        Ok(()) => Redirect::to(&ctx.url("/admin")).into_response(),
        Err(e) => render::notice(&e, false, "admin").into_response(),
    }
}

/// Add a world installed in this season to the instance catalog (sysadmin).
pub(crate) async fn admin_world_to_catalog(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<ApworldForm>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return render::notice("Sysadmins only.", false, "admin").into_response();
    }
    let (who, _) = ctx.ident(&me);
    match worlds::catalog_add_installed(&st.db, ctx.id, &f.name, &who).await {
        Ok(_) => Redirect::to(&ctx.url("/admin")).into_response(),
        Err(e) => render::notice(&e, false, "admin").into_response(),
    }
}

// ---- the apworld catalog (instance, sysadmin) ----

#[derive(Deserialize)]
pub(crate) struct ConsoleForm {
    pub(crate) cmd: String,
}

/// Run a server command as admin and return its output as JSON, so the admin
/// page can append it to a live log without a page reload.
pub(crate) async fn admin_console(
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<ConsoleForm>,
) -> Json<serde_json::Value> {
    let (_me, is_admin) = ctx.ident(&me);
    if !is_admin {
        return Json(serde_json::json!({"ok": false, "output": "Admins only."}));
    }
    let cmd = f.cmd.trim().to_string();
    // A plain `/send <player> <item>` grants a duplicate and leaves the item's
    // location (and any hint) untouched. If that item has an active hint, deliver
    // it from its real location instead (`/send_location`), which also marks the
    // hint found — matching the expectation that sending an item resolves it.
    let effective = ctx
        .rt
        .with_board(|b| {
            let rest = cmd.strip_prefix("/send ")?.trim();
            let (player, item) = rest.split_once(char::is_whitespace)?;
            let (finder, loc) = b.send_to_location(player.trim(), item.trim())?;
            Some(format!("/send_location {finder} {loc}"))
        })
        .flatten()
        .unwrap_or_else(|| cmd.clone());
    let output = match ctx.rt.ap_cfg().as_ref() {
        Some(cfg) => {
            match ap::send_admin(
                &cfg.url,
                &cfg.slot,
                &cfg.password,
                &cfg.admin_password,
                &effective,
            )
            .await
            {
                Ok(lists) => {
                    let text = ctx
                        .rt
                        .with_board(|b| {
                            lists
                                .iter()
                                .map(|p| ap::resolve_parts(b, p))
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default();
                    if text.trim().is_empty() {
                        // Several server commands (/hint, /send, /option, /collect,
                        // /release …) succeed silently — they act on the room and
                        // broadcast to players but return no console text. An empty
                        // reply here is normal, not a failure.
                        "(command accepted — no console output; some commands like /hint and /send are silent on success and just broadcast to the room)".to_string()
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

#[derive(Deserialize)]
pub(crate) struct ConnPwForm {
    pub(crate) password: String,
}

pub(crate) async fn admin_connect_password(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<ConnPwForm>,
) -> Response {
    let managed = config::get().ap_managed;
    // Managed: the password is the season's own; external: it's the shared
    // server's, an instance setting.
    let allowed = if managed {
        ctx.can_manage(&me)
    } else {
        me.can(auth::Action::ManageInstance)
    };
    if !allowed {
        return render::notice("Admins only.", false, "board").into_response();
    }
    let pw = f.password.trim().to_string();
    if !(3..=64).contains(&pw.len())
        || !pw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return render::notice(
            "Password must be 3–64 characters: letters, numbers, - or _ only.",
            false,
            "admin",
        )
        .into_response();
    }
    if managed {
        // Stored with the season (used whenever its server starts) and applied
        // to the running server's console right away — no restart.
        if let Err(e) = st.db.set_ap_password(ctx.id, &pw).await {
            return render::notice(&format!("Couldn't save it: {e}"), false, "admin")
                .into_response();
        }
        st.seasons.set_connect_password(ctx.id, &pw);
        let cmd = format!("/option password {pw}");
        let applied = st.seasons.console(ctx.id, &cmd).await;
        let msg = if applied {
            "Connect password updated on the running server."
        } else {
            "Connect password saved — it applies when this season's server starts."
        };
        return render::notice(msg, true, "admin").into_response();
    }
    let Some(cfg) = ctx.rt.ap_cfg() else {
        return render::notice("AP connection isn't configured.", false, "admin").into_response();
    };
    // Is a season actually live right now (tracker connected to a running
    // server)? Between seasons there's nothing to set the password ON — the
    // value just needs persisting so the next generated/loaded season uses it.
    let live = ctx
        .rt
        .with_board(|b| b.snapshot().connected)
        .unwrap_or(false);
    // Best-effort set it on the running server first (bifrost's env is still the
    // old one, so this connection still authenticates). If a season IS live and
    // this fails, bail WITHOUT persisting — otherwise bifrost would restart with
    // the new password in its env and be unable to reconnect to the still-running
    // old-password server. If no season is live, a failure here is expected
    // (nothing to connect to), so fall through and persist for the next season.
    if let Err(e) = ap::send_admin(
        &cfg.url,
        &cfg.slot,
        &cfg.password,
        &cfg.admin_password,
        &format!("/option password {pw}"),
    )
    .await
    {
        if live {
            return render::notice(
                &format!("Couldn't set it on the running server: {e}"),
                false,
                "admin",
            )
            .into_response();
        }
    }
    // Persist after a short delay so this response reaches the browser before the
    // root helper restarts bifrost (writing the request file triggers it: a
    // host-side path unit persists it to host.yaml + the service env).
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let line = format!("{pw}\n");
        let _ = tokio::fs::write(config::get().connpw_request_file(), line).await;
    });
    let msg = if live {
        "Connect password updated. Bifrost is restarting to apply it everywhere — give it a few seconds, then reload."
    } else {
        "No season is live, so the connect password was saved to the server config — it'll apply to the next season you generate. Bifrost is restarting; give it a few seconds, then reload."
    };
    render::notice(msg, true, "admin").into_response()
}

pub(crate) async fn admin_minesweeper(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<SignupsForm>,
) -> Redirect {
    let (_me, is_admin) = ctx.ident(&me);
    if is_admin {
        let _ = st.db.set_minesweeper_enabled(ctx.id, f.open == "1").await;
    }
    Redirect::to(&ctx.url("/admin"))
}

#[derive(Deserialize)]
pub(crate) struct OutputQuery {
    pub(crate) file: String,
}

/// Stream one generated per-player file (or the spoiler) out of the current
/// seed zip as a download. Admin-only (also behind the admin-gated Traefik
/// router); `generate::read_output` validates the member is really in the zip.
pub(crate) async fn admin_output(
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Query(q): Query<OutputQuery>,
) -> Response {
    let (_me, is_admin) = ctx.ident(&me);
    if !is_admin {
        return render::notice("Admins only.", false, "board").into_response();
    }
    let member = q.file.clone();
    let id = ctx.id;
    let bytes = tokio::task::spawn_blocking(move || generate::read_output(id, &member))
        .await
        .ok()
        .flatten();
    let Some(data) = bytes else {
        return render::notice("No such output file in the current seed.", false, "admin")
            .into_response();
    };
    download_response(&q.file, data)
}
