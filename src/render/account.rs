//! Sign-in, setup, invite and reset pages, and the users console.

use super::*;

pub(super) const AUTH_STYLE: &str = r#"<style>
.authform{display:grid;gap:12px;max-width:380px;margin-bottom:10px}
.authform label{display:grid;gap:4px;font-size:13px;color:var(--muted)}
.authform input,.linkform input,.linkform select,.utable select{background:var(--bg);color:var(--fg);border:1px solid var(--bd);border-radius:6px;padding:7px 9px;font-size:13px}
.authform button,.linkform button{justify-self:start;padding:8px 14px;font-size:13px}
.utable{width:100%;border-collapse:collapse;font-size:13px;margin-bottom:8px}
.utable th{text-align:left;font-size:11px;letter-spacing:.06em;text-transform:uppercase;color:var(--muted);padding:6px 8px;border-bottom:1px solid var(--bd)}
.utable td{padding:7px 8px;border-bottom:1px solid var(--bd);vertical-align:middle}
.utable form{display:inline}.utable .mono{font-family:var(--mono);font-size:12px;overflow-wrap:anywhere}
.utable tr.dead td{color:var(--muted)}
/* A phone can't fit a table's columns, so each row becomes a card: one line per
   field, its column's name on the left (data-label, from `label_cells`), and
   the actions together at the bottom. Nothing is off-screen. */
@media (max-width:560px){
.utable,.utable tbody{display:block}
.utable tr:has(th):not(:has(td)){display:none}
.utable th{display:block;padding:5px 0;border:0;text-align:left}
.utable tr{display:block;border:1px solid var(--bd);border-radius:10px;padding:6px 12px;margin:0 0 10px;background:var(--panel)}
.utable td{display:block;padding:5px 0;border-bottom:0;text-align:right;overflow-wrap:break-word}
.utable .mono{overflow-wrap:break-word}
.utable td+td{border-top:1px solid var(--bd)}
.utable td::before{content:attr(data-label);float:left;margin-right:12px;font-size:11px;line-height:2;letter-spacing:.06em;text-transform:uppercase;color:var(--muted);overflow-wrap:normal}
.utable td:not([data-label]){text-align:left}
.utable td:not([data-label])::before{content:none}
.utable td:empty{display:none}
.utable td::after{content:"";display:block;clear:both}
}
.linkform{display:flex;flex-wrap:wrap;gap:8px;align-items:end;margin:6px 0 14px}
.linkform label{display:grid;gap:4px;font-size:12px;color:var(--muted)}
.minted input{width:100%;font-family:var(--mono);font-size:12.5px;background:var(--bg);color:var(--fg);border:1px solid var(--ok);border-radius:6px;padding:8px 10px;margin-top:6px}
.tag{font-family:var(--mono);font-size:11px;color:var(--muted);border:1px solid var(--bd);border-radius:5px;padding:1px 6px}
</style>"#;

