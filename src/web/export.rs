//! The common-game export: what a sysadmin's own sync needs from Bifrost to
//! run the servers of live seasons' common games, behind a read-only token:
//! which common games are live, each one's allowlist, and the seed's files for
//! its slot. Bifrost never reaches those servers; a script with the token does,
//! e.g. `curl -H "Authorization: Bearer $TOKEN" https://bifrost.example/export/commons`.

use super::*;
use crate::id::Id;

/// Whether the request carries the export token. Off until a sysadmin makes one.
async fn authorized(st: &AppState, headers: &axum::http::HeaderMap) -> bool {
    let Some(want) = st.db.export_token_hash().await else {
        return false;
    };
    let Some(token) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, t)| t.trim())
        .filter(|t| !t.is_empty())
    else {
        return false;
    };
    let got = auth::token_hash(token);
    // Both are hex SHA-256: compare without stopping at the first difference.
    got.len() == want.len()
        && got
            .bytes()
            .zip(want.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

fn refused() -> Response {
    let mut r = (
        axum::http::StatusCode::UNAUTHORIZED,
        "export token missing or wrong\n",
    )
        .into_response();
    r.headers_mut().insert(
        axum::http::header::WWW_AUTHENTICATE,
        axum::http::HeaderValue::from_static("Bearer"),
    );
    r
}

fn not_found() -> Response {
    (
        axum::http::StatusCode::NOT_FOUND,
        "no such live common game\n",
    )
        .into_response()
}

/// A live season's common game, by season id and key.
async fn live_common(
    st: &AppState,
    season: Id,
    key: &str,
) -> Option<(db::SeasonRow, db::SeasonCommon)> {
    let row = st.db.season(season).await?;
    if !row.state().is_live() {
        return None;
    }
    let common = st
        .db
        .season_commons(season)
        .await
        .into_iter()
        .find(|c| c.key == key)?;
    Some((row, common))
}

/// The files of a season's current seed.
async fn outputs(season: Id) -> Vec<generate::OutputFile> {
    tokio::task::spawn_blocking(move || generate::list_outputs(season))
        .await
        .unwrap_or_default()
}

/// Of the seed's files, a season's common game's: its slot's per-player files.
fn slot_files<'a>(
    slots: &[db::SlotRow],
    outputs: &'a [generate::OutputFile],
    key: &str,
) -> Vec<&'a generate::OutputFile> {
    let Some(ap_slot) = slots
        .iter()
        .find(|r| r.common_key.as_deref() == Some(key))
        .map(|r| r.ap_slot)
    else {
        return Vec::new();
    };
    outputs
        .iter()
        .filter(|o| o.player == Some(ap_slot) && !o.is_spoiler)
        .collect()
}

