//! The home page's live parts: seasons, links, past seasons.

use super::*;

/// One of the signed-in player's games in a live season.
pub struct MyGame {
    pub season: Id,
    pub game_id: Id,
    pub slot: String,
    pub game: String,
}

/// A season on the home page: one the viewer can see that isn't archived.
pub struct SeasonCard {
    pub id: Id,
    pub name: String,
    pub state: crate::db::State,
    /// Where the card leads: the board once live, else the signup page (the
    /// season's administration for a draft its admins open).
    pub href: String,
    pub games: usize,
    pub players: usize,
    /// The viewer plays in it, or has signed up.
    pub member: bool,
    /// The viewer's own games in it, while it's live.
    pub mine: Vec<MyGame>,
}

fn count(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

/// The home page's seasons: a card each, the viewer's own games linked from
/// theirs. `sign_in` is where a visitor who isn't signed in can, since the
/// seasons they're in may be private.
pub fn landing_seasons(cards: &[SeasonCard], sign_in: Option<&str>) -> String {
    if cards.is_empty() {
        return match sign_in {
            Some(url) => format!(
                r#"<div class="empty">No season is open to visitors right now. <a href="{}">Sign in</a> to see the ones you're in.</div>"#,
                esc(url)
            ),
            None => r#"<div class="empty">No season under way right now.</div>"#.to_string(),
        };
    }
    let mut out = String::new();
    for c in cards {
        let (cls, label) = match c.state {
            crate::db::State::Live => ("live", "live"),
            crate::db::State::Generating => ("gen", "generating"),
            crate::db::State::Signup => ("signup", "signups open"),
            _ => ("draft", "draft"),
        };
        let you = if !c.mine.is_empty() {
            let games: Vec<String> = c
                .mine
                .iter()
                .map(|g| {
                    format!(
                        r#"<a href="/s/{}/game/{}">{} · {}</a>"#,
                        g.season,
                        g.game_id,
                        esc(&g.slot),
                        esc(&g.game)
                    )
                })
                .collect();
            format!(r#"<div class="you">You play {}</div>"#, games.join(", "))
        } else if c.member {
            r#"<div class="you">You're in</div>"#.to_string()
        } else {
            String::new()
        };
        out.push_str(&format!(
            r#"<div class="slot stretch {cls}"><span class="st">{label}</span><div class="game"><a class="cover" href="{}">{}</a></div><div class="meta">{} · {}</div>{you}</div>"#,
            esc(&c.href),
            esc(&c.name),
            count(c.games, "game"),
            count(c.players, "player"),
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
    let host = rest.split(['/', '\\', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    // Without a port, as the page's own script shows it (`URL.hostname`).
    let host = match host.rsplit_once(':') {
        // A port, not part of a bare IPv6 address.
        Some((h, p))
            if !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && (h.ends_with(']') || !h.contains(':')) =>
        {
            h
        }
        _ => host,
    };
    let host = host.to_ascii_lowercase();
    match host.chars().count() {
        0 => "home".into(),
        n if n > 24 => format!("{}…", host.chars().take(23).collect::<String>()),
        _ => host,
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

#[cfg(test)]
mod tests {
    #[test]
    fn the_home_link_is_named_after_its_host() {
        for (url, want) in [
            ("https://example.org/", "example.org"),
            ("https://Example.org:443/x", "example.org"),
            ("http://user:pw@example.org:8080", "example.org"),
            ("https://example.org\\evil", "example.org"),
            ("https://[::1]:8080/", "[::1]"),
            ("", "home"),
            (
                "https://a-very-long-host-name.example.org/",
                "a-very-long-host-name.e…",
            ),
        ] {
            assert_eq!(super::home_label(url), want, "{url}");
        }
    }
}
