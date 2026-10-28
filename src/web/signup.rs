//! The season signup wizard.

use super::*;

/// `BIFROST_COOP="Cam:Abi,Foo:Bar"` — map an AP slot name to an extra Bifrost
/// slot name (a co-op seat) for the current season's seeding.
pub(crate) fn parse_coop() -> Vec<(String, String)> {
    config::var("BIFROST_COOP")
        .unwrap_or_default()
        .split(',')
        .filter_map(|p| p.split_once(':'))
        .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
        .filter(|(a, b)| !a.is_empty() && !b.is_empty())
        .collect()
}

/// Season `worlds/` file stems Bifrost itself stages (`generate.rs`).
pub(crate) fn slugify(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    if out.is_empty() { "slot".into() } else { out }
}

pub(crate) async fn signup_get(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
) -> Html<String> {
    let (me, is_admin) = ctx.ident(&me);
    let subs = st.db.submissions(ctx.id).await.unwrap_or_default();
    let open = ctx.state().accepts_signups();
    let worlds = render::SignupWorlds {
        custom_allowed: sandbox::get().custom_worlds_allowed(),
        manual: st.db.approval().await == db::Approval::Manual,
        catalog: st.db.catalog().await,
    };
    render::signup_page(&subs, &me, is_admin, open, &worlds)
}

/// Step 1 of signup: put a custom world in the season — a catalog pick
/// (`catalog` = its hash) or an uploaded `.apworld` — so step 2 can check a
/// YAML against it. An upload may instead wait for approval (`pending`), in
/// which case the YAML is held rather than checked. Returns JSON for the
/// wizard.
pub(crate) async fn signup_apworld(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    mut mp: Multipart,
) -> Json<serde_json::Value> {
    let fail = |e: &str| Json(serde_json::json!({"ok": false, "error": e}));
    // Worlds change only while the season takes signups — never under a
    // season that's being played.
    if !ctx.state().accepts_signups() {
        return fail("Signups are closed right now.");
    }
    let mut bytes: Option<Vec<u8>> = None;
    let mut fname = String::new();
    let mut catalog = String::new();
    while let Ok(Some(field)) = mp.next_field().await {
        match field.name() {
            Some("apworld") => {
                fname = field.file_name().unwrap_or("").to_string();
                bytes = field.bytes().await.ok().map(|b| b.to_vec());
            }
            Some("catalog") => catalog = field.text().await.unwrap_or_default(),
            _ => {
                let _ = field.bytes().await;
            }
        }
    }
    if !sandbox::get().custom_worlds_allowed() {
        return fail(
            "Custom apworlds are disabled on this server (no sandbox available to run them safely). Ask an admin.",
        );
    }
    let (uploader, _) = ctx.ident(&me);
    if !catalog.is_empty() {
        return match worlds::install_from_catalog(&st.db, ctx.id, catalog.trim(), &uploader).await {
            Ok(file) => Json(serde_json::json!({"ok": true, "world": file, "pending": false})),
            Err(e) => fail(&e),
        };
    }
    let Some(bytes) = bytes.filter(|b| !b.is_empty()) else {
        return fail("No file received.");
    };
    if !worlds::looks_like_apworld(&bytes) {
        return fail("That doesn't look like a valid .apworld (zip) file.");
    }
    let file = match worlds::world_file(&fname) {
        Ok(f) => f,
        Err(e) => return fail(e),
    };
    match worlds::place_upload(&st.db, ctx.id, &file, bytes, &uploader).await {
        Ok(placed) => Json(serde_json::json!({
            "ok": true, "world": file, "pending": placed == worlds::Placed::Pending
        })),
        Err(e) => {
            tracing::info!("apworld upload refused: {e}");
            fail(&e)
        }
    }
}

/// Step 2 of signup: validate a YAML live against the installed worlds (incl. an
/// apworld just installed in step 1). Returns JSON for the wizard.
pub(crate) async fn signup_check(
    ctx: scope::SeasonCtx,
    mut mp: Multipart,
) -> Json<serde_json::Value> {
    let mut yaml: Option<Vec<u8>> = None;
    while let Ok(Some(field)) = mp.next_field().await {
        if field.name() == Some("yaml") {
            yaml = field.bytes().await.ok().map(|b| b.to_vec());
        } else {
            let _ = field.bytes().await;
        }
    }
    let Some(yaml) = yaml.filter(|b| !b.is_empty()) else {
        return Json(serde_json::json!({"ok": false, "error": "No YAML received."}));
    };
    let yaml_str = String::from_utf8_lossy(&yaml).to_string();
    match checker::check_yaml(&ctx.dirs(), &yaml_str).await {
        Ok(r) => {
            let (name, game) = r
                .players
                .first()
                .map(|p| (p.name.clone(), p.game.clone().unwrap_or_default()))
                .unwrap_or_default();
            Json(serde_json::json!({
                "ok": r.ok, "name": name, "game": game, "errors": r.errors
            }))
        }
        Err(e) => Json(serde_json::json!({"ok": false, "error": e})),
    }
}

