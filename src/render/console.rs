//! The instance console: seasons, settings and the apworld catalog.

use super::*;

/// Who runs a season, for its admin page.
pub struct SeasonMods {
    pub owner: Option<String>,
    pub co_mods: Vec<(i64, String)>,
}

pub(super) fn mods_section(m: &SeasonMods) -> String {
    let mut out = String::from("<h2>Moderators</h2><div class=\"statline\">");
    out.push_str(&format!(
        "Owner: <b>{}</b>",
        esc(m.owner.as_deref().unwrap_or("—"))
    ));
    out.push_str("</div>");
    for (id, name) in &m.co_mods {
        out.push_str(&format!(
            r#"<div class="hintrow"><span>{}</span>{}</div>"#,
            esc(name),
            action_btn(
                "admin/mods/remove",
                &[("account_id", &id.to_string())],
                "remove",
                true,
                ""
            )
        ));
    }
    out.push_str(
        r#"<form class="hintform" method="post" action="admin/mods/add"><input type="text" name="username" placeholder="add a co-moderator (moderator username)" required><button type="submit">Add</button></form>"#,
    );
    out
}

// ---- seasons (instance-level list + create) ----

/// `manage[i]`: may the viewer run `seasons[i]`?
pub fn seasons_page(
    seasons: &[crate::db::SeasonRow],
    manage: &[bool],
    default_name: &str,
    err: Option<&str>,
    sysadmin: bool,
) -> Html<String> {
    let mut body = console_nav("/admin/seasons", sysadmin);
    body.push_str(&format!(
        r#"<h2>New season</h2>
<form class="linkform" method="post" action="/admin/seasons">
<label>Name<input type="text" name="name" value="{}" maxlength="80" required></label>
<button type="submit">Create season</button></form>
<p class="statline">A new season starts as a <b>draft</b>: set it up in its admin page, open signups, then generate it to go live. Each live season gets its own AP port until it's archived.</p>"#,
        esc(default_name)
    ));
    if sysadmin {
        body.push_str(
            r#"<h2>Import a finished season</h2>
<form class="linkform" method="post" action="/admin/seasons/import" enctype="multipart/form-data">
<label>Season bundle<input type="file" name="bundle" accept=".tar.gz,.tgz,application/gzip" required></label>
<button type="submit">Import</button></form>
<p class="statline">A bundle is what archiving a season writes (its summary, seed, save, YAMLs and worlds) — download one from another Bifrost's Seasons list. It shows up here as an archived season.</p>"#,
        );
    }
    body.push_str("<h2>Seasons</h2>");
    if seasons.is_empty() {
        body.push_str(r#"<div class="empty">No seasons yet.</div>"#);
    } else {
        body.push_str(r#"<table class="utable"><tr><th>Season</th><th>State</th><th>Port</th><th>Created</th><th></th></tr>"#);
        for (s, can) in seasons
            .iter()
            .zip(manage.iter().chain(std::iter::repeat(&false)))
        {
            let when = match (s.archived_at, s.live_at) {
                (Some(t), _) => format!("archived {}", fmt_date(t)),
                (None, Some(t)) => format!("live since {}", fmt_date(t)),
                _ => fmt_date(s.created_at),
            };
            let links = if s.state() == crate::db::State::Archived {
                let bundle = s
                    .seed
                    .as_deref()
                    .and_then(crate::archive::bundle_path)
                    .is_some()
                    && *can;
                let bundle = if bundle {
                    format!(r#" · <a href="/s/{}/admin/bundle">bundle</a>"#, s.id)
                } else {
                    String::new()
                };
                format!(r#"<a href="/s/{0}/season">summary</a>{bundle}"#, s.id)
            } else if *can {
                format!(
                    r#"<a href="/s/{0}/board">board</a> · <a href="/s/{0}/admin">admin</a>"#,
                    s.id
                )
            } else {
                format!(r#"<a href="/s/{0}/board">board</a>"#, s.id)
            };
            body.push_str(&format!(
                r#"<tr class="{}"><td>{}</td><td><span class="tag">{}</span></td><td class="mono">{}</td><td class="mono">{}</td><td>{links}</td></tr>"#,
                if s.state() == crate::db::State::Archived { "dead" } else { "" },
                esc(&s.name),
                s.state().as_str(),
                s.ap_port.map(|p| p.to_string()).unwrap_or_default(),
                when,
            ));
        }
        body.push_str("</table>");
    }
    auth_page("Console", r#"<a href="/">← home</a>"#, err, &body)
}

// ---- instance settings (sysadmin) ----

/// Deployment facts the settings page shows read-only.
pub struct SettingsStatus {
    pub auth_mode: String,
    /// The sandbox posture line, and whether it's the strong tier.
    pub isolation: String,
    pub strong_isolation: bool,
    pub managed: bool,
    pub ports: (u16, u16),
    /// Ready Archipelago installs, each with the seasons pinned to it.
    pub installs: Vec<(crate::apinstall::Install, Vec<String>)>,
    pub active_ap: String,
    pub updates: crate::apupdate::Updates,
    pub backups: Vec<crate::backup::Backup>,
    pub backup_policy: crate::backup::Policy,
    pub backup_dir: String,
    pub backup_running: bool,
    /// The last run's (when, message, ok).
    pub backup_last: Option<(i64, String, bool)>,
    /// The live-load panel's contents ([`load_panel`]).
    pub load_html: String,
}

pub(super) fn backups_panel(status: &SettingsStatus) -> String {
    let p = &status.backup_policy;
    let when = match p.hour {
        Some(h) => format!("Nightly at {h:02}:00 UTC"),
        None => "Nightly backups are off".into(),
    };
    let mut b = format!(
        r#"<h2 id="backups">Backups</h2><p class="statline">{when}, to <span class="mono">{}</span>. Keeps the newest backup of each of the last {} days, {} weeks and {} months, and drops the oldest beyond {} MiB in total (never the newest). Restore with <span class="mono">bifrost backup restore &lt;file&gt;</span> while Bifrost is stopped. Settings: <span class="mono">[backup]</span> in config.toml.</p>"#,
        esc(&status.backup_dir),
        p.daily,
        p.weekly,
        p.monthly,
        p.max_bytes >> 20,
    );
    if status.backup_running {
        b.push_str(
            r#"<meta http-equiv="refresh" content="3"><div class="banner">Backing up…</div>"#,
        );
    } else if let Some((at, msg, ok)) = &status.backup_last {
        b.push_str(&format!(
            r#"<div class="banner {}">{} <span class="statline">({})</span></div>"#,
            if *ok { "ok" } else { "bad" },
            esc(msg),
            fmt_date(*at)
        ));
    }
    if status.backups.is_empty() {
        b.push_str(r#"<div class="empty">No backups yet.</div>"#);
    } else {
        let total: u64 = status.backups.iter().map(|x| x.size).sum();
        b.push_str(&format!(
            r#"<p class="statline">{} backups, {} in total.</p>"#,
            status.backups.len(),
            human_size(total)
        ));
        for x in &status.backups {
            b.push_str(&format!(
                r#"<div class="hintrow"><span class="mono">{} <span class="statline">· {}</span></span><span><a class="ghost" href="/admin/settings/backups/download?name={}">download</a> {}</span></div>"#,
                esc(&x.name),
                human_size(x.size),
                urlq(&x.name),
                action_btn(
                    "/admin/settings/backups/delete",
                    &[("name", &x.name)],
                    "delete",
                    true,
                    &format!("Delete {}?", x.name)
                ),
            ));
        }
    }
    b.push_str(r#"<div class="btnrow">"#);
    if !status.backup_running {
        b.push_str(&action_btn(
            "/admin/settings/backups/now",
            &[],
            "Back up now",
            false,
            "",
        ));
    }
    b.push_str("</div>");
    b
}

/// The Archipelago panel: installed versions, the install job, releases.
pub(super) fn ap_panel(status: &SettingsStatus) -> String {
    let job = status.updates.job.as_ref();
    let busy = job.is_some_and(|j| j.outcome.is_none());
    let mut b = String::from(
        r#"<h2 id="ap">Archipelago</h2><p class="statline">Signup checks and new generations use the <b>active</b> version. A season keeps running on the version that generated its seed until it's re-rolled or archived, so switching never changes a live game.</p>"#,
    );
    b.push_str(r#"<table class="utable"><tr><th>Version</th><th>Location</th><th>Used by</th><th></th></tr>"#);
    for (i, users) in &status.installs {
        let active = i.version == status.active_ap;
        let mut used = Vec::new();
        if active {
            used.push(r#"<span class="tag">active</span>"#.to_string());
        }
        used.extend(users.iter().map(|u| esc(u)));
        let mut acts = String::new();
        if !active {
            acts.push_str(&action_btn(
                "/admin/settings/ap/use",
                &[("version", &i.version)],
                "use for new seasons",
                false,
                "",
            ));
        }
        if !i.bundled && !active && users.is_empty() {
            acts.push(' ');
            acts.push_str(&action_btn(
                "/admin/settings/ap/remove",
                &[("version", &i.version)],
                "remove",
                true,
                &format!("Remove Archipelago {}?", i.version),
            ));
        }
        b.push_str(&format!(
            r#"<tr><td class="mono">{}{}</td><td class="mono">{}</td><td>{}</td><td>{acts}</td></tr>"#,
            esc(&i.version),
            if i.bundled { " · bundled" } else { "" },
            esc(&i.dir.display().to_string()),
            if used.is_empty() { "—".into() } else { used.join(" · ") },
        ));
    }
    b.push_str("</table>");

    if let Some(j) = job {
        match &j.outcome {
            None => b.push_str(&format!(
                r#"<meta http-equiv="refresh" content="4"><div class="banner">Installing Archipelago {} (started {}) — this page refreshes until it's done.</div>"#,
                esc(&j.version),
                fmt_date(j.started)
            )),
            Some(Ok(m)) => b.push_str(&format!(r#"<div class="banner ok">{}</div>"#, esc(m))),
            Some(Err(e)) => b.push_str(&format!(
                r#"<div class="banner bad">Installing Archipelago {} failed: {}</div>"#,
                esc(&j.version),
                esc(e)
            )),
        }
        if !j.log.is_empty() {
            b.push_str(&format!(
                r#"<pre class="mono" style="max-height:220px;overflow:auto;white-space:pre-wrap;font-size:12px">{}</pre>"#,
                esc(&j.log.join("\n"))
            ));
        }
    }

    b.push_str(r#"<div class="btnrow">"#);
    b.push_str(&action_btn(
        "/admin/settings/ap/check",
        &[],
        "Check for releases",
        false,
        "",
    ));
    b.push_str("</div>");
    match &status.updates.releases {
        None => {}
        Some((_, Err(e))) => b.push_str(&format!(
            r#"<div class="banner bad">Couldn't check: {}</div>"#,
            esc(e)
        )),
        Some((at, Ok(rels))) => {
            let installed = |t: &str| status.installs.iter().any(|(i, _)| i.version == t);
            let fresh: Vec<_> = rels.iter().filter(|r| !installed(&r.tag)).collect();
            b.push_str(&format!(
                r#"<p class="statline">Checked {}.</p>"#,
                fmt_date(*at)
            ));
            if fresh.is_empty() {
                b.push_str(r#"<div class="empty">Every recent release is installed.</div>"#);
            }
            for r in fresh {
                let pre = if r.prerelease {
                    r#" <span class="tag">pre-release</span>"#
                } else {
                    ""
                };
                let date = r.published.get(..10).unwrap_or("");
                let btn = if busy {
                    String::new()
                } else {
                    action_btn(
                        "/admin/settings/ap/install",
                        &[("version", &r.tag)],
                        "install",
                        false,
                        "",
                    )
                };
                b.push_str(&format!(
                    r#"<div class="hintrow"><span><b class="mono">{}</b>{pre} <span class="statline">· {} · {}</span></span>{btn}</div>"#,
                    esc(&r.tag),
                    esc(&r.name),
                    esc(date),
                ));
            }
        }
    }
    b
}

pub fn settings_page(
    inst: &crate::db::Instance,
    status: &SettingsStatus,
    err: Option<&str>,
    saved: bool,
) -> Html<String> {
    let mut body = console_nav("/admin/settings", true);
    if saved {
        body.push_str(r#"<div class="banner ok">Saved — the home page shows it now.</div>"#);
    }
    let ports = if status.managed {
        let (a, b) = status.ports;
        format!(
            "AP servers: managed by Bifrost, one port per live season from <b>{a}–{b}</b> \
             (up to {} live at once; set with BIFROST_AP_PORTS)",
            b - a + 1
        )
    } else {
        "AP server: external (BIFROST_AP_MODE=external)".into()
    };
    body.push_str(&format!(
        r#"<h2>Deployment</h2><p class="statline">Bifrost <b>{}</b> · {}<br>{ports}<br>Auth: <b>{}</b><br>Isolation: <span style="color:var({})">{}</span></p>"#,
        esc(crate::release_check::current()),
        release_status(
            crate::release_check::enabled(),
            crate::release_check::last().as_ref()
        ),
        esc(&status.auth_mode),
        if status.strong_isolation { "--ok" } else { "--warn" },
        esc(&status.isolation),
    ));
    body.push_str(&format!(
        r#"<h2 id="load">Load</h2><div id="loadpanel">{}</div><script>setInterval(function(){{fetch('/admin/settings/load').then(function(r){{return r.ok?r.text():null}}).then(function(t){{if(t!==null)document.getElementById('loadpanel').innerHTML=t}})}},10000)</script>"#,
        status.load_html
    ));
    body.push_str(&ap_panel(status));
    body.push_str(&backups_panel(status));
    let discords: String = inst
        .discords
        .iter()
        .map(|d| {
            if d.url.is_empty() {
                d.name.clone()
            } else if d.note.is_empty() {
                format!("{} | {}", d.name, d.url)
            } else {
                format!("{} | {} | {}", d.name, d.url, d.note)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    body.push_str(&format!(
        r#"<h2>Home page</h2>
<style>.authform{{max-width:560px}}.authform textarea{{background:var(--bg);color:var(--fg);border:1px solid var(--bd);border-radius:6px;padding:7px 9px;font:13px/1.5 var(--mono);min-height:110px}}</style>
<form class="authform" method="post" action="/admin/settings">
<label>Connect host — what players' AP clients connect to; each live season adds its port<input type="text" name="domain" value="{}" placeholder="ap.example.org" maxlength="253"></label>
<label>Password note — shown under the connect address<input type="text" name="password_note" value="{}" maxlength="200"></label>
<label>Home link (optional) — a "home" pill in the header<input type="url" name="home_url" value="{}" placeholder="https://example.org/"></label>
<label>Discord servers — one per line: <span class="mono">Name | https://discord.gg/… | note</span><textarea name="discords" spellcheck="false">{}</textarea></label>
<button type="submit">Save</button></form>"#,
        esc(&inst.domain),
        esc(&inst.password_note),
        esc(&inst.home_url),
        esc(&discords),
    ));
    auth_page("Console", r#"<a href="/">← home</a>"#, err, &body)
}

/// The Deployment line's word on newer releases.
pub(super) fn release_status(
    enabled: bool,
    last: Option<&crate::release_check::Checked>,
) -> String {
    match last {
        _ if !enabled => "update checks off (BIFROST_UPDATE_CHECK)".into(),
        None => "not checked for updates yet".into(),
        Some((_, Ok(None))) => "the newest release".into(),
        Some((_, Ok(Some(n)))) => format!(
            r#"<a href="{}">{}</a> is out"#,
            esc(&n.url),
            esc(&n.version)
        ),
        Some((_, Err(e))) => format!(
            r#"<span style="color:var(--warn)">couldn't check for updates: {}</span>"#,
            esc(e)
        ),
    }
}

// ---- the apworld catalog (instance, sysadmin) ----

pub fn catalog_page(
    catalog: &[crate::db::CatalogWorld],
    approval: crate::db::Approval,
    err: Option<&str>,
    saved: Option<&str>,
) -> Html<String> {
    use crate::db::Approval;
    let mut body = console_nav("/admin/catalog", true);
    if let Some(m) = saved {
        body.push_str(&format!(r#"<div class="banner ok">{}</div>"#, esc(m)));
    }
    let checked = |a: Approval| if a == approval { " checked" } else { "" };
    body.push_str(&format!(
        r#"<h2>Uploads</h2>
<form class="authform" method="post" action="/admin/catalog/approval">
<label class="chk"><input type="radio" name="mode" value="manual"{}> <span><b>Manual approval</b> — an uploaded apworld that isn't in the catalog waits until a moderator of that season (or a sysadmin) approves it. Signups that need it are held, then checked on approval.</span></label>
<label class="chk"><input type="radio" name="mode" value="auto"{}> <span><b>Automatic</b> — uploads install right away (still pinned by hash, and still run only in the sandbox).</span></label>
<button type="submit">Save</button></form>"#,
        checked(Approval::Manual),
        checked(Approval::Auto),
    ));
    body.push_str(r#"<h2>Catalog</h2><p class="statline">Worlds you've vetted. Players can pick them at signup, and an upload with the same contents is accepted without approval. Removing one doesn't touch seasons that already installed it.</p>"#);
    if catalog.is_empty() {
        body.push_str(r#"<div class="empty">The catalog is empty.</div>"#);
    } else {
        body.push_str(r#"<table class="utable"><tr><th>Game</th><th>Version</th><th>File</th><th>SHA-256</th><th>Added</th><th></th></tr>"#);
        for w in catalog {
            let note = if w.note.is_empty() {
                String::new()
            } else {
                format!(r#"<div class="statline">{}</div>"#, esc(&w.note))
            };
            body.push_str(&format!(
                r#"<tr><td>{}{note}</td><td class="mono">{}</td><td class="mono">{}</td><td class="mono" title="{}">{}…</td><td>{} · {}</td><td>{}</td></tr>"#,
                esc(if w.game.is_empty() { "—" } else { &w.game }),
                esc(&w.version),
                esc(&w.file),
                esc(&w.sha256),
                esc(&w.sha256[..12.min(w.sha256.len())]),
                esc(&w.added_by),
                fmt_date(w.added_at),
                action_btn(
                    "/admin/catalog/remove",
                    &[("sha256", &w.sha256)],
                    "remove",
                    true,
                    &format!("Remove {} from the catalog?", w.file)
                ),
            ));
        }
        body.push_str("</table>");
    }
    body.push_str(
        r#"<h2>Add a world</h2>
<form class="authform" method="post" action="/admin/catalog/add" enctype="multipart/form-data">
<label>.apworld file<input type="file" name="apworld" accept=".apworld" required></label>
<label>Game (optional — read from the world's manifest when blank)<input type="text" name="game" maxlength="80"></label>
<label>Version (optional — likewise)<input type="text" name="version" maxlength="80"></label>
<label>Note (optional)<input type="text" name="note" maxlength="200" placeholder="e.g. source URL, who vetted it"></label>
<button type="submit">Add to catalog</button></form>
<p class="statline">You can also add a world straight from a season: its admin page has an “add to catalog” button next to each installed upload.</p>"#,
    );
    auth_page("Console", r#"<a href="/">← home</a>"#, err, &body)
}
