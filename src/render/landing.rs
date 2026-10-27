//! The home page's live parts: roster, connect info, links, past seasons.

use super::*;

/// Build the landing page's season roster grid from live data: one card per AP
/// slot (game + its player(s)), co-op slots listing every player with the AP
/// slot's own player first, ordered alphabetically by that lead player's name.
/// `ap_names` maps an AP slot to its connect name (from the live board) so we
/// can tell which co-op member is the slot's primary.
pub fn landing_roster(
    slots: &[SlotRow],
    ap_names: &std::collections::HashMap<i64, String>,
) -> String {
    if slots.is_empty() {
        return r#"<div class="empty" style="grid-column:1/-1">The roster fills in once the season's game is running.</div>"#
            .to_string();
    }
    struct Entry {
        key: String,
        game: String,
        who: String,
    }
    let mut entries: Vec<Entry> = group_by_ap(slots)
        .into_iter()
        .map(|g| {
            // The common game is shared — show it as everyone's, sorted first.
            if g.is_common {
                return Entry {
                    key: String::new(),
                    game: g.game.to_string(),
                    who: "Everyone".to_string(),
                };
            }
            let mut names: Vec<String> = g.rows.iter().map(|r| r.name.clone()).collect();
            names.sort_by_key(|n| n.to_ascii_lowercase());
            // Put the AP slot's own player (the primary) first, rest alphabetical.
            if let Some(lead) = ap_names.get(&g.ap_slot) {
                if let Some(pos) = names.iter().position(|n| n.eq_ignore_ascii_case(lead)) {
                    let p = names.remove(pos);
                    names.insert(0, p);
                }
            }
            let key = names
                .first()
                .map(|n| n.to_ascii_lowercase())
                .unwrap_or_default();
            Entry {
                key,
                game: g.game.to_string(),
                who: names.join(" & "),
            }
        })
        .collect();
    entries.sort_by(|a, b| a.key.cmp(&b.key));

    let mut out = String::new();
    for e in &entries {
        out.push_str(&format!(
            r#"<div class="slot"><div class="who">{}</div><div class="game">{}</div></div>"#,
            esc(&e.who),
            esc(&e.game)
        ));
    }
    out
}

/// Frontpage roster for the gap between seasons (no live game yet): the people
/// signed up for the *next* season, straight from the submission queue, so the
/// section fills in live as they register. Mirrors `landing_roster`'s card style

/// The home page's "Connect to" block: each live season's address (or "ask
/// an admin" until the sysadmin sets the connect host), then the password note.
pub fn landing_connect(live: &[(String, Option<String>)], password_note: &str) -> String {
    let mut out = String::new();
    if live.is_empty() {
        out.push_str(r#"<div class="lbl">Connect to</div><div class="cs">no season is live</div>"#);
    }
    for (name, addr) in live {
        let lbl = if live.len() == 1 {
            "Connect to".to_string()
        } else {
            format!("{} — connect to", esc(name))
        };
        out.push_str(&format!(
            r#"<div class="lbl">{lbl}</div><div class="cs">{}</div>"#,
            esc(addr.as_deref().unwrap_or("ask an admin"))
        ));
    }
    if !password_note.trim().is_empty() {
        out.push_str(&format!(
            r#"<div class="pw" style="margin-top:8px">{}</div>"#,
            esc(password_note)
        ));
    }
    out
}

/// The header's optional "home" link (instance settings).
pub fn home_pill(url: &str) -> String {
    if url.is_empty() {
        String::new()
    } else {
        format!(r#"<a class="pill" href="{}">home</a>"#, esc(url))
    }
}

/// The brand tagline: `<domain> · archipelago`.
pub fn landing_tag(domain: &str) -> String {
    match domain.trim() {
        "" => "archipelago".into(),
        d => format!("{} · archipelago", esc(d)),
    }
}

/// The home page's Discords column (nothing when none are configured).
pub fn landing_discords(discords: &[crate::db::Discord]) -> String {
    if discords.is_empty() {
        return String::new();
    }
    let items: String = discords
        .iter()
        .map(|d| {
            let note = if d.note.is_empty() {
                String::new()
            } else {
                format!(r#" <span class="d">{}</span>"#, esc(&d.note))
            };
            format!(
                r#"<li><a href="{}" target="_blank" rel="noopener">{}</a>{note}</li>"#,
                esc(&d.url),
                esc(&d.name)
            )
        })
        .collect();
    format!(
        r#"<div><h2 class="sec">Discords</h2><div class="card"><ul class="links">{items}</ul></div></div>"#
    )
}

/// (who + game) so the section looks identical whether a season is live or not.
pub fn landing_signups(subs: &[SubmissionRow]) -> String {
    if subs.is_empty() {
        return r#"<div class="empty" style="grid-column:1/-1">No sign-ups yet — be the first to <a href="signup">claim a game for next season</a>.</div>"#
            .to_string();
    }
    struct Entry {
        key: String,
        game: String,
        who: String,
    }
    let mut entries: Vec<Entry> = subs
        .iter()
        .map(|s| {
            // Primary player first, then any co-op seats, matching the live roster.
            let mut names = vec![s.slot_name.clone()];
            names.extend(
                s.coop_names
                    .split(',')
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_string),
            );
            Entry {
                key: s.slot_name.to_ascii_lowercase(),
                game: s.game.clone(),
                who: names.join(" & "),
            }
        })
        .collect();
    entries.sort_by(|a, b| a.key.cmp(&b.key));

    let mut out = String::new();
    for e in &entries {
        out.push_str(&format!(
            r#"<div class="slot"><div class="who">{}</div><div class="game">{}</div></div>"#,
            esc(&e.who),
            esc(&e.game)
        ));
    }
    out
}

/// The frontpage "Past seasons" block — a card per archived season linking to
/// its detail page. Empty string when there are none (so the section hides).
pub fn past_seasons_section(seasons: &[(i64, String, i64)]) -> String {
    if seasons.is_empty() {
        return String::new();
    }
    let mut out = String::from(r#"<h2 class="sec">Past seasons</h2><div class="grid roster">"#);
    for (id, name, ts) in seasons {
        out.push_str(&format!(
            r#"<a class="slot cardlink" href="/s/{id}/season"><div class="game">{}</div><div class="who">{}</div></a>"#,
            esc(name),
            fmt_date(*ts),
        ));
    }
    out.push_str("</div>");
    out
}
