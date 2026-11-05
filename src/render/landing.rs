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
    commons: &[crate::db::SeasonCommon],
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
            // A common game is shared: show it as everyone's, by its name as on
            // the board, sorted first.
            if g.is_common {
                let name = g.rows[0]
                    .common_key
                    .as_deref()
                    .and_then(|k| commons.iter().find(|c| c.key == k))
                    .map_or(g.game, |c| c.name.as_str());
                return Entry {
                    key: format!("\u{0}{}", name.to_lowercase()),
                    game: name.to_string(),
                    who: "Everyone".to_string(),
                };
            }
            let mut names: Vec<String> = g.rows.iter().map(|r| r.name.clone()).collect();
            names.sort_by_key(|n| n.to_ascii_lowercase());
            // Put the AP slot's own player (the primary) first, rest alphabetical.
            if let Some(lead) = ap_names.get(&g.ap_slot)
                && let Some(pos) = names.iter().position(|n| n.eq_ignore_ascii_case(lead))
            {
                let p = names.remove(pos);
                names.insert(0, p);
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

/// One of the signed-in player's games in a live season, for the home page.
pub struct MyGame {
    pub season: Id,
    pub game_id: Id,
    pub slot: String,
    pub game: String,
}

/// The home page's "Join" block: the signed-in player's own games, each
/// linking to its page, then where each live season's connection details are
/// (its own address and password, on its board).
pub fn landing_connect(live: &[(Id, String)], mine: &[MyGame]) -> String {
    let mut out = String::new();
    if !mine.is_empty() {
        out.push_str(r#"<div class="lbl">Your games</div>"#);
        for g in mine {
            let season = live
                .iter()
                .find(|(id, _)| *id == g.season)
                .map(|(_, n)| n.as_str())
                .unwrap_or("");
            out.push_str(&format!(
                r#"<div class="pw"><a href="/s/{}/game/{}">{} · {}</a> in {}</div>"#,
                g.season,
                g.game_id,
                esc(&g.slot),
                esc(&g.game),
                esc(season)
            ));
        }
    }
    out.push_str(if mine.is_empty() {
        r#"<div class="lbl">Connect to</div>"#
    } else {
        r#"<div class="lbl" style="margin-top:12px">Connect to</div>"#
    });
    if live.is_empty() {
        out.push_str(r#"<div class="cs">no season is live</div>"#);
    }
    for (id, name) in live {
        out.push_str(&format!(
            r#"<div class="pw"><a href="/s/{id}/board">{}</a>: connection details on its live board</div>"#,
            esc(name)
        ));
    }
    out
}

/// The header's optional "home" link (instance settings).
pub fn home_pill(url: &str) -> String {
    if url.is_empty() {
        String::new()
    } else {
        format!(
            r#"<a class="pill" href="{}">{} ↗</a>"#,
            esc(url),
            esc(&home_label(url))
        )
    }
}

/// What the header's home link says: the site it goes to, by host name.
pub fn home_label(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    if host.is_empty() {
        "home".into()
    } else {
        host.to_string()
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

/// Frontpage roster for the gap between seasons (no live game yet): the people
/// signed up for the *next* season, straight from the submission queue, so the
/// section fills in live as they register. Mirrors `landing_roster`'s card style
/// (who + game) so the section looks identical whether a season is live or not.
pub fn landing_signups(subs: &[SubmissionRow]) -> String {
    if subs.is_empty() {
        return r#"<div class="empty" style="grid-column:1/-1">No sign-ups yet — be the first to <a href="/signups">sign up for next season</a>.</div>"#
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
                s.seat_names
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
pub fn past_seasons_section(seasons: &[(Id, String, i64)]) -> String {
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
