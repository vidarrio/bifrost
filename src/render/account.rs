//! Sign-in, setup, invite and reset pages, and the users console.

use super::*;

pub(super) const AUTH_STYLE: &str = r#"<style>
.authform{display:grid;gap:12px;max-width:380px;margin-bottom:10px}
.authform label{display:grid;gap:4px;font-size:13px;color:var(--muted)}
.authform input,.linkform input,.linkform select,.utable select{background:var(--bg);color:var(--fg);border:1px solid var(--bd);border-radius:6px;padding:7px 9px;font-size:13px}
.authform button,.linkform button{justify-self:start}
.utable{width:100%;border-collapse:collapse;font-size:13px;margin-bottom:8px}
.utable th{text-align:left;font-size:11px;letter-spacing:.06em;text-transform:uppercase;color:var(--muted);padding:6px 8px;border-bottom:1px solid var(--bd)}
.utable td{padding:7px 8px;border-bottom:1px solid var(--bd);vertical-align:middle}
.utable form{display:inline}.utable .mono{font-family:var(--mono);font-size:12px}
.utable tr.dead td{color:var(--muted)}
.linkform{display:flex;flex-wrap:wrap;gap:8px;align-items:end;margin:6px 0 14px}
.linkform label{display:grid;gap:4px;font-size:12px;color:var(--muted)}
.minted input{width:100%;font-family:var(--mono);font-size:12.5px;background:var(--bg);color:var(--fg);border:1px solid var(--ok);border-radius:6px;padding:8px 10px;margin-top:6px}
.tag{font-family:var(--mono);font-size:11px;color:var(--muted);border:1px solid var(--bd);border-radius:5px;padding:1px 6px}
</style>"#;

