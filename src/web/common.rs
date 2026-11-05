//! A season's common games: choosing them, their addresses, the names players
//! register for their servers' allowlists, and their client downloads.

use super::*;
use crate::common::Setup;
use crate::db::SeasonCommon;

/// Where players join a season's common game: the season's own address, or
/// the setup's default.
pub(crate) fn common_address(row: &SeasonCommon, setup: Option<&Setup>) -> String {
    if row.address.is_empty() {
        setup.map(|s| s.address.clone()).unwrap_or_default()
    } else {
        row.address.clone()
    }
}

/// The season's common games with their addresses filled in.
pub(crate) async fn season_commons(st: &AppState, season: crate::id::Id) -> Vec<SeasonCommon> {
    let mut rows = st.db.season_commons(season).await;
    for r in &mut rows {
        r.address = common_address(r, crate::common::load(&r.key).ok().as_ref());
    }
    rows
}

/// What a common game's page shows.
pub(crate) async fn common_view(
    st: &AppState,
    season: crate::id::Id,
    key: &str,
    account: i64,
) -> render::CommonView {
    let row = st
        .db
        .season_commons(season)
        .await
        .into_iter()
        .find(|r| r.key == key);
    let setup = crate::common::load(key);
    let (name, slot) = match (&row, &setup) {
        (Some(r), _) => (r.name.clone(), r.slot_name.clone()),
        (None, Ok(s)) => (s.name.clone(), s.slot.clone()),
        (None, Err(_)) => (key.to_string(), String::new()),
    };
    let address = row
        .as_ref()
        .map(|r| common_address(r, setup.as_ref().ok()))
        .unwrap_or_default();
    let Ok(setup) = setup else {
        return render::CommonView {
            key: key.to_string(),
            name,
            address,
            ..Default::default()
        };
    };
    let ident = match &setup.identifier {
        Some(id) => Some(render::CommonIdent {
            label: id.label.clone(),
            pattern: id.pattern.clone(),
            hint: id.hint.clone(),
            current: st.db.game_ident(account, &id.kind).await,
        }),
        None => None,
    };
    render::CommonView {
        key: key.to_string(),
        name,
        guide_html: crate::common::guide_html(&setup.guide, &address, &slot),
        address,
        downloads: setup.downloads.iter().map(|d| d.label.clone()).collect(),
        ident,
        in_catalog: true,
    }
}

#[derive(Deserialize)]
pub(crate) struct IdentForm {
    pub(crate) name: String,
    pub(crate) back: Option<String>,
}

/// A player registers (or changes) the name they play a common game under,
/// for its server's allowlist. Only players with a game of their own in the
/// season are on it.
pub(crate) async fn common_ident_post(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Path(key): Path<String>,
    Form(f): Form<IdentForm>,
) -> Response {
    let my_id = me.id();
    let season = ctx.id;
    let back = ctx.back(f.back.as_deref(), "/board");
    let fail = |msg: &str| render::notice(msg, false, &back).into_response();
    if !st
        .db
        .season_commons(season)
        .await
        .iter()
        .any(|r| r.key == key)
    {
        return fail("This season doesn't have that common game.");
    }
    if ctx.state() == db::State::Archived {
        return fail("This season has finished.");
    }
    let Some(id) = crate::common::load(&key).ok().and_then(|s| s.identifier) else {
        return fail("This common game doesn't take a name.");
    };
    if !st.db.has_claimed_slot(season, my_id).await {
        return fail(
            "Play a game this season first: the allowlist is for players with a game of their own.",
        );
    }
    let name = f.name.trim();
    if !id.accepts(name) {
        let why = if id.hint.is_empty() {
            String::new()
        } else {
            format!(" ({})", id.hint)
        };
        return fail(&format!("That doesn't look like a {}{why}.", id.label));
    }
    match st.db.set_game_ident(my_id, &id.kind, name).await {
        Ok(db::SetIdentOutcome::TakenByOther) => fail(&format!(
            "Someone else already registered that {}. If it's really yours, ask an admin.",
            id.label
        )),
        Ok(db::SetIdentOutcome::Ok) => Redirect::to(&back).into_response(),
        Err(e) => {
            tracing::warn!("registering a {} name: {e}", id.kind);
            fail("Couldn't save that right now. Try again.")
        }
    }
}

/// A client file a season's common game offers, by its place in the setup's
/// list.
pub(crate) async fn common_file(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    Path((key, n)): Path<(String, usize)>,
    req: axum::extract::Request,
) -> Response {
    let missing = || {
        render::notice("That download isn't available.", false, &ctx.url("/board")).into_response()
    };
    if !st
        .db
        .season_commons(ctx.id)
        .await
        .iter()
        .any(|r| r.key == key)
    {
        return missing();
    }
    let Some(d) = crate::common::load(&key)
        .ok()
        .and_then(|s| s.downloads.into_iter().nth(n))
    else {
        return missing();
    };
    let Ok(path) = crate::common::file(&key, &d.file) else {
        return missing();
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| if "\"\\\r\n".contains(c) { '_' } else { c })
        .collect::<String>();
    download(path, &name, req).await
}

#[derive(Deserialize)]
pub(crate) struct CommonAdminForm {
    #[serde(rename = "do")]
    pub(crate) action: String,
    pub(crate) key: String,
    #[serde(default)]
    pub(crate) address: String,
}