/// Copy each `utable`'s column names onto its cells (`data-label`), for the
/// card layout a phone gets (see `AUTH_STYLE`). A column with an empty heading,
/// such as the actions, gets no label, and so does a cell that spans columns.
/// Cells that already have a label keep it. Pages built outside `auth_page`
/// that show a `utable`, such as the settings page's refreshed load panel, run
/// their HTML through this too.
pub fn label_cells(body: &str) -> String {
    let mut out = String::with_capacity(body.len() + 256);
    let mut rest = body;
    while let Some(start) = rest.find("<table") {
        let Some(tag_end) = rest[start..].find('>') else {
            break;
        };
        let tag = &rest[start..start + tag_end];
        let Some(len) = rest[start..].find("</table>") else {
            break;
        };
        let is_utable =
            attr(tag, "class").is_some_and(|c| c.split_whitespace().any(|w| w == "utable"));
        out.push_str(&rest[..start]);
        let table = &rest[start..start + len];
        if !is_utable {
            out.push_str(table);
            rest = &rest[start + len..];
            continue;
        }
        let mut labels: Vec<String> = Vec::new();
        for (i, row) in table.split("<tr").enumerate() {
            if i > 0 {
                out.push_str("<tr");
            }
            // The heading row: heading cells (not `<thead>`) and no data cells.
            let has_th = row.contains("<th>") || row.contains("<th ");
            if labels.is_empty() && has_th && !row.contains("<td") {
                for c in row.split("<th").skip(1) {
                    let (attrs, inner) = c.split_once('>').unwrap_or(("", ""));
                    let text = strip_tags(inner.split("</th>").next().unwrap_or(""));
                    let span = attr(attrs, "colspan")
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(1usize);
                    labels.push(text.trim().replace('"', "&quot;"));
                    labels.extend(std::iter::repeat_n(String::new(), span.saturating_sub(1)));
                }
                out.push_str(row);
                continue;
            }
            // Data cells, and any row-heading cells among them, each take
            // their columns; only data cells get labels.
            let mut col = 0usize;
            let mut at = 0usize;
            while let Some((pos, is_td)) = next_cell(&row[at..]) {
                let pos = at + pos;
                out.push_str(&row[at..pos]);
                let open = if is_td { "<td" } else { "<th" };
                out.push_str(open);
                let after = pos + open.len();
                let attrs = row[after..].split_once('>').map_or("", |(a, _)| a);
                let span = attr(attrs, "colspan")
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(1usize);
                if is_td
                    && span == 1
                    && !attrs.contains("data-label")
                    && let Some(l) = labels.get(col).filter(|l| !l.is_empty())
                {
                    out.push_str(&format!(r#" data-label="{l}""#));
                }
                col += span;
                at = after;
            }
            out.push_str(&row[at..]);
        }
        rest = &rest[start + len..];
    }
    out.push_str(rest);
    out
}

/// Where the next `<td` or `<th` cell (not `<thead`) starts, and whether it's a `<td`.
fn next_cell(s: &str) -> Option<(usize, bool)> {
    let mut from = 0;
    while let Some(i) = s[from..].find("<t") {
        let at = from + i;
        let rest = &s[at..];
        for (tag, td) in [("<td", true), ("<th", false)] {
            if rest.starts_with(tag) && rest[tag.len()..].starts_with([' ', '>']) {
                return Some((at, td));
            }
        }
        from = at + 2;
    }
    None
}

/// An attribute's value in an HTML start tag (double-quoted, as Bifrost writes them).
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let at = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    tag[at..].split('"').next()
}

/// Text without its tags (headings are plain text, but be safe).
fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

pub(super) fn auth_page(title: &str, crumb: &str, err: Option<&str>, body: &str) -> Html<String> {
    let mut b = page_head(None);
    b.push_str(AUTH_STYLE);
    b.push_str(&format!(
        r#"<h1>{}</h1><div class="crumb">{crumb}</div>"#,
        esc(title)
    ));
    if let Some(e) = err {
        b.push_str(crate::modlog::REFUSED_MARK);
        b.push_str(&format!(r#"<div class="banner bad">{}</div>"#, esc(e)));
    }
    b.push_str(&label_cells(body));
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
<p class="statline">No account? Accounts are invite-only. Ask an admin for an invite link. Forgot your password? An admin can give you a reset link.</p>"#,
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
            r#"<div class="banner ok minted"><b>{}</b>. Copy it now; it won't be shown again.
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
        let kind = if a.idp {
            r#" <span class="tag">idp</span>"#
        } else {
            ""
        };
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
<p class="statline">A community invite works for anyone who has it until it expires or you revoke it, which is fine for a Discord post; single-use invites admit one account.</p>"#,
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

    auth_page("Console", r#"<a href="/">← home</a>"#, v.err, &body)
}

#[cfg(test)]
mod tests {
    #[test]
    fn table_cells_get_their_column_names() {
        let body = concat!(
            r#"<p>before</p><table class="utable"><tr><th>User</th><th>Label / for</th><th></th></tr>"#,
            r#"<tr class="dead"><td class="mono">bob</td><td>x</td><td><form><button>reset</button></form></td></tr>"#,
            r#"</table><table class="other"><tr><td>untouched</td></tr></table>"#
        );
        let out = super::label_cells(body);
        assert!(
            out.contains(r#"<td data-label="User" class="mono">bob</td>"#),
            "{out}"
        );
        assert!(
            out.contains(r#"<td data-label="Label / for">x</td>"#),
            "{out}"
        );
        assert!(
            out.contains("<td><form>"),
            "an empty heading gives no label: {out}"
        );
        assert!(
            out.contains(r#"<p>before</p>"#) && out.contains("<td>untouched</td>"),
            "{out}"
        );
        assert_eq!(super::label_cells("no tables"), "no tables");
    }

    #[test]
    fn labelling_copes_with_real_markup() {
        let body = concat!(
            r#"<table class="utable wide" id="t"><tr><th>What</th><th colspan="2">Use <small>now</small></th><th>Note</th></tr>"#,
            r#"<tr><td colspan="3">spans</td><td>n</td></tr>"#,
            r#"<tr><td>a</td><td>b</td><td>c</td><td data-label="Mine">d</td></tr>"#,
            r#"<tr><th>row head</th><td>x</td></tr></table>"#
        );
        let out = super::label_cells(body);
        assert!(
            out.contains(r#"<td colspan="3">spans</td><td data-label="Note">n</td>"#),
            "a spanning cell has no label, and shifts the rest: {out}"
        );
        assert!(
            out.contains(
                r#"<td data-label="What">a</td><td data-label="Use now">b</td><td>c</td>"#
            ),
            "headings lose their tags; a spanned heading's extra columns have none: {out}"
        );
        assert!(
            out.contains(r#"<td data-label="Mine">d</td>"#),
            "an existing label stays: {out}"
        );
        assert!(
            out.contains(r#"<tr><th>row head</th><td data-label="Use now">x</td>"#),
            "a row heading takes its column: {out}"
        );
        let thead = super::label_cells(concat!(
            r#"<table class="utable"><thead><tr><th>A</th><th>B</th></tr></thead>"#,
            r#"<tbody><tr><td>1</td><td>2</td></tr></tbody></table>"#
        ));
        assert!(
            thead.contains(r#"<td data-label="A">1</td><td data-label="B">2</td>"#),
            "with a thead: {thead}"
        );
        assert_eq!(
            super::label_cells(&out),
            out,
            "labelling twice changes nothing"
        );
    }
}