/// Every live season's common games, with where to fetch each one's
/// allowlist and files. The seed name changes on a re-roll, which is when a
/// server needs its new file.
pub(crate) async fn export_commons(
    State(st): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Response {
    if !authorized(&st, &headers).await {
        return refused();
    }
    let mut seasons = Vec::new();
    for row in st.db.seasons().await {
        if !row.state().is_live() {
            continue;
        }
        let commons = season_commons(&st, row.id).await;
        if commons.is_empty() {
            continue;
        }
        let slots = st.db.slots(row.id).await.unwrap_or_default();
        let seed_files = outputs(row.id).await;
        let mut out = Vec::new();
        for c in commons {
            let setup = crate::common::load(&c.key).ok();
            let ident = setup.as_ref().and_then(|s| s.identifier.as_ref());
            let base = format!("/export/{}/{}", row.id, c.key);
            let files: Vec<serde_json::Value> = slot_files(&slots, &seed_files, &c.key)
                .into_iter()
                .map(|f| {
                    serde_json::json!({
                        // As `file?name=` takes it: its path in the seed.
                        "name": f.member,
                        "filename": f.member.rsplit('/').next().unwrap_or(&f.member),
                        "size": f.size,
                        "url": format!("{base}/file?name={}", url_encode(&f.member)),
                    })
                })
                .collect();
            let allowlist = match ident {
                Some(id) => serde_json::json!({
                    "kind": id.kind,
                    "count": allowlist(&st, row.id, id).await.len(),
                    "url": format!("{base}/allowlist"),
                }),
                None => serde_json::Value::Null,
            };
            out.push(serde_json::json!({
                "key": c.key,
                "name": c.name,
                "slot": c.slot_name,
                "address": c.address,
                "allowlist": allowlist,
                "files": files,
            }));
        }
        seasons.push(serde_json::json!({
            "id": row.id.to_string(),
            "name": row.name,
            "seed": row.seed,
            "commons": out,
        }));
    }
    Json(serde_json::json!({ "seasons": seasons })).into_response()
}

/// A live season's allowlist for one of its common games: one name per line,
/// or a JSON array with `?format=json`.
pub(crate) async fn export_allowlist(
    State(st): State<AppState>,
    headers: axum::http::HeaderMap,
    Path((season, key)): Path<(String, String)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if !authorized(&st, &headers).await {
        return refused();
    }
    let Ok(season) = season.parse::<Id>() else {
        return not_found();
    };
    let q = AllowlistQuery {
        format: q.get("format").cloned().unwrap_or_default(),
    };
    if live_common(&st, season, &key).await.is_none() {
        return not_found();
    }
    let Some(id) = crate::common::load(&key).ok().and_then(|s| s.identifier) else {
        return not_found();
    };
    let names = allowlist(&st, season, &id).await;
    allowlist_response(season, &key, &names, &q.format)
}

/// One of the seed's files for a live season's common game, by its name as
/// the index lists it.
pub(crate) async fn export_file(
    State(st): State<AppState>,
    headers: axum::http::HeaderMap,
    Path((season, key)): Path<(String, String)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if !authorized(&st, &headers).await {
        return refused();
    }
    let (Ok(season), Some(name)) = (season.parse::<Id>(), q.get("name")) else {
        return not_found();
    };
    if live_common(&st, season, &key).await.is_none() {
        return not_found();
    }
    let slots = st.db.slots(season).await.unwrap_or_default();
    let seed_files = outputs(season).await;
    if !slot_files(&slots, &seed_files, &key)
        .iter()
        .any(|f| &f.member == name)
    {
        return not_found();
    }
    let member = name.clone();
    match tokio::task::spawn_blocking(move || generate::read_output(season, &member))
        .await
        .ok()
        .flatten()
    {
        Some(data) => download_response(name, data),
        None => not_found(),
    }
}

/// Percent-encode a query value.
fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[derive(Deserialize)]
pub(crate) struct ExportTokenForm {
    #[serde(rename = "do")]
    pub(crate) action: String,
}

/// A sysadmin makes (or replaces) the export token, shown this once, or
/// revokes it. Only its hash is kept.
pub(crate) async fn admin_export_token(
    State(st): State<AppState>,
    me: auth::Me,
    Form(f): Form<ExportTokenForm>,
) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return sysadmin_only();
    }
    match f.action.as_str() {
        "new" => {
            let token = auth::new_token();
            if let Err(e) = st
                .db
                .set_export_token_hash(Some(&auth::token_hash(&token)))
                .await
            {
                tracing::warn!("saving the export token: {e}");
                return render::notice("Couldn't save the token.", false, "/admin/settings")
                    .into_response();
            }
            tracing::info!("export token replaced by {}", me.username());
            render::export_token_page(&token).into_response()
        }
        "revoke" => {
            if let Err(e) = st.db.set_export_token_hash(None).await {
                tracing::warn!("revoking the export token: {e}");
                return render::notice("Couldn't revoke the token.", false, "/admin/settings")
                    .into_response();
            }
            tracing::info!("export token revoked by {}", me.username());
            Redirect::to("/admin/settings#export").into_response()
        }
        _ => Redirect::to("/admin/settings").into_response(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn query_values_are_encoded() {
        assert_eq!(
            super::url_encode("AP_1/AP_1_P3_Vanilla.apmc"),
            "AP_1%2FAP_1_P3_Vanilla.apmc"
        );
        assert_eq!(super::url_encode("a b&c"), "a%20b%26c");
    }
}
