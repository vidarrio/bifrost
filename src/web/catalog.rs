//! The apworld catalog console.

use super::*;

pub(crate) async fn catalog_page(
    st: &AppState,
    err: Option<&str>,
    saved: Option<&str>,
) -> Html<String> {
    render::catalog_page(
        &st.db.catalog().await,
        &worlds::offers(),
        st.db.approval().await,
        err,
        saved,
    )
}

pub(crate) async fn catalog_view(State(st): State<AppState>, me: auth::Me) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    catalog_page(&st, None, None).await.into_response()
}

#[derive(Deserialize, Default)]
pub(crate) struct AuditQuery {
    #[serde(default)]
    user: String,
    #[serde(default)]
    season: String,
    #[serde(default)]
    action: String,
    page: Option<i64>,
    per: Option<i64>,
    #[serde(default)]
    fit: String,
}

/// The moderator log (sysadmins only).
pub(crate) async fn audit_view(
    State(st): State<AppState>,
    me: auth::Me,
    Query(q): Query<AuditQuery>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    let f = db::ModLogFilter {
        username: q.user.trim().to_string(),
        season: q.season.trim().to_string(),
        action: q.action.trim().to_string(),
        page: 1,
        per: q.per.unwrap_or(0),
        // `<width>x<height>`, numbers only.
        fit: q
            .fit
            .split_once('x')
            .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
            .map(|(w, h)| format!("{w}x{h}"))
            .unwrap_or_default(),
    };
    // A page past the last shows the last.
    let total = st.db.mod_actions_count(&f).await;
    let pages = ((total + f.per() - 1) / f.per()).max(1);
    let f = db::ModLogFilter {
        page: q.page.unwrap_or(1).clamp(1, pages),
        ..f
    };
    let rows = st.db.mod_actions(&f).await;
    let choices = st.db.mod_log_choices().await;
    render::audit_page(&rows, &choices, &f, total).into_response()
}

pub(crate) async fn catalog_upload(
    State(st): State<AppState>,
    me: auth::Me,
    mut mp: Multipart,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    let (mut bytes, mut fname) = (None::<Vec<u8>>, String::new());
    let (mut game, mut version, mut note) = (String::new(), String::new(), String::new());
    let mut public_note = String::new();
    while let Ok(Some(field)) = mp.next_field().await {
        match field.name().unwrap_or("") {
            "apworld" => {
                fname = field.file_name().unwrap_or("").to_string();
                bytes = field.bytes().await.ok().map(|b| b.to_vec());
            }
            "game" => game = field.text().await.unwrap_or_default(),
            "version" => version = field.text().await.unwrap_or_default(),
            "note" => note = field.text().await.unwrap_or_default(),
            "public_note" => public_note = field.text().await.unwrap_or_default(),
            _ => {
                let _ = field.bytes().await;
            }
        }
    }
    let res = async {
        let bytes = bytes
            .filter(|b| !b.is_empty())
            .ok_or("No file received.".to_string())?;
        if !worlds::looks_like_apworld(&bytes) {
            return Err("That doesn't look like a valid .apworld (zip) file.".to_string());
        }
        let file = worlds::upload_file(&fname).map_err(str::to_string)?;
        let who = me.username();
        worlds::catalog_add(
            &st.db,
            &file,
            &bytes,
            &game,
            &version,
            &note,
            &public_note,
            &who,
        )
        .await
    }
    .await;
    match res {
        Ok(added) => catalog_page(&st, None, Some(&added.message()))
            .await
            .into_response(),
        Err(e) => catalog_page(&st, Some(&e), None).await.into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct CatalogRemoveForm {
    pub(crate) sha256: String,
}

pub(crate) async fn catalog_remove(
    State(st): State<AppState>,
    me: auth::Me,
    Form(f): Form<CatalogRemoveForm>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    match worlds::catalog_remove(&st.db, &f.sha256).await {
        Ok(()) => Redirect::to("/admin/catalog").into_response(),
        Err(e) => catalog_page(&st, Some(&e), None).await.into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct CatalogNotesForm {
    pub(crate) sha256: String,
    #[serde(default)]
    pub(crate) note: String,
    #[serde(default)]
    pub(crate) public_note: String,
}

/// Change a catalog world's notes: the sysadmins' own, and the public one
/// players see on the Games page.
pub(crate) async fn catalog_notes(
    State(st): State<AppState>,
    me: auth::Me,
    Form(f): Form<CatalogNotesForm>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    let (note, public_note) = (
        worlds::clean_note(&f.note),
        worlds::clean_note(&f.public_note),
    );
    match st
        .db
        .catalog_set_notes(f.sha256.trim(), &note, &public_note)
        .await
    {
        Ok(true) => catalog_page(&st, None, Some("Notes saved."))
            .await
            .into_response(),
        Ok(false) => catalog_page(&st, Some("That world isn't in the catalog."), None)
            .await
            .into_response(),
        Err(e) => catalog_page(&st, Some(&e.to_string()), None)
            .await
            .into_response(),
    }
}

/// Promote a player's newer version of a catalog game into the catalog,
/// replacing the game's current entry.
pub(crate) async fn catalog_offer_promote(
    State(st): State<AppState>,
    me: auth::Me,
    Form(f): Form<CatalogRemoveForm>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    match worlds::offer_promote(&st.db, &f.sha256, &me.username()).await {
        Ok(added) => {
            tracing::info!("{}: {}", me.username(), added.message());
            catalog_page(&st, None, Some(&added.message()))
                .await
                .into_response()
        }
        Err(e) => catalog_page(&st, Some(&e), None).await.into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct OfferQuery {
    pub(crate) sha256: String,
}

/// Download an offer's file, to vet it before promoting it.
pub(crate) async fn catalog_offer_download(
    State(st): State<AppState>,
    me: auth::Me,
    Query(q): Query<OfferQuery>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    match worlds::offer_file(&q.sha256).await {
        Ok((file, bytes)) => download_response(&file, bytes),
        Err(e) => catalog_page(&st, Some(&e), None).await.into_response(),
    }
}

pub(crate) async fn catalog_offer_dismiss(
    State(st): State<AppState>,
    me: auth::Me,
    Form(f): Form<CatalogRemoveForm>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    match worlds::offer_dismiss(&f.sha256).await {
        Ok(()) => Redirect::to("/admin/catalog").into_response(),
        Err(e) => catalog_page(&st, Some(&e), None).await.into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct ApprovalForm {
    pub(crate) mode: String,
}

pub(crate) async fn catalog_approval(
    State(st): State<AppState>,
    me: auth::Me,
    Form(f): Form<ApprovalForm>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    let Some(mode) = db::Approval::parse(&f.mode) else {
        return catalog_page(&st, Some("Unknown approval mode."), None)
            .await
            .into_response();
    };
    match st.db.set_approval(mode).await {
        Ok(()) => Redirect::to("/admin/catalog").into_response(),
        Err(e) => catalog_page(&st, Some(&e.to_string()), None)
            .await
            .into_response(),
    }
}
