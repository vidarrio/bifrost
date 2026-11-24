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
<p class="statline">A bundle is what archiving a season writes (its summary, seed, save, YAMLs and worlds). Download one from another Bifrost's Seasons list. It shows up here as an archived season.</p>"#,
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
    /// Whether there's a common-game export token.
    pub export_token: bool,
}

/// The common-game export token: make, replace or revoke it.
pub(super) fn export_panel(status: &SettingsStatus) -> String {
    let mut b = String::from(
        r#"<h2 id="export">Common-game export</h2><p class="statline">For your own sync to the common games' servers: a read-only token that fetches every live season's common games, their allowlists and their seed files from <code>/export/commons</code>. It reads nothing else.</p>"#,
    );
    if status.export_token {
        b.push_str(r#"<p class="statline">A token is set. Replacing it stops the old one working.</p><div class="btnrow">"#);
        b.push_str(&action_btn(
            "/admin/settings/export",
            &[("do", "new")],
            "Replace token",
            false,
            "Replace the export token? Anything using the old one stops working.",
        ));
        b.push_str(&action_btn(
            "/admin/settings/export",
            &[("do", "revoke")],
            "Revoke",
            true,
            "Revoke the export token?",
        ));
    } else {
        b.push_str(
            r#"<p class="statline">No token is set, so the export is off.</p><div class="btnrow">"#,
        );
        b.push_str(&action_btn(
            "/admin/settings/export",
            &[("do", "new")],
            "Make a token",
            false,
            "",
        ));
    }
    b.push_str("</div>");
    b
}

/// The new export token, shown once, with how to use it.
pub fn export_token_page(token: &str) -> Html<String> {
    let mut b = page_head(None);
    b.push_str(&format!(
        r#"<h1>Export token</h1><div class="crumb"><a href="/admin/settings#export">← settings</a></div>
<div class="banner ok">Copy it now: it isn't shown again, and only its hash is kept.</div>
<style>.token,pre.howto{{overflow-wrap:anywhere;white-space:pre-wrap}}</style>
<p><code class="token">{t}</code></p>
<p class="statline">Send it as a bearer token. The index lists every live season's common games, each with its allowlist's and files' URLs:</p>
<pre class="howto">printf 'Authorization: Bearer %s\n' {t} &gt; bifrost-export.hdr    # chmod 600
curl -H @bifrost-export.hdr https://&lt;bifrost&gt;/export/commons</pre>
<p class="statline">Keep the token in a file like that rather than on a command line, where other users of the host can see it.</p>
<p class="statline">An allowlist is one name per line, or JSON with <code>?format=json</code>. A season's seed name changes when it's re-rolled, which is when its servers need their new files.</p>"#,
        t = esc(token)
    ));
    b.push_str("</main></body></html>");
    Html(b)
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
                r#"<meta http-equiv="refresh" content="4"><div class="banner">Installing Archipelago {} (started {}). This page refreshes until it's done.</div>"#,
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
        body.push_str(r#"<div class="banner ok">Saved.</div>"#);
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
    body.push_str(&export_panel(status));
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
        r#"<h2>Connections and home page</h2>
<style>.authform{{max-width:560px}}.authform textarea{{background:var(--bg);color:var(--fg);border:1px solid var(--bd);border-radius:6px;padding:7px 9px;font:13px/1.5 var(--mono);min-height:110px}}</style>
<form class="authform" method="post" action="/admin/settings">
<label><span>Site address (optional): this Bifrost's own address, shown next to the title on the home page; empty for the address in <span class="mono">server.public_url</span> (without its scheme), if that's set</span><input type="text" name="site_address" value="{}" placeholder="bifrost.example.org" maxlength="253"></label>
<label>Connect host: what players' AP clients connect to; each live season adds its port. Shown on season boards, not the home page<input type="text" name="domain" value="{}" placeholder="ap.example.org" maxlength="253"></label>
<label>Home link (optional): a pill in the header that goes there<input type="url" name="home_url" value="{}" placeholder="https://example.org/"></label>
<label>Home pill label: what that pill says (shown in capitals); empty for “{}”<input type="text" name="home_label" value="{}" placeholder="{}" maxlength="{}"></label>
<label><span>Discord servers, one per line: <span class="mono">Name | https://discord.gg/… | note</span></span><textarea name="discords" spellcheck="false">{}</textarea></label>
<button type="submit">Save</button></form>"#,
        esc(&inst.site_address),
        esc(&inst.domain),
        esc(&inst.home_url),
        crate::db::DEFAULT_HOME_LABEL,
        esc(&inst.home_label),
        crate::db::DEFAULT_HOME_LABEL,
        crate::db::HOME_LABEL_MAX,
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
    offers: &[crate::worlds::Offer],
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
<label class="chk"><input type="radio" name="mode" value="manual"{}> <span><b>Manual approval</b>: an uploaded apworld that isn't in the catalog waits until a moderator of that season (or a sysadmin) approves it. Signups that need it are held, then checked on approval.</span></label>
<label class="chk"><input type="radio" name="mode" value="auto"{}> <span><b>Automatic</b>: uploads of new games install right away (still pinned by hash, and still run only in the sandbox). A newer version of a game a season has still waits for its moderators.</span></label>
<button type="submit">Save</button></form>"#,
        checked(Approval::Manual),
        checked(Approval::Auto),
    ));
    body.push_str(r#"<style>.catnotes{clear:both;text-align:left}.catnotes summary{cursor:pointer;color:var(--muted);font-size:12px;margin-top:2px}.catnotes .authform{margin:8px 0 4px}</style>"#);
    body.push_str(r#"<h2>Catalog</h2><p class="statline">Worlds you've vetted. Players can pick them at signup. The catalog holds one version of each game: a player's upload of a game that's here gets the catalog's version instead (a newer one is listed below for you to consider), and adding a game that's here replaces its entry. Removing one doesn't touch seasons that already installed it.</p>"#);
    if catalog.is_empty() {
        body.push_str(r#"<div class="empty">The catalog is empty.</div>"#);
    } else {
        body.push_str(r#"<table class="utable"><tr><th>Game</th><th>Version</th><th>File</th><th>SHA-256</th><th>Added</th><th></th></tr>"#);
        for w in catalog {
            // Both notes, editable in place: the sysadmins' own and the one
            // players see on the Games page.
            let notes = format!(
                r#"<details class="catnotes"><summary>{}</summary><form class="authform" method="post" action="/admin/catalog/notes"><input type="hidden" name="sha256" value="{}"><label>Public note (shown to players on the Games page)<input type="text" name="public_note" value="{}" maxlength="{max}" placeholder="e.g. needs the Steam version; beta"></label><label>Admin note (only sysadmins see it)<input type="text" name="note" value="{}" maxlength="{max}" placeholder="e.g. source URL, who vetted it"></label><button type="submit">Save notes</button></form></details>"#,
                match (w.public_note.is_empty(), w.note.is_empty()) {
                    (true, true) => "add notes".to_string(),
                    _ => [("public", &w.public_note), ("admin", &w.note)]
                        .iter()
                        .filter(|(_, n)| !n.is_empty())
                        .map(|(k, n)| format!("{k}: {}", esc(n)))
                        .collect::<Vec<_>>()
                        .join(" · "),
                },
                esc(&w.sha256),
                esc(&w.public_note),
                esc(&w.note),
                max = crate::worlds::NOTE_MAX,
            );
            body.push_str(&format!(
                r#"<tr><td>{}{notes}</td><td class="mono">{}</td><td class="mono">{}</td><td class="mono" title="{}">{}…</td><td>{} · {}</td><td>{}</td></tr>"#,
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
    if !offers.is_empty() {
        body.push_str(r#"<h2>Worlds offered for the catalog</h2><p class="statline">Newer versions of catalog games that players uploaded (their signups use the catalog's version), and worlds a season's moderators approved that the catalog doesn't have. Add one here once you've vetted it, and new signups get it. Seasons that installed an older version keep it.</p>"#);
        body.push_str(r#"<p class="statline">The game and version are what the upload's own manifest claims. Download it and vet its code before adding it: the catalog's worlds are trusted on every season.</p>"#);
        body.push_str(r#"<table class="utable"><tr><th>Game</th><th>Version</th><th>Replaces</th><th>File</th><th>Uploaded</th><th></th></tr>"#);
        for o in offers {
            let approved = if o.approved_by.is_empty() {
                String::new()
            } else {
                format!(" · approved by {}", esc(&o.approved_by))
            };
            let (label, confirm) = if o.replaces.is_empty() {
                (
                    "add to the catalog",
                    format!(
                        "Add {} {} ({}'s upload) to the catalog? Its code becomes trusted catalog code for every season: vet it first.",
                        o.game, o.version, o.uploaded_by
                    ),
                )
            } else {
                (
                    "replace the catalog's version",
                    format!(
                        "Replace {} {} in the catalog with {}'s upload ({})? Its code becomes trusted catalog code for every season: vet it first.",
                        o.game, o.replaces, o.uploaded_by, o.version
                    ),
                )
            };
            body.push_str(&format!(
                r#"<tr><td>{}</td><td class="mono">{}</td><td class="mono">{}</td><td class="mono" title="{}"><a href="/admin/catalog/offer/download?sha256={}">{}</a> · {}…</td><td>{} · {}{approved} · <a href="/s/{}/admin">season</a></td><td>{} {}</td></tr>"#,
                esc(&o.game),
                esc(&o.version),
                if o.replaces.is_empty() { "new".to_string() } else { esc(&o.replaces) },
                esc(&o.sha256),
                esc(&o.sha256),
                esc(&o.file),
                esc(&o.sha256[..12.min(o.sha256.len())]),
                esc(&o.uploaded_by),
                fmt_date(o.uploaded_at),
                o.season,
                action_btn("/admin/catalog/offer/promote", &[("sha256", &o.sha256)], label, false, &confirm),
                action_btn("/admin/catalog/offer/dismiss", &[("sha256", &o.sha256)], "dismiss", true, ""),
            ));
        }
        body.push_str("</table>");
    }
    body.push_str(
        r#"<h2>Add a world</h2>
<form class="authform" method="post" action="/admin/catalog/add" enctype="multipart/form-data">
<label>.apworld file<input type="file" name="apworld" accept=".apworld" required></label>
<label>Game (optional; read from the world's manifest when blank)<input type="text" name="game" maxlength="80"></label>
<label>Version (optional; likewise)<input type="text" name="version" maxlength="80"></label>
<label>Public note (optional, shown to players on the Games page)<input type="text" name="public_note" maxlength="200" placeholder="e.g. needs the Steam version; beta"></label>
<label>Admin note (optional, only sysadmins see it)<input type="text" name="note" maxlength="200" placeholder="e.g. source URL, who vetted it"></label>
<button type="submit">Add to catalog</button></form>
<p class="statline">You can also add a world straight from a season: its admin page has an “add to catalog” button next to each installed upload. Adding a game that's already here replaces its entry.</p>"#,
    );
    auth_page("Console", r#"<a href="/">← home</a>"#, err, &body)
}

/// One setup on the console's Common games page.
#[derive(Debug, Default)]
pub struct CommonCatalogRow {
    pub key: String,
    pub name: String,
    pub slot: String,
    /// Why it can't be used, if it can't.
    pub problem: Option<String>,
    /// Seasons not yet archived that include it.
    pub seasons: Vec<String>,
}

/// The catalog of common games: each setup, and a form to make one.
pub fn common_catalog_page(
    rows: &[CommonCatalogRow],
    err: Option<&str>,
    saved: Option<&str>,
) -> Html<String> {
    let mut body = console_nav("/admin/common-games", true);
    if let Some(m) = saved {
        body.push_str(&format!(r#"<div class="banner ok">{}</div>"#, esc(m)));
    }
    body.push_str(r#"<h2>Common games</h2><p class="statline">Co-op games you host, such as a Minecraft or Factorio server, that a season's moderators can add as a slot everyone plays. Each setup is a folder under <code>common/</code> in the data directory; editing it here or by hand comes to the same thing. Bifrost never reaches the game's server: set it up from the season's files, and keep its allowlist in step from the season console or the export.</p>"#);
    if rows.is_empty() {
        body.push_str(r#"<div class="empty">No setups yet.</div>"#);
    }
    for r in rows {
        let used = if r.seasons.is_empty() {
            String::new()
        } else {
            format!(" · in {}", esc(&r.seasons.join(", ")))
        };
        let head = if r.name.is_empty() {
            format!("<b>{}</b>", esc(&r.key))
        } else {
            format!(
                r#"<b>{}</b> <span class="by free">{}</span> · plays as <b>{}</b>"#,
                esc(&r.name),
                esc(&r.key),
                esc(&r.slot)
            )
        };
        body.push_str(&format!(
            r#"<div class="hintrow"><span>{head}{used}</span><span class="act"><a href="/admin/common-games/{k}">edit</a></span></div>"#,
            k = esc(&r.key)
        ));
        if let Some(p) = &r.problem {
            body.push_str(&format!(
                r#"<p class="warnnote">Can't be used: {}.</p>"#,
                esc(p)
            ));
        }
    }
    body.push_str(r#"<h2>New setup</h2>
<form class="signup" style="max-width:none" method="post" action="/admin/common-games/new" autocomplete="off">
<label><span>Key, the folder's name (lower-case letters, digits, <code>.</code>, <code>_</code> or <code>-</code>)</span><input type="text" name="key" placeholder="vanilla.26.2" maxlength="40" required></label>
<label>Name, as players see it<input type="text" name="name" placeholder="Minecraft 26.2" maxlength="60" required></label>
<label>Slot, the player name its YAML declares (at most 16 characters)<input type="text" name="slot" placeholder="Commons" maxlength="16" required></label>
<label>Player YAML<textarea name="yaml" rows="6" style="width:100%;font-family:var(--mono)" required>name: Commons
game: Minecraft
Minecraft: {}
</textarea></label>
<div class="btnrow"><button type="submit">Make it</button></div></form>
<p class="statline">Then add its apworld, if Archipelago doesn't ship the game, and its guide and downloads on its page.</p>"#);
    auth_page("Console", r#"<a href="/">← home</a>"#, err, &body)
}

/// A setup's page in the console.
#[derive(Debug, Default)]
pub struct CommonSetupView {
    pub key: String,
    pub fields: crate::common_edit::Fields,
    pub apworld: Option<String>,
    /// The apworld came in through the console, so it counts as custom.
    pub uploaded: bool,
    /// This host can run custom worlds, so the console can take an apworld.
    pub can_upload_world: bool,
    /// Its `game.toml` doesn't parse.
    pub unreadable: bool,
    /// Each download's (file, label).
    pub downloads: Vec<(String, String)>,
    pub problem: Option<String>,
    pub seasons: Vec<String>,
}

pub fn common_setup_page(
    v: &CommonSetupView,
    err: Option<&str>,
    saved: Option<&str>,
) -> Html<String> {
    let k = esc(&v.key);
    let mut body = console_nav("/admin/common-games", true);
    // Long file names and the add-download form wrap on a phone.
    body.push_str("<style>.hintrow{flex-wrap:wrap;gap:6px}.hintrow code{overflow-wrap:anywhere}form.console{flex-wrap:wrap}</style>");
    body.push_str(&format!(
        r#"<div class="crumb"><a href="/admin/common-games">← common games</a></div><h2 style="font-size:20px;letter-spacing:0;text-transform:none;color:var(--fg);margin-top:14px">{}</h2>"#,
        if v.fields.name.is_empty() {
            k.clone()
        } else {
            esc(&v.fields.name)
        }
    ));
    if let Some(m) = saved {
        body.push_str(&format!(r#"<div class="banner ok">{}</div>"#, esc(m)));
    }
    if let Some(p) = &v.problem {
        body.push_str(&format!(
            r#"<div class="banner warn">It can't be used as it is: {}. Saving the form below rewrites <code>game.toml</code>{}.</div>"#,
            esc(p),
            if v.unreadable {
                ": as it can't be read, its apworld and downloads have to be added again after"
            } else {
                ""
            }
        ));
    }
    if !v.seasons.is_empty() {
        body.push_str(&format!(
            r#"<p class="statline">In use by {}. Changes show on their game pages straight away, and reach their seeds when they're next generated. A season keeps the slot name it was added with: if you change it, the season's moderators remove the game and add it again before generating.</p>"#,
            esc(&v.seasons.join(", "))
        ));
    }
    let f = &v.fields;
    let (kind, label, pattern, hint) = &f.ident;
    body.push_str(&format!(
        r#"<form class="signup" style="max-width:none" method="post" action="/admin/common-games/{k}" autocomplete="off">
<label>Name, as players see it<input type="text" name="name" value="{name}" maxlength="60" required></label>
<label>Slot, the player name its YAML declares<input type="text" name="slot" value="{slot}" maxlength="16" required></label>
<label>Address players join at; a season can set its own<input type="text" name="address" value="{address}" maxlength="200" placeholder="mc.example:25565"></label>
<h2>Allowlist name</h2>
<p class="statline">If the server only lets in the season's players, players register a name for it. Setups of one kind share them, so one Minecraft username serves every Minecraft setup. Leave the kind empty if the server lets anyone in.</p>
<label><span>Kind (lower-case letters, digits, <code>_</code> or <code>-</code>)</span><input type="text" name="kind" value="{kind}" maxlength="32" placeholder="minecraft"></label>
<label>Label<input type="text" name="label" value="{label}" maxlength="60" placeholder="Minecraft username"></label>
<label>Pattern, a regular expression the whole name has to match<input type="text" name="pattern" value="{pattern}" maxlength="200" placeholder="[A-Za-z0-9_]{{3,16}}"></label>
<label>Hint, the rule in words<input type="text" name="hint" value="{hint}" maxlength="120" placeholder="3 to 16 letters, digits or underscores"></label>
<h2>Join guide</h2>
<p class="statline">Markdown, shown on the game page. <code>{{address}}</code> and <code>{{slot}}</code> are filled in. HTML shows as text.</p>
<textarea name="guide" rows="14" style="width:100%;font-family:var(--mono)">{guide}</textarea>
<h2>Player YAML</h2>
<textarea name="yaml" rows="8" style="width:100%;font-family:var(--mono)" required>{yaml}</textarea>
<div class="btnrow"><button type="submit">Save</button></div></form>"#,
        name = esc(&f.name),
        slot = esc(&f.slot),
        address = esc(&f.address),
        kind = esc(kind),
        label = esc(label),
        pattern = esc(pattern),
        hint = esc(hint),
        guide = esc(&f.guide),
        yaml = esc(&f.yaml),
    ));

    body.push_str(r#"<h2>Apworld</h2>"#);
    match &v.apworld {
        Some(w) => body.push_str(&format!(
            r#"<div class="hintrow"><span><code>{}</code> · {}</span><span>{}</span></div>"#,
            esc(w),
            if v.uploaded {
                "uploaded here, so it runs as a custom world: in the sandbox"
            } else {
                "put in the folder on the host, so it's trusted like Bifrost's own"
            },
            action_btn(
                &format!("/admin/common-games/{}/remove", v.key),
                &[("what", "apworld")],
                "remove",
                true,
                "Remove the apworld?"
            )
        )),
        None => {
            body.push_str(r#"<p class="statline">None: the game is one Archipelago ships.</p>"#)
        }
    }
    if v.can_upload_world {
        body.push_str(&format!(
            r#"<form class="console" method="post" action="/admin/common-games/{k}/apworld" enctype="multipart/form-data"><input type="file" name="file" accept=".apworld" required style="flex:1;min-width:0"><button type="submit">{}</button></form><p class="statline">An apworld uploaded here is a custom world, run in the sandbox like a player's upload.</p>"#,
            if v.apworld.is_some() { "Replace apworld" } else { "Add apworld" }
        ));
    } else {
        body.push_str(r#"<p class="statline">This host has no sandbox for custom worlds, so an apworld can't be uploaded here. Put it in the setup's folder on the host instead, and name it in <code>game.toml</code>.</p>"#);
    }

    body.push_str(r#"<h2>Downloads</h2><p class="statline">Client files the game page offers, such as a mod or its loader.</p>"#);
    for (file, label) in &v.downloads {
        body.push_str(&format!(
            r#"<div class="hintrow"><span>{} · <code>{}</code></span><span>{}</span></div>"#,
            esc(label),
            esc(file),
            action_btn(
                &format!("/admin/common-games/{}/remove", v.key),
                &[("what", "download"), ("file", file)],
                "remove",
                true,
                &format!("Remove {file}?")
            )
        ));
    }
    body.push_str(&format!(
        r#"<form class="console" method="post" action="/admin/common-games/{k}/download" enctype="multipart/form-data"><input type="text" name="label" placeholder="label, e.g. NeoForge installer" maxlength="80" required style="flex:1 1 14em;min-width:0"><input type="file" name="file" required style="flex:1 1 14em;min-width:0"><button type="submit">Add download</button></form>"#
    ));

    body.push_str(r#"<h2>Delete</h2>"#);
    if v.seasons.is_empty() {
        body.push_str(&action_btn(
            &format!("/admin/common-games/{}/delete", v.key),
            &[],
            "Delete this setup",
            true,
            "Delete this setup and its folder? This can't be undone.",
        ));
    } else {
        body.push_str(r#"<p class="statline">It can't be deleted while a season that isn't archived includes it.</p>"#);
    }
    auth_page("Console", r#"<a href="/">← home</a>"#, err, &body)
}

/// The moderator log (sysadmins): what moderators and sysadmins did, newest
/// first, filterable by who, season and action.
#[allow(clippy::type_complexity)]
pub fn audit_page(
    rows: &[crate::db::ModAction],
    choices: &(Vec<String>, Vec<(String, String, bool)>, Vec<String>),
    f: &crate::db::ModLogFilter,
    total: i64,
) -> Html<String> {
    let (users, seasons, actions) = choices;
    let mut body = console_nav("/admin/audit", true);
    body.push_str(r#"<h2>Moderator log</h2><details class="about"><summary>What's in it</summary><p class="statline">Every change a moderator or sysadmin made in season or instance administration, console commands included, and what they did with a moderator's powers on players' pages: who, where, what (the form's fields, passwords and tokens left out) and the answer: ok, refused, or unfinished (the request never completed). A few refusals on players' pages redirect instead of showing an error, so they show as ok. Kept for a year. Each page holds as many entries as fit your window.</p></details>"#);
    body.push_str(AUDIT_CSS);
    let opt = |value: &str, label: &str, on: bool| {
        format!(
            r#"<option value="{}"{}>{}</option>"#,
            esc(value),
            if on { " selected" } else { "" },
            esc(label)
        )
    };
    let mut who = opt("", "anyone", f.username.is_empty());
    for u in users {
        who.push_str(&opt(u, u, u.eq_ignore_ascii_case(&f.username)));
    }
    let mut wher = opt("", "any season", f.season.is_empty());
    for (key, name, gone) in seasons {
        let name = if name.is_empty() {
            key.as_str()
        } else {
            name.as_str()
        };
        let label = if *gone {
            format!("{name} (gone)")
        } else {
            name.to_string()
        };
        wher.push_str(&opt(key, &label, *key == f.season));
    }
    let mut what = opt("", "any action", f.action.is_empty());
    for a in actions {
        what.push_str(&opt(a, crate::modlog::label(a), *a == f.action));
    }
    // A filter keeps the page size the screen asked for.
    let keep_per = if f.per == 0 {
        String::new()
    } else {
        format!(
            r#"<input type="hidden" name="rpp" value="{}"><input type="hidden" name="fit" value="{}">"#,
            f.per(),
            esc(&f.fit)
        )
    };
    body.push_str(&format!(
        r#"<form class="logfilter" method="get" action="/admin/audit"><label>Who <select name="user">{who}</select></label> <label>Season <select name="season">{wher}</select></label> <label>Action <select name="action">{what}</select></label> {keep_per}<button type="submit">Filter</button> <a class="pill" href="/admin/audit">clear</a></form>"#
    ));
    if rows.is_empty() {
        body.push_str(r#"<div class="empty">Nothing logged that matches.</div>"#);
    } else {
        // The page and its size as the server used them, for the fit script.
        let pages = ((total + f.per() - 1) / f.per()).max(1);
        body.push_str(&format!(
            r#"<table class="utable" id="modlog" data-page="{}" data-per="{}">"#,
            f.page.clamp(1, pages),
            f.per()
        ));
        body.push_str(r#"<tr><th>When (UTC)</th><th>Who</th><th>Season</th><th>Action</th><th>Details</th><th>Answer</th></tr>"#);
        for r in rows {
            let when = format!(
                "{} {:02}:{:02}",
                fmt_date(r.at),
                r.at.rem_euclid(86_400) / 3600,
                r.at.rem_euclid(3600) / 60
            );
            let season = match (&r.season_id, r.season_name.as_str()) {
                (Some(id), name) => format!(
                    r#"<a href="/s/{id}/admin">{}</a>"#,
                    esc(if name.is_empty() { "?" } else { name })
                ),
                (None, "") => "—".to_string(),
                (None, name) => format!(
                    "<span title=\"{0} (gone)\">{0} <span class=\"statline\">(gone)</span></span>",
                    esc(name)
                ),
            };
            let answer = match r.outcome.as_str() {
                "ok" => "ok".to_string(),
                "unfinished" => {
                    r#"<span title="The request never completed">unfinished</span>"#.to_string()
                }
                _ => format!("refused ({})", r.status),
            };
            body.push_str(&format!(
                r#"<tr tabindex="0" aria-expanded="false"><td class="mono">{when}</td><td>{}</td><td>{season}</td><td title="{}">{}</td><td class="mono det" title="{}">{}</td><td>{answer}</td></tr>"#,
                esc(&r.username),
                esc(&r.action),
                esc(crate::modlog::label(&r.action)),
                esc(&r.detail),
                esc(&r.detail),
            ));
        }
        body.push_str("</table>");
        // The activity log's pager, under the rows.
        body.push_str(&super::log::activity_pager(
            "/admin/audit",
            &audit_query(f),
            f.page.clamp(1, pages),
            pages,
            f.per(),
        ));
    }
    body.push_str(AUDIT_FIT_JS);
    auth_page("Console", r#"<a href="/">← home</a>"#, None, &body)
}

/// The log's own layout. Every cell is one line, cut short with "…" (the
/// whole of it in its tooltip), so every row is as tall as the next and a page
/// can be fitted to the window exactly. On a phone a row is compact: one line
/// of when, who, where, what and the answer, the details under it.
const AUDIT_CSS: &str = r#"<style>
/* A long season name doesn't widen its filter past the screen. */
.logfilter select{max-width:12em}.logfilter label{max-width:100%}
/* The shared pager is as wide as the activity log's feed; under this wider
   table it spans the table, so it's centred under it. */
#modlog+.pager{max-width:none}
details.about{position:relative}
details.about summary{cursor:pointer;color:var(--muted);font-size:13px;margin-bottom:6px}
details.about[open] .statline{position:absolute;z-index:5;left:0;right:0;top:100%;margin:0;padding:10px 12px;background:var(--panel);border:1px solid var(--bd);border-radius:8px;box-shadow:0 8px 20px rgba(0,0,0,.3)}
#modlog{table-layout:fixed}
#modlog th:nth-child(1){width:19ch}#modlog th:nth-child(2){width:11%}#modlog th:nth-child(3){width:14%}
#modlog th:nth-child(4){width:18%}#modlog th:nth-child(6){width:11ch}
#modlog td{white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
#modlog tr[tabindex]{cursor:pointer}#modlog tr[tabindex]:focus-visible{outline:2px solid var(--accent);outline-offset:-2px}
/* A row pressed (or entered from the keyboard) shows all of itself. */
#modlog tr.open td{white-space:normal;overflow:visible;overflow-wrap:anywhere}
@media (max-width:560px){
/* Two lines: when, who and season; then action, answer and details. */
#modlog tr{display:grid;grid-template-columns:auto auto minmax(0,1fr);gap:2px 8px;padding:6px 10px;margin:0 0 6px;font-size:12px}
#modlog td:nth-child(4){grid-row:2;grid-column:1}#modlog td:nth-child(6){grid-row:2;grid-column:2}
#modlog tr:has(th){display:none}
#modlog{table-layout:auto}
#modlog td{display:block;padding:0;border:0;text-align:left;max-width:100%;min-width:0}
#modlog td+td{border-top:0}
#modlog td::before,#modlog td::after{content:none}
#modlog td.det{grid-row:2;grid-column:3;width:auto;max-width:100%;color:var(--muted)}
#modlog tr.open{grid-template-columns:minmax(0,1fr)}#modlog tr.open td{grid-row:auto;grid-column:1}
}
</style>"#;

/// Fits a page of the moderator log to the window. From the rows shown (all
/// one height: see `AUDIT_CSS`), the room from the first row down, and the
/// space the page keeps under its last row, how many fit; then load that many,
/// keeping the first row shown on its page. The window height it fitted for
/// goes into the URL (`fit`), and a page fitted for this height is left as it
/// is: it can't reload more than once, storage or not, and going Back to a
/// page fitted for another height fits it again.
const AUDIT_FIT_JS: &str = r#"<script>
(function(){
 var t=document.getElementById('modlog');if(!t)return;
 var per=+t.dataset.per,page=+t.dataset.page;
 function size(){return innerWidth+'x'+innerHeight}
 function fitted(){return new URL(location.href).searchParams.get('fit')===size()}
 function count(){
  var rows=[].slice.call(t.rows).filter(function(r){return r.cells.length&&r.cells[0].tagName==='TD'});
  if(!rows.length)return 0;
  var top0=rows[0].getBoundingClientRect().top;
  var each=0;rows.forEach(function(r){each=Math.max(each,r.getBoundingClientRect().height)});
  if(rows.length>1)each=Math.max(each,(rows[rows.length-1].getBoundingClientRect().top-top0)/(rows.length-1));
  if(!(each>0))return 0;
  var last=rows[rows.length-1].getBoundingClientRect().bottom;
  var below=Math.max(0,document.body.getBoundingClientRect().bottom-last);
  return Math.max(1,Math.min(200,Math.floor((innerHeight-(top0+scrollY)-below)/each)));
 }
 function go(n){
  var u=new URL(location.href);
  u.searchParams.set('rpp',n);u.searchParams.set('fit',size());
  u.searchParams.set('page',Math.floor((page-1)*per/n)+1);
  if(u.toString()!==location.href)location.replace(u.toString());
 }
 function fit(){var n=count();if(n)go(n)}
 if(!fitted())fit();
 // Fitted for this window, but this page's pager took more room than the
 // one it was fitted on: fewer rows, never more, so it can only settle.
 else if(document.documentElement.scrollHeight>innerHeight){var n=count();if(n&&n<per)go(n)}
 // Back to a page kept from before (the back-forward cache) fitted elsewhere.
 addEventListener('pageshow',function(e){if(e.persisted&&!fitted())fit()});
 // Not while a row is open: on a phone, scrolling it shows or hides the
 // address bar, which is a resize, and a refit would close it.
 var w;addEventListener('resize',function(){clearTimeout(w);w=setTimeout(function(){if(!fitted()&&!t.querySelector('tr.open'))fit()},500)});
 // A row pressed, or entered from the keyboard, shows all of itself.
 function toggle(tr){var o=tr.classList.toggle('open');tr.setAttribute('aria-expanded',o?'true':'false')}
 t.addEventListener('click',function(e){var tr=e.target.closest('tr[tabindex]');if(tr&&!e.target.closest('a'))toggle(tr)});
 t.addEventListener('keydown',function(e){var tr=e.target.closest('tr[tabindex]');if(tr&&!e.target.closest('a')&&(e.key==='Enter'||e.key===' ')){e.preventDefault();toggle(tr)}});
})();
</script>"#;

/// The log's filters (and the window it was fitted to) as a query string for
/// the pager's links: HTML-escaped and `&amp;`-joined, as `activity_pager`
/// takes it.
fn audit_query(f: &crate::db::ModLogFilter) -> String {
    let mut parts = Vec::new();
    for (k, v) in [
        ("user", &f.username),
        ("season", &f.season),
        ("action", &f.action),
        ("fit", &f.fit),
    ] {
        if !v.is_empty() {
            parts.push(format!("{k}={}", esc(&url_enc(v))));
        }
    }
    parts.join("&amp;")
}

/// A query-string value, percent-encoded.
fn url_enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