pub(super) fn auth_page(title: &str, crumb: &str, err: Option<&str>, body: &str) -> Html<String> {
    let mut b = page_head(None);
    b.push_str(AUTH_STYLE);
    b.push_str(&format!(
        r#"<h1>{}</h1><div class="crumb">{crumb}</div>"#,
        esc(title)
    ));
    if let Some(e) = err {
        b.push_str(&format!(r#"<div class="banner bad">{}</div>"#, esc(e)));
    }
    b.push_str(body);
    b.push_str("</main></body></html>");
    Html(b)
}

pub(super) const NEW_PASSWORD_FIELDS: &str = r#"<label>Password (at least 10 characters)<input type="password" name="password" autocomplete="new-password" minlength="10" required></label>
<label>Confirm password<input type="password" name="confirm" autocomplete="new-password" minlength="10" required></label>"#;

pub fn login_page(rd: &str, username: &str, err: Option<&str>) -> Html<String> {
    auth_page(
        "Sign in",
        r#"<a href="/">← home</a>"#,
        err,
        &format!(
            r#"<form class="authform" method="post" action="/login">
<input type="hidden" name="rd" value="{}">
<label>Username<input type="text" name="username" value="{}" autocomplete="username" required autofocus></label>
<label>Password<input type="password" name="password" autocomplete="current-password" required></label>
<button type="submit">Sign in</button></form>
<p class="statline">No account? Accounts are invite-only — ask an admin for an invite link. Forgot your password? An admin can give you a reset link.</p>"#,
            esc(rd),
            esc(username)
        ),
    )
}

pub fn setup_page(token: &str, username: &str, err: Option<&str>) -> Html<String> {
    auth_page(
        "First-run setup",
        "create the sysadmin account",
        err,
        &format!(
            r#"<p class="statline">This account administers the whole instance: users, invites, seasons.</p>
<form class="authform" method="post" action="/setup">
<input type="hidden" name="token" value="{}">
<label>Username<input type="text" name="username" value="{}" autocomplete="username" required autofocus></label>
{NEW_PASSWORD_FIELDS}
<button type="submit">Create sysadmin</button></form>"#,
            esc(token),
            esc(username)
        ),
    )
}

pub fn invite_page(token: &str, username: &str, err: Option<&str>) -> Html<String> {
    auth_page(
        "Create your account",
        "you've been invited to Bifrost",
        err,
        &format!(
            r#"<form class="authform" method="post" action="/invite/{}">
<label>Username (3–32 letters, digits, _ . -)<input type="text" name="username" value="{}" autocomplete="username" required autofocus></label>
{NEW_PASSWORD_FIELDS}
<button type="submit">Create account</button></form>"#,
            urlq(token),
            esc(username)
        ),
    )
}

pub fn reset_page(token: &str, username: &str, err: Option<&str>) -> Html<String> {
    auth_page(
        "Reset password",
        &format!("for <b>{}</b>", esc(username)),
        err,
        &format!(
            r#"<form class="authform" method="post" action="/reset/{}">
{NEW_PASSWORD_FIELDS}
<button type="submit">Set password</button></form>
<p class="statline">This signs you out everywhere else.</p>"#,
            urlq(token)
        ),
    )
}

/// "in 3 h" / "in 2 d" / "expired".
pub(super) fn fmt_left(secs: i64) -> String {
    match secs {
        s if s <= 0 => "expired".into(),
        s if s < 3600 => format!("in {} min", (s + 59) / 60),
        s if s < 2 * 86400 => format!("in {} h", (s + 3599) / 3600),
        s => format!("in {} d", (s + 86399) / 86400),
    }
}

pub struct UsersView<'a> {
    pub accounts: &'a [crate::db::AccountRow],
    pub links: &'a [crate::db::LinkRow],
    pub me_id: i64,
    /// Native auth (links and passwords apply).
    pub native: bool,
    /// Roles are dictated by the IdP's groups (read-only here).
    pub idp_roles: bool,
    pub minted: Option<&'a crate::account::Minted>,
    pub err: Option<&'a str>,
    pub now: i64,
}

pub fn users_page(v: &UsersView) -> Html<String> {
    let mut body = console_nav("/admin/users", true);
    if let Some(m) = v.minted {
        body.push_str(&format!(
            r#"<div class="banner ok minted"><b>{}</b> — copy it now; it won't be shown again.
<input readonly id="mintedlink" data-path="{}" value="{}" onclick="this.select()"></div>
<script>(function(){{var i=document.getElementById('mintedlink');i.value=location.origin+i.dataset.path;}})();</script>"#,
            esc(&m.what),
            esc(&m.path),
            esc(&m.path)
        ));
    }

    body.push_str("<h2>Accounts</h2><table class=\"utable\"><tr><th>User</th><th>Role</th><th>Status</th><th></th></tr>");
    for a in v.accounts {
        let me = a.id == v.me_id;
        let role_cell = if v.idp_roles {
            format!(r#"<span class="tag">{}</span>"#, esc(&a.role))
        } else {
            let opts: String = ["user", "moderator", "sysadmin"]
                .iter()
                .map(|r| {
                    let sel = if *r == a.role { " selected" } else { "" };
                    format!(r#"<option value="{r}"{sel}>{r}</option>"#)
                })
                .collect();
            format!(
                r#"<form method="post" action="/admin/users/role"><input type="hidden" name="account_id" value="{}">
<select name="role" onchange="this.form.submit()">{opts}</select></form>"#,
                a.id
            )
        };
        let kind = if a.idp { r#" <span class="tag">idp</span>"# } else { "" };
        let status = if a.disabled { "disabled" } else { "active" };
        let mut actions = String::new();
        if !me {
            actions.push_str(&format!(
                r#"<form method="post" action="/admin/users/disable"><input type="hidden" name="account_id" value="{}"><input type="hidden" name="disabled" value="{}"><button class="ghost" type="submit">{}</button></form> "#,
                a.id,
                !a.disabled,
                if a.disabled { "enable" } else { "disable" }
            ));
        }
        if v.native && a.has_password {
            actions.push_str(&format!(
                r#"<form method="post" action="/admin/links/reset"><input type="hidden" name="account_id" value="{}"><button class="ghost" type="submit">reset link</button></form>"#,
                a.id
            ));
        }
        body.push_str(&format!(
            r#"<tr class="{}"><td class="mono">{}{}{kind}</td><td>{role_cell}</td><td>{status}</td><td>{actions}</td></tr>"#,
            if a.disabled { "dead" } else { "" },
            esc(&a.username),
            if me { " (you)" } else { "" },
        ));
    }
    body.push_str("</table>");
    if v.idp_roles {
        body.push_str(r#"<p class="statline">Roles follow the identity provider's groups on this instance.</p>"#);
    }

    if v.native {
        body.push_str(&format!(
            r#"<h2>Invite links</h2>
<form class="linkform" method="post" action="/admin/links/invite">
<label>Label (optional)<input type="text" name="label" maxlength="80" placeholder="who it's for"></label>
<label>Valid for (days)<input type="number" name="days" value="7" min="1" max="90"></label>
<button type="submit">Mint single-use invite</button></form>
<form class="linkform" method="post" action="/admin/links/community">
<label>Label (optional)<input type="text" name="label" maxlength="80" placeholder="e.g. Discord, season 5"></label>
<label>Valid for (hours)<input type="number" name="hours" value="48" min="1" max="{}"></label>
<button type="submit">Mint community invite</button></form>
<p class="statline">A community invite works for anyone who has it until it expires or you revoke it — fine for a Discord post; single-use invites admit one account.</p>"#,
            90 * 24
        ));
    }

    if !v.links.is_empty() {
        body.push_str("<table class=\"utable\"><tr><th>Link</th><th>Label / for</th><th>Used</th><th>Expires</th><th></th></tr>");
        for l in v.links {
            let live = l.live(v.now);
            let used = match l.max_uses {
                Some(m) => format!("{}/{m}", l.uses),
                None => l.uses.to_string(),
            };
            let label = match (&l.username, l.label.is_empty()) {
                (Some(u), _) => format!("for {}", esc(u)),
                (None, false) => esc(&l.label),
                (None, true) => String::new(),
            };
            let state = if l.revoked {
                "revoked".to_string()
            } else if live {
                fmt_left(l.expires_at - v.now)
            } else if l.expires_at <= v.now {
                "expired".to_string()
            } else {
                "used up".to_string()
            };
            let revoke = if live {
                format!(
                    r#"<form method="post" action="/admin/links/revoke"><input type="hidden" name="id" value="{}"><button class="ghost" type="submit">revoke</button></form>"#,
                    l.id
                )
            } else {
                String::new()
            };
            body.push_str(&format!(
                r#"<tr class="{}"><td><span class="tag">{}</span></td><td>{label}</td><td class="mono">{used}</td><td class="mono" title="{}">{state}</td><td>{revoke}</td></tr>"#,
                if live { "" } else { "dead" },
                esc(&l.kind),
                fmt_date(l.expires_at),
            ));
        }
        body.push_str("</table>");
    }

    auth_page(
        "Console",
        r#"<a href="/">← home</a>"#,
        v.err,
        &body,
    )
}
