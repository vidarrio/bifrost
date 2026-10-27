//! The common game's Minecraft server: address, whitelist, client downloads.

use super::*;

/// The player-facing Minecraft common-game server address (host:port), shown on
/// the communal slot's card (`BIFROST_MC_ADDR`); not shown when unset.
pub(crate) fn mc_addr() -> Option<String> {
    config::var("BIFROST_MC_ADDR")
}

/// A valid Minecraft (Java) username: 3–16 of `[A-Za-z0-9_]`.
pub(crate) fn valid_mc_username(s: &str) -> bool {
    (3..=16).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Write the Minecraft whitelist (one JSON array of usernames for the one
/// shared server: every live, MC-enabled season's claimants) to
/// `config::Config::mc_whitelist_file`, atomically (temp + rename). A
/// host-side syncer watches the file and applies it to the Minecraft server,
/// which Bifrost has no reach to. Best-effort: a failure is logged, and the
/// next change (or the host timer) reconciles.
pub(crate) async fn export_mc_whitelist(st: &AppState) {
    let names = st.db.mc_whitelist_all().await;
    let json = serde_json::to_string(&names).unwrap_or_else(|_| "[]".into());
    let file = config::get().mc_whitelist_file();
    let tmp = file.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, &json).and_then(|_| std::fs::rename(&tmp, &file)) {
        tracing::warn!("mc whitelist export failed: {e}");
    }
}

#[derive(Deserialize)]
pub(crate) struct McUsernameForm {
    pub(crate) mc_username: String,
    pub(crate) back: Option<String>,
}

/// A claimant registers/updates the Minecraft username to whitelist on the
/// common-game server. Gated (defence-in-depth on top of the oauth2-proxy group
/// gate) on holding a regular slot this season; the write is applied to the
/// Minecraft server by the host-side syncer that watches the export.
pub(crate) async fn mc_username_post(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<McUsernameForm>,
) -> Response {
    let my_id = me.id();
    let (_me, _is_admin) = ctx.ident(&me);
    let season = ctx.id;
    let back = ctx.back(f.back.as_deref(), "/board");
    let name = f.mc_username.trim();
    if !st.db.has_claimed_slot(season, my_id).await {
        return render::notice(
            "Claim a game this season first — the whitelist is for players with a slot of their own.",
            false,
            &back,
        )
        .into_response();
    }
    if !valid_mc_username(name) {
        return render::notice(
            "That doesn't look like a Minecraft username (3–16 letters, digits or underscores).",
            false,
            &back,
        )
        .into_response();
    }
    match st.db.set_mc_username(my_id, name).await {
        Ok(db::SetMcOutcome::TakenByOther) => render::notice(
            "Someone else already registered that Minecraft username. If it's really yours, ask an admin.",
            false,
            &back,
        )
        .into_response(),
        Ok(db::SetMcOutcome::Ok) => {
            export_mc_whitelist(&st).await;
            Redirect::to(&back).into_response()
        }
        Err(_) => render::notice("Couldn't save that right now — try again.", false, &back)
            .into_response(),
    }
}

pub(crate) async fn admin_mc(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<SignupsForm>,
) -> Redirect {
    let (_me, is_admin) = ctx.ident(&me);
    if is_admin {
        let _ = st.db.set_mc_enabled(ctx.id, f.open == "1").await;
        export_mc_whitelist(&st).await;
    }
    Redirect::to(&ctx.url("/admin"))
}

/// Serve a staged Minecraft client asset (the NeoForge installer or the AP mod)
/// for download from the common-game page. Lives under `/game`, so it's SSO-gated
/// to archipelago_users like the rest. `which` is "neoforge" or "mod".
pub(crate) async fn mc_asset(Path(which): Path<String>) -> Response {
    match tokio::task::spawn_blocking(move || generate::read_client_asset(&which))
        .await
        .ok()
        .flatten()
    {
        Some((name, data)) => download_response(name, data),
        None => render::notice(
            "That download isn't available yet — ask an admin to stage it.",
            false,
            "board",
        )
        .into_response(),
    }
}
