//! The apworld catalog console.

use super::*;

pub(crate) async fn catalog_page(st: &AppState, err: Option<&str>, saved: Option<&str>) -> Html<String> {
    render::catalog_page(&st.db.catalog().await, st.db.approval().await, err, saved)
}

pub(crate) async fn catalog_view(State(st): State<AppState>, me: auth::Me) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return render::notice("Sysadmins only.", false, "/").into_response();
    }
    catalog_page(&st, None, None).await.into_response()
}

pub(crate) async fn catalog_upload(State(st): State<AppState>, me: auth::Me, mut mp: Multipart) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return render::notice("Sysadmins only.", false, "/").into_response();
    }
    let (mut bytes, mut fname) = (None::<Vec<u8>>, String::new());
    let (mut game, mut version, mut note) = (String::new(), String::new(), String::new());
    while let Ok(Some(field)) = mp.next_field().await {
        match field.name().unwrap_or("") {
            "apworld" => {
                fname = field.file_name().unwrap_or("").to_string();
                bytes = field.bytes().await.ok().map(|b| b.to_vec());
            }
            "game" => game = field.text().await.unwrap_or_default(),
            "version" => version = field.text().await.unwrap_or_default(),
            "note" => note = field.text().await.unwrap_or_default(),
            _ => {
                let _ = field.bytes().await;
            }
        }
    }
    let res = async {
        let bytes = bytes.filter(|b| !b.is_empty()).ok_or("No file received.".to_string())?;
        if !worlds::looks_like_apworld(&bytes) {
            return Err("That doesn't look like a valid .apworld (zip) file.".to_string());
        }
        let file = worlds::world_file(&fname).map_err(str::to_string)?;
        let who = me.username();
        worlds::catalog_add(&st.db, &file, &bytes, &game, &version, &note, &who).await
    }
    .await;
    match res {
        Ok(w) => {
            let msg = format!("Added {} to the catalog.", w.file);
            catalog_page(&st, None, Some(&msg)).await.into_response()
        }
        Err(e) => catalog_page(&st, Some(&e), None).await.into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct CatalogRemoveForm {
    pub(crate) sha256: String,
}

pub(crate) async fn catalog_remove(State(st): State<AppState>, me: auth::Me, Form(f): Form<CatalogRemoveForm>) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return render::notice("Sysadmins only.", false, "/").into_response();
    }
    match worlds::catalog_remove(&st.db, &f.sha256).await {
        Ok(()) => Redirect::to("/admin/catalog").into_response(),
        Err(e) => catalog_page(&st, Some(&e), None).await.into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct ApprovalForm {
    pub(crate) mode: String,
}

pub(crate) async fn catalog_approval(State(st): State<AppState>, me: auth::Me, Form(f): Form<ApprovalForm>) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return render::notice("Sysadmins only.", false, "/").into_response();
    }
    let Some(mode) = db::Approval::parse(&f.mode) else {
        return catalog_page(&st, Some("Unknown approval mode."), None).await.into_response();
    };
    match st.db.set_approval(mode).await {
        Ok(()) => Redirect::to("/admin/catalog").into_response(),
        Err(e) => catalog_page(&st, Some(&e.to_string()), None).await.into_response(),
    }
}
