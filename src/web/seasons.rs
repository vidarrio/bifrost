//! The Seasons console: creating, importing and downloading seasons.

use super::*;

pub(crate) async fn seasons_view(State(st): State<AppState>, me: auth::Me) -> Response {
    if !me.can(auth::Action::CreateSeason) {
        return render::notice("Moderators only.", false, "/").into_response();
    }
    seasons_page(&st, &me, &default_season_name(), None).await
}

pub(crate) async fn seasons_page(st: &AppState, me: &auth::Me, name: &str, err: Option<&str>) -> Response {
    let seasons = st.db.seasons().await;
    let mut manage = Vec::with_capacity(seasons.len());
    for s in &seasons {
        let mods = st.db.season_mods(s.id).await;
        manage.push(me.can(auth::Action::ManageSeason { mods: &mods }));
    }
    render::seasons_page(&seasons, &manage, name, err, me.can(auth::Action::ManageInstance))
        .into_response()
}

/// Import a finished season from another instance's archive bundle
/// (sysadmins). The upload streams to a temp file — bundles can be large.
pub(crate) async fn seasons_import(State(st): State<AppState>, me: auth::Me, mut mp: Multipart) -> Response {
    if !me.can(auth::Action::ManageInstance) {
        return render::notice("Sysadmins only.", false, "/").into_response();
    }
    let tmp = config::get().tmp_dir().join(format!("import-{}.tar.gz", auth::new_token()));
    // Removed however this ends (an aborted upload drops this future).
    struct Upload(std::path::PathBuf);
    impl Drop for Upload {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _upload = Upload(tmp.clone());
    let res = async {
        tokio::fs::create_dir_all(config::get().tmp_dir()).await.map_err(|e| e.to_string())?;
        let mut got = false;
        while let Ok(Some(mut field)) = mp.next_field().await {
            if field.name() != Some("bundle") {
                continue;
            }
            let mut out = tokio::fs::File::create(&tmp).await.map_err(|e| e.to_string())?;
            while let Some(chunk) = field.chunk().await.map_err(|e| format!("upload: {e}"))? {
                tokio::io::AsyncWriteExt::write_all(&mut out, &chunk).await.map_err(|e| e.to_string())?;
                got = true;
            }
        }
        if !got {
            return Err("No file received.".to_string());
        }
        archive::import(&st.db, &tmp, Some(me.id()), true).await
    }
    .await;
    match res {
        Ok(id) => {
            tracing::info!("{} imported season {id}", me.username());
            Redirect::to(&format!("/s/{id}/season")).into_response()
        }
        Err(e) => seasons_page(&st, &me, &default_season_name(), Some(&format!("Import failed: {e}"))).await,
    }
}

/// An archived season's bundle (its admins), e.g. to import it elsewhere.
pub(crate) async fn admin_bundle(ctx: scope::SeasonCtx, me: auth::Me, req: axum::extract::Request) -> Response {
    let (_me, is_admin) = ctx.ident(&me);
    if !is_admin {
        return render::notice("Admins only.", false, "/").into_response();
    }
    let seed = ctx.row.seed.clone().unwrap_or_default();
    match archive::bundle_path(&seed).filter(|_| ctx.state() == db::State::Archived) {
        Some(path) => download(path, &format!("bifrost-season-{seed}.tar.gz"), req).await,
        None => (axum::http::StatusCode::NOT_FOUND, "no bundle for this season").into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct SeasonForm {
    pub(crate) name: String,
}

/// Create a season (a draft, named up front, owned by its creator) and open
/// its admin page.
pub(crate) async fn seasons_create(State(st): State<AppState>, me: auth::Me, Form(f): Form<SeasonForm>) -> Response {
    if !me.can(auth::Action::CreateSeason) {
        return render::notice("Moderators only.", false, "/").into_response();
    }
    let name: String = f.name.trim().chars().take(80).collect();
    if name.is_empty() {
        return seasons_page(&st, &me, &default_season_name(), Some("Give the season a name.")).await;
    }
    let created = st
        .db
        .create_season(&name, me.principal().map(|p| p.account_id), auth::now())
        .await;
    match created {
        Ok(id) => {
            if let Err(e) = config::get().season(id).ensure() {
                tracing::warn!("season {id} dirs: {e}");
            }
            tracing::info!("season {id} “{name}” created by {}", me.username());
            Redirect::to(&format!("/s/{id}/admin")).into_response()
        }
        Err(e) => seasons_page(&st, &me, &name, Some(&e.to_string())).await,
    }
}

/// A unix timestamp (seconds) as an ISO-8601 UTC string, for season.txt.
pub(crate) fn fmt_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (h, mi, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// A season name for a blank form: "Season <Mon YYYY>" by day's date.
pub(crate) fn default_season_name() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    // Civil date from unix seconds (UTC), Howard Hinnant's algorithm.
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    const MON: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mon = MON[((m - 1).clamp(0, 11)) as usize];
    format!("Season {mon} {year}")
}
