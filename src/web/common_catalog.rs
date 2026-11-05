//! The console's catalog of common games: sysadmins make, edit and delete
//! setups (see [`crate::common_edit`]).

use super::*;
use crate::common_edit;

const NO_SANDBOX: &str = "Apworlds can't be uploaded here: this host has no sandbox for custom \
     worlds. Install bubblewrap or use the container image, or put the apworld in the \
     setup's folder yourself.";

fn root() -> std::path::PathBuf {
    config::get().common_dir()
}

fn sysadmin_only() -> Response {
    render::notice("Sysadmins only.", false, "/").into_response()
}

async fn list_page(st: &AppState, err: Option<&str>, saved: Option<&str>) -> Response {
    let mut rows = Vec::new();
    for (key, s) in crate::common::catalog() {
        let seasons = st.db.common_in_use(&key).await;
        rows.push(match s {
            Ok(s) => render::CommonCatalogRow {
                key,
                name: s.name,
                slot: s.slot,
                problem: None,
                seasons,
            },
            Err(e) => render::CommonCatalogRow {
                key,
                problem: Some(e),
                seasons,
                ..Default::default()
            },
        });
    }
    render::common_catalog_page(&rows, err, saved).into_response()
}

async fn setup_page(st: &AppState, key: &str, err: Option<&str>, saved: Option<&str>) -> Response {
    let r = root();
    let Ok(m) = crate::common::manifest_from(&r, key).or_else(|e| {
        // A folder whose game.toml can't be read is still there to fix.
        if crate::common::valid_key(key) && r.join(key).is_dir() {
            Ok(crate::common::Manifest::default())
        } else {
            Err(e)
        }
    }) else {
        return list_page(st, Some("That setup isn't in the catalog."), None).await;
    };
    let problem = crate::common::load_from(&r, key).err();
    let ident = m
        .identifier
        .clone()
        .map(|i| (i.kind, i.label, i.pattern, i.hint))
        .unwrap_or_default();
    let view = render::CommonSetupView {
        key: key.to_string(),
        fields: common_edit::Fields {
            guide: crate::common::text_from(&r, key, m.guide_file()).unwrap_or_default(),
            yaml: crate::common::text_from(&r, key, m.yaml_file()).unwrap_or_default(),
            name: m.name.clone(),
            slot: m.slot.clone(),
            address: m.address.clone(),
            ident,
        },
        uploaded: m
            .apworld
            .as_deref()
            .is_some_and(|w| crate::common::uploaded_from(&r, key, w)),
        can_upload_world: sandbox::get().custom_worlds_allowed(),
        unreadable: crate::common::manifest_from(&r, key).is_err(),
        apworld: m.apworld.clone(),
        downloads: m
            .download
            .iter()
            .map(|d| (d.file.clone(), d.label.clone()))
            .collect(),
        problem,
        seasons: st.db.common_in_use(key).await,
    };
    render::common_setup_page(&view, err, saved).into_response()
}

pub(crate) async fn common_catalog_view(State(st): State<AppState>, me: auth::Me) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    list_page(&st, None, None).await
}

#[derive(Deserialize)]
pub(crate) struct NewSetupForm {
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) slot: String,
    pub(crate) yaml: String,
}

