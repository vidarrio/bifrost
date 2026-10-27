//! The instance settings page: home-page settings, Archipelago versions, backups, load.

use super::*;

/// Every season, plus the create form (instance level: no season scope).
/// Moderators see them all but get admin links only for the ones they run.
pub(crate) async fn settings_view(State(st): State<AppState>, me: auth::Me) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return render::notice("Sysadmins only.", false, "/").into_response();
    }
    let inst = st.db.instance().await;
    render::settings_page(&inst, &settings_status(&st).await, None, false).into_response()
}

/// What the settings page reports about the deployment (read-only there).
pub(crate) async fn settings_status(st: &AppState) -> render::SettingsStatus {
    let cfg = config::get();
    let sb = sandbox::get();
    let seasons = st.db.seasons().await;
    let installs = apinstall::installed()
        .into_iter()
        .map(|i| {
            let users = apinstall::pinned_to(&i.version)
                .into_iter()
                .filter_map(|id| seasons.iter().find(|s| s.id == id))
                .map(|s| format!("{} ({})", s.name, s.state().as_str()))
                .collect();
            (i, users)
        })
        .collect();
    let names = seasons.iter().map(|s| (s.id, s.name.clone())).collect();
    render::SettingsStatus {
        load_html: render::load_panel(load::snapshot().as_ref(), &names),
        installs,
        active_ap: apinstall::active().version,
        updates: apupdate::snapshot(),
        backups: backup::list(),
        backup_policy: backup::Policy::from_config(),
        backup_dir: backup::dir().display().to_string(),
        backup_running: backup::running(),
        backup_last: backup::last(),
        auth_mode: st.auth.mode.as_str().to_string(),
        isolation: sb.posture(),
        strong_isolation: sb.strong(),
        managed: cfg.ap_managed,
        ports: (*cfg.ap_ports.start(), *cfg.ap_ports.end()),
    }
}

/// The live-load panel alone (the settings page refreshes it).
pub(crate) async fn settings_load(State(st): State<AppState>, me: auth::Me) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }
    let names = st
        .db
        .seasons()
        .await
        .into_iter()
        .map(|s| (s.id, s.name))
        .collect();
    Html(render::load_panel(load::snapshot().as_ref(), &names)).into_response()
}

// ---- Archipelago installs (settings page) ----

pub(crate) async fn settings_notice(st: &AppState, err: Option<&str>) -> Response {
    let inst = st.db.instance().await;
    render::settings_page(&inst, &settings_status(st).await, err, false).into_response()
}

pub(crate) async fn ap_check(me: auth::Me) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return render::notice("Sysadmins only.", false, "/").into_response();
    }
    apupdate::check_releases().await;
    Redirect::to("/admin/settings#ap").into_response()
}

#[derive(Deserialize)]
pub(crate) struct ApVersionForm {
    pub(crate) version: String,
}

pub(crate) async fn ap_install(
    State(st): State<AppState>,
    me: auth::Me,
    Form(f): Form<ApVersionForm>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return render::notice("Sysadmins only.", false, "/").into_response();
    }
    match apupdate::start_install(f.version.trim()) {
        Ok(()) => Redirect::to("/admin/settings#ap").into_response(),
        Err(e) => settings_notice(&st, Some(&e)).await,
    }
}

/// Make a version the active one: signup checks and new generations use it.
pub(crate) async fn ap_use(
    State(st): State<AppState>,
    me: auth::Me,
    Form(f): Form<ApVersionForm>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return render::notice("Sysadmins only.", false, "/").into_response();
    }
    let Some(i) = apinstall::find(f.version.trim()) else {
        return settings_notice(&st, Some("That version isn't installed.")).await;
    };
    let v = (!i.bundled).then_some(i.version.clone());
    if let Err(e) = st.db.set_ap_active(v.as_deref()).await {
        return settings_notice(&st, Some(&e.to_string())).await;
    }
    apinstall::set_active(v);
    tracing::info!("{} made Archipelago {} active", me.username(), i.version);
    Redirect::to("/admin/settings#ap").into_response()
}

pub(crate) async fn ap_remove(
    State(st): State<AppState>,
    me: auth::Me,
    Form(f): Form<ApVersionForm>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return render::notice("Sysadmins only.", false, "/").into_response();
    }
    match apupdate::remove(f.version.trim()).await {
        Ok(()) => Redirect::to("/admin/settings#ap").into_response(),
        Err(e) => settings_notice(&st, Some(&e)).await,
    }
}