pub(crate) async fn signup_post(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    mut mp: Multipart,
) -> Html<String> {
    let my_id = me.id();
    let (_me, _) = ctx.ident(&me);
    if !ctx.state().accepts_signups() {
        return render::signup_result("Signups are closed right now.", false);
    }
    let mut yaml: Option<Vec<u8>> = None;
    let mut is_custom = false;
    let mut coop = String::new();
    let mut pending_world = String::new();
    while let Ok(Some(field)) = mp.next_field().await {
        match field.name().unwrap_or("") {
            "yaml" => yaml = field.bytes().await.ok().map(|b| b.to_vec()),
            "is_custom" => is_custom = field.text().await.unwrap_or_default() == "1",
            "coop" => coop = field.text().await.unwrap_or_default(),
            "pending_world" => pending_world = field.text().await.unwrap_or_default(),
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    let Some(yaml) = yaml.filter(|b| !b.is_empty()) else {
        return render::signup_result("No YAML uploaded.", false);
    };
    let yaml_str = String::from_utf8_lossy(&yaml).to_string();
    let held_for = pending_world.trim().to_string();
    let (slot_name, game) = if !held_for.is_empty() {
        // Its world waits for approval, so the YAML can't be checked yet: hold
        // the signup under the name it declares; it's checked on approval.
        if st.db.world_request(ctx.id, &held_for).await.is_none() {
            return render::signup_result(
                "That apworld isn't waiting for approval any more — start over.",
                false,
            );
        }
        if yaml.len() > checker::MAX_YAML {
            return render::signup_result("That YAML is too large.", false);
        }
        match worlds::yaml_name_game(&yaml_str) {
            (Some(name), game) => (name, game.unwrap_or_else(|| "?".into())),
            (None, _) => {
                return render::signup_result(
                    "Couldn't find a top-level name: in that YAML.",
                    false,
                );
            }
        }
    } else {
        // Trust-but-verify: the wizard already checked, but re-roll server-side so a
        // hand-crafted POST can't slip an invalid YAML through. Custom worlds are
        // installed by now, so everything must pass.
        let result = match checker::check_yaml(&ctx.dirs(), &yaml_str).await {
            Ok(r) => r,
            Err(e) => return render::signup_result(&format!("Checker error: {e}"), false),
        };
        let Some(p0) = result.players.first() else {
            return render::signup_result("Couldn't read a player from that YAML.", false);
        };
        if !result.ok {
            return render::signup_result(
                &format!(
                    "That YAML didn't pass: {}. Custom game? Install its apworld in step 1 first.",
                    result.errors.join("; ")
                ),
                false,
            );
        }
        (
            p0.name.clone(),
            p0.game.clone().unwrap_or_else(|| "?".into()),
        )
    };

    // A file of its own, never another signup's: names that slugify alike
    // (`Ann B`, `Ann-B`) or a refused duplicate must not touch an existing one.
    let slug = slugify(&slot_name);
    let uploads = ctx.dirs().submissions;
    let _ = tokio::fs::create_dir_all(&uploads).await;
    let nonce = &auth::new_token()[..16];
    let yaml_path = uploads
        .join(format!("{slug}-{nonce}.yaml"))
        .to_string_lossy()
        .into_owned();
    let written = async {
        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&yaml_path)
            .await?;
        f.write_all(&yaml).await
    };
    if written.await.is_err() {
        let _ = tokio::fs::remove_file(&yaml_path).await;
        return render::signup_result("Couldn't store the YAML.", false);
    }
    let coop_clean: String = coop
        .split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ");

    match st
        .db
        .add_submission(
            ctx.id,
            my_id,
            &slot_name,
            &game,
            is_custom,
            &coop_clean,
            &yaml_path,
            (!held_for.is_empty()).then_some(held_for.as_str()),
        )
        .await
    {
        Ok(_) if !held_for.is_empty() => render::signup_result(
            &format!(
                "Signed up: {slot_name} — held until a moderator approves {held_for}; your YAML is checked then."
            ),
            true,
        ),
        Ok(_) => render::signup_result(&format!("Signed up: {slot_name} — {game}."), true),
        Err(_) => {
            let _ = tokio::fs::remove_file(&yaml_path).await;
            render::signup_result(
                &format!(
                    "'{slot_name}' is already signed up — delete that entry first to resubmit."
                ),
                false,
            )
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct DelForm {
    pub(crate) id: i64,
}

pub(crate) async fn signup_delete(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    me: auth::Me,
    Form(f): Form<DelForm>,
) -> Redirect {
    let my_id = me.id();
    let (_me, is_admin) = ctx.ident(&me);
    if let Ok(Some(yaml_file)) = st.db.delete_submission(ctx.id, f.id, my_id, is_admin).await {
        let _ = tokio::fs::remove_file(&yaml_file).await;
    }
    Redirect::to(&ctx.url("/signup"))
}