pub(crate) async fn common_catalog_new(
    State(st): State<AppState>,
    me: auth::Me,
    Form(f): Form<NewSetupForm>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    let key = f.key.trim().to_string();
    let yaml = f.yaml.replace("\r\n", "\n");
    let made = tokio::task::spawn_blocking({
        let key = key.clone();
        move || {
            let r = root();
            std::fs::create_dir_all(&r).map_err(|e| e.to_string())?;
            common_edit::create(&r, &key, &f.name, &f.slot, yaml.as_bytes())
        }
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    match made {
        Ok(()) => {
            tracing::info!("common game {key} made by {}", me.username());
            Redirect::to(&format!("/admin/common-games/{key}")).into_response()
        }
        Err(e) => list_page(&st, Some(&format!("Couldn't make it: {e}")), None).await,
    }
}

pub(crate) async fn common_setup_view(
    State(st): State<AppState>,
    me: auth::Me,
    Path(key): Path<String>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    setup_page(&st, &key, None, None).await
}

#[derive(Deserialize)]
pub(crate) struct SetupForm {
    pub(crate) name: String,
    pub(crate) slot: String,
    #[serde(default)]
    pub(crate) address: String,
    #[serde(default)]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) label: String,
    #[serde(default)]
    pub(crate) pattern: String,
    #[serde(default)]
    pub(crate) hint: String,
    #[serde(default)]
    pub(crate) guide: String,
    pub(crate) yaml: String,
}

pub(crate) async fn common_setup_save(
    State(st): State<AppState>,
    me: auth::Me,
    Path(key): Path<String>,
    Form(f): Form<SetupForm>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    let fields = common_edit::Fields {
        name: f.name,
        slot: f.slot,
        address: f.address,
        ident: (f.kind, f.label, f.pattern, f.hint),
        guide: f.guide,
        yaml: f.yaml,
    };
    let k = key.clone();
    let saved = tokio::task::spawn_blocking(move || common_edit::save(&root(), &k, &fields))
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
    match saved {
        Ok(()) => {
            tracing::info!("common game {key} edited by {}", me.username());
            setup_page(&st, &key, None, Some("Saved.")).await
        }
        Err(e) => {
            setup_page(
                &st,
                &key,
                Some(&format!("Not saved, nothing changed: {e}")),
                None,
            )
            .await
        }
    }
}

/// Stream an upload's `file` field into the setup's folder; returns the
/// temporary file, the uploaded name and the `label` field.
async fn receive(
    key: &str,
    mut mp: Multipart,
) -> Result<(std::path::PathBuf, String, String), String> {
    use tokio::io::AsyncWriteExt;
    let tmp = common_edit::upload_path(&root(), key)?;
    let (mut name, mut label, mut got) = (String::new(), String::new(), false);
    let result: Result<(), String> = async {
        while let Some(mut field) = mp.next_field().await.map_err(|e| e.to_string())? {
            match field.name().unwrap_or("") {
                "file" => {
                    name = field.file_name().unwrap_or("").to_string();
                    let mut out = tokio::fs::File::create(&tmp)
                        .await
                        .map_err(|e| e.to_string())?;
                    while let Some(chunk) = field.chunk().await.map_err(|e| e.to_string())? {
                        out.write_all(&chunk).await.map_err(|e| e.to_string())?;
                    }
                    out.flush().await.map_err(|e| e.to_string())?;
                    got = true;
                }
                "label" => label = field.text().await.unwrap_or_default(),
                _ => {
                    let _ = field.bytes().await;
                }
            }
        }
        Ok(())
    }
    .await;
    let empty = tokio::fs::metadata(&tmp)
        .await
        .map_or(true, |m| m.len() == 0);
    if let Err(e) = result.and_then(|_| {
        if !got || empty {
            Err("No file received.".to_string())
        } else {
            Ok(())
        }
    }) {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    Ok((tmp, name, label))
}

async fn put_upload(
    st: &AppState,
    me: &auth::Me,
    key: String,
    mp: Multipart,
    download: bool,
) -> Response {
    let res = async {
        let (tmp, name, label) = receive(&key, mp).await?;
        let file = match common_edit::file_name(&name) {
            Ok(f) => f,
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                return Err(e);
            }
        };
        if download && label.trim().is_empty() {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err("A download needs a label.".to_string());
        }
        let k = key.clone();
        let t = tmp.clone();
        let r = tokio::task::spawn_blocking(move || {
            common_edit::put(&root(), &k, &t, &file, download.then_some(label.as_str()))
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        let _ = tokio::fs::remove_file(&tmp).await;
        r
    }
    .await;
    match res {
        Ok(()) => {
            tracing::info!(
                "common game {key}: {} added by {}",
                if download {
                    "a download"
                } else {
                    "its apworld"
                },
                me.username()
            );
            setup_page(st, &key, None, Some("Added.")).await
        }
        Err(e) => setup_page(st, &key, Some(&format!("Not added: {e}")), None).await,
    }
}

pub(crate) async fn common_setup_apworld(
    State(st): State<AppState>,
    me: auth::Me,
    Path(key): Path<String>,
    mp: Multipart,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    // An apworld uploaded here is custom code, like a player's upload.
    if !sandbox::get().custom_worlds_allowed() {
        return setup_page(&st, &key, Some(NO_SANDBOX), None).await;
    }
    put_upload(&st, &me, key, mp, false).await
}

pub(crate) async fn common_setup_download(
    State(st): State<AppState>,
    me: auth::Me,
    Path(key): Path<String>,
    mp: Multipart,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    put_upload(&st, &me, key, mp, true).await
}

#[derive(Deserialize)]
pub(crate) struct RemoveForm {
    pub(crate) what: String,
    #[serde(default)]
    pub(crate) file: String,
}

pub(crate) async fn common_setup_remove(
    State(st): State<AppState>,
    me: auth::Me,
    Path(key): Path<String>,
    Form(f): Form<RemoveForm>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    let which = match f.what.as_str() {
        "apworld" => None,
        "download" if !f.file.is_empty() => Some(f.file),
        _ => return setup_page(&st, &key, Some("Remove what?"), None).await,
    };
    let k = key.clone();
    let r = tokio::task::spawn_blocking(move || common_edit::remove(&root(), &k, which.as_deref()))
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
    match r {
        Ok(()) => setup_page(&st, &key, None, Some("Removed.")).await,
        Err(e) => setup_page(&st, &key, Some(&format!("Not removed: {e}")), None).await,
    }
}

pub(crate) async fn common_setup_delete(
    State(st): State<AppState>,
    me: auth::Me,
    Path(key): Path<String>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    let seasons = st.db.common_in_use(&key).await;
    if !seasons.is_empty() {
        return setup_page(
            &st,
            &key,
            Some(&format!("{} still include it.", seasons.join(", "))),
            None,
        )
        .await;
    }
    let k = key.clone();
    let r = tokio::task::spawn_blocking(move || common_edit::delete(&root(), &k))
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
    match r {
        Ok(()) => {
            tracing::info!("common game {key} deleted by {}", me.username());
            list_page(&st, None, Some(&format!("Deleted {key}."))).await
        }
        Err(e) => list_page(&st, Some(&format!("Couldn't delete it: {e}")), None).await,
    }
}