// ---- backups (settings page) ----

pub(crate) async fn backup_now(State(st): State<AppState>, me: auth::Me) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return render::notice("Sysadmins only.", false, "/").into_response();
    }
    if !backup::running() {
        let db = st.db.clone();
        tokio::spawn(async move {
            let _ = backup::create(&db, &backup::Policy::from_config()).await;
        });
    }
    Redirect::to("/admin/settings#backups").into_response()
}

#[derive(Deserialize)]
pub(crate) struct BackupName {
    pub(crate) name: String,
}

pub(crate) async fn backup_download(
    me: auth::Me,
    Query(q): Query<BackupName>,
    req: axum::extract::Request,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return render::notice("Sysadmins only.", false, "/").into_response();
    }
    let Some(path) = backup::path(&q.name) else {
        return (axum::http::StatusCode::NOT_FOUND, "no such backup").into_response();
    };
    download(path, &q.name, req).await
}

pub(crate) async fn backup_delete(
    State(st): State<AppState>,
    me: auth::Me,
    Form(f): Form<BackupName>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return render::notice("Sysadmins only.", false, "/").into_response();
    }
    match backup::delete(&f.name) {
        Ok(()) => Redirect::to("/admin/settings#backups").into_response(),
        Err(e) => settings_notice(&st, Some(&e)).await,
    }
}

#[derive(Deserialize)]
pub(crate) struct SettingsForm {
    #[serde(default)]
    pub(crate) domain: String,
    #[serde(default)]
    pub(crate) home_url: String,
    #[serde(default)]
    pub(crate) password_note: String,
    /// One server per line: `Name | https://discord.gg/… | note`.
    #[serde(default)]
    pub(crate) discords: String,
}

pub(crate) async fn settings_save(
    State(st): State<AppState>,
    me: auth::Me,
    Form(f): Form<SettingsForm>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return render::notice("Sysadmins only.", false, "/").into_response();
    }
    match parse_settings(&f) {
        Ok(inst) => match st.db.set_instance(&inst).await {
            Ok(()) => render::settings_page(&inst, &settings_status(&st).await, None, true)
                .into_response(),
            Err(e) => render::notice(&format!("Couldn't save: {e}"), false, "/admin/settings")
                .into_response(),
        },
        Err(e) => {
            // Re-show what they typed, with the problem.
            let typed = db::Instance {
                domain: f.domain.clone(),
                home_url: f.home_url.clone(),
                password_note: f.password_note.clone(),
                discords: f
                    .discords
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| db::Discord {
                        name: l.to_string(),
                        url: String::new(),
                        note: String::new(),
                    })
                    .collect(),
            };
            render::settings_page(&typed, &settings_status(&st).await, Some(&e), false)
                .into_response()
        }
    }
}

pub(crate) fn web_url(u: &str) -> bool {
    (u.starts_with("https://") || u.starts_with("http://"))
        && u.len() <= 300
        && !u
            .chars()
            .any(|c| c.is_whitespace() || c == '"' || c == '<' || c == '>')
}

pub(crate) fn parse_settings(f: &SettingsForm) -> Result<db::Instance, String> {
    let domain = f.domain.trim().to_string();
    let host_ok = |h: &str| {
        h.len() <= 253
            && h.chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-[]:".contains(c))
    };
    if !host_ok(&domain) {
        return Err("The connect host should be a bare hostname or IP, like ap.example.org (no scheme, no port).".into());
    }
    let home_url = f.home_url.trim().to_string();
    if !home_url.is_empty() && !web_url(&home_url) {
        return Err("The home link must be an http(s) URL.".into());
    }
    let password_note: String = f.password_note.trim().chars().take(200).collect();
    let mut discords = Vec::new();
    for (i, line) in f
        .discords
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let mut parts = line.splitn(3, '|').map(str::trim);
        let (name, url, note) = (
            parts.next().unwrap_or(""),
            parts.next().unwrap_or(""),
            parts.next().unwrap_or(""),
        );
        if name.is_empty() || !web_url(url) {
            return Err(format!(
                "Discord line {}: expected “Name | https://discord.gg/… | optional note”.",
                i + 1
            ));
        }
        discords.push(db::Discord {
            name: name.chars().take(60).collect(),
            url: url.to_string(),
            note: note.chars().take(80).collect(),
        });
    }
    if discords.len() > 20 {
        return Err("At most 20 Discord servers.".into());
    }
    Ok(db::Instance {
        domain,
        home_url,
        password_note,
        discords,
    })
}