/// Add a common game to the season, drop one, or set its address. The lineup
/// only changes before generation; the address any time.
pub(crate) async fn admin_common(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<CommonAdminForm>,
) -> Response {
    let (_me, is_admin) = ctx.ident(&me);
    let admin = ctx.url("/admin");
    if !is_admin {
        return Redirect::to(&admin).into_response();
    }
    let fail = |msg: &str| render::notice(msg, false, &admin).into_response();
    let before_generation = matches!(ctx.state(), db::State::Draft | db::State::Signup);
    let season = ctx.id;
    match f.action.as_str() {
        "add" => {
            if !before_generation {
                return fail("Common games are chosen before the season is generated.");
            }
            let setup = match crate::common::load(&f.key) {
                Ok(s) => s,
                Err(e) => return fail(&format!("That setup can't be used: {e}.")),
            };
            if st.db.slot_name_taken(season, &setup.slot).await {
                return fail(&format!(
                    "{} plays as {}, a name a signup, seat or common game of this season already has.",
                    setup.name, setup.slot
                ));
            }
            if let Err(e) = st
                .db
                .add_season_common(season, &setup.key, &setup.name, &setup.slot)
                .await
            {
                return if db::name_clash(&e) {
                    fail(&format!(
                        "The season has {} already, or something else plays as {}.",
                        setup.name, setup.slot
                    ))
                } else {
                    tracing::warn!("adding a common game to season {season}: {e}");
                    fail("Couldn't add it right now.")
                };
            }
        }
        "remove" => {
            if !before_generation {
                return fail("A generated season keeps its common games.");
            }
            let _ = st.db.remove_season_common(season, &f.key).await;
        }
        "address" => {
            if ctx.state() == db::State::Archived {
                return fail("This season has finished.");
            }
            let address = f.address.trim();
            if address.chars().count() > 200 || address.chars().any(char::is_control) {
                return fail("An address is one line, up to 200 characters.");
            }
            let _ = st.db.set_common_address(season, &f.key, address).await;
        }
        _ => {}
    }
    Redirect::to(&admin).into_response()
}

#[derive(Deserialize)]
pub(crate) struct AllowlistQuery {
    #[serde(default)]
    pub(crate) format: String,
}

/// A season's allowlist for one of its common games, for the sysadmin to apply
/// to the game's server: one name per line, or a JSON array with
/// `?format=json`.
pub(crate) async fn admin_common_allowlist(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Path(key): Path<String>,
    Query(q): Query<AllowlistQuery>,
) -> Response {
    let (_me, is_admin) = ctx.ident(&me);
    let admin = ctx.url("/admin");
    if !is_admin {
        return Redirect::to(&admin).into_response();
    }
    if !st
        .db
        .season_commons(ctx.id)
        .await
        .iter()
        .any(|r| r.key == key)
    {
        return render::notice("This season doesn't have that common game.", false, &admin)
            .into_response();
    }
    let Some(id) = crate::common::load(&key).ok().and_then(|s| s.identifier) else {
        return render::notice("That common game doesn't take names.", false, &admin)
            .into_response();
    };
    let names = st.db.season_allowlist(ctx.id, &id.kind).await;
    allowlist_response(ctx.id, &key, &names, &q.format)
}

pub(crate) fn allowlist_response(
    season: crate::id::Id,
    key: &str,
    names: &[String],
    format: &str,
) -> Response {
    let (body, ext, mime) = if format == "json" {
        (
            serde_json::to_string(names).unwrap_or_else(|_| "[]".into()),
            "json",
            "application/json",
        )
    } else {
        (
            names.iter().map(|n| format!("{n}\n")).collect::<String>(),
            "txt",
            "text/plain; charset=utf-8",
        )
    };
    let mut resp = Response::new(axum::body::Body::from(body));
    let h = resp.headers_mut();
    h.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static(mime),
    );
    if let Ok(v) = axum::http::HeaderValue::from_str(&format!(
        "attachment; filename=\"{season}-{key}-allowlist.{ext}\""
    )) {
        h.insert(axum::http::header::CONTENT_DISPOSITION, v);
    }
    resp
}

/// The admin page's Common games section: the season's, and the catalog's
/// others it could add.
pub(crate) async fn common_admin(st: &AppState, ctx: &scope::SeasonCtx) -> render::CommonAdmin {
    let catalog = crate::common::catalog();
    let mut chosen = Vec::new();
    for row in st.db.season_commons(ctx.id).await {
        let setup = catalog
            .iter()
            .find(|(k, _)| *k == row.key)
            .map(|(_, s)| s.clone());
        let (problem, setup) = match setup {
            Some(Ok(s)) if !s.slot.eq_ignore_ascii_case(&row.slot_name) => (
                Some(format!(
                    "its setup plays as {} now: remove it and add it again",
                    s.slot
                )),
                Some(s),
            ),
            Some(Ok(s)) => (None, Some(s)),
            Some(Err(e)) => (Some(e), None),
            None => (Some("it isn't in the catalog anymore".to_string()), None),
        };
        let allowlist = match setup.as_ref().and_then(|s| s.identifier.as_ref()) {
            Some(id) => Some((
                id.label.clone(),
                st.db.season_allowlist(ctx.id, &id.kind).await.len(),
            )),
            None => None,
        };
        chosen.push(render::CommonAdminRow {
            default_address: setup
                .as_ref()
                .map(|s| s.address.clone())
                .unwrap_or_default(),
            key: row.key,
            name: row.name,
            slot: row.slot_name,
            address: row.address,
            allowlist,
            problem,
        });
    }
    let mut available = Vec::new();
    let mut broken = Vec::new();
    for (key, s) in catalog {
        if chosen.iter().any(|c| c.key == key) {
            continue;
        }
        match s {
            Ok(s) => available.push((key, s.name, s.slot)),
            Err(e) => broken.push((key, e)),
        }
    }
    render::CommonAdmin {
        chosen,
        available,
        broken,
        lineup_open: matches!(ctx.state(), db::State::Draft | db::State::Signup),
    }
}
