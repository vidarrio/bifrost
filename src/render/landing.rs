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
    /// Where the card leads: the board once live, the administration for a
    /// draft (only those who run it see one), else the signup page.
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
    if let Some(url) = sign_in {
        out.push_str(&format!(
            r#"<div class="empty"><a href="{}">Sign in</a> to see the seasons you're in.</div>"#,
            esc(url)
        ));
    }
    out
}

/// The header's optional "home" link (instance settings).
pub fn home_pill(inst: &crate::db::Instance) -> String {
    if inst.home_url.is_empty() {
        String::new()
    } else {
        format!(
            r#"<a class="pill home" href="{}">{}</a>"#,
            esc(&inst.home_url),
            esc(inst.home_label())
        )
    }
}

/// The brand tagline: `<site address> · archipelago`.
pub fn landing_tag(site_address: &str) -> String {
    match site_address.trim() {
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
    fn the_tag_shows_the_site_address() {
        assert_eq!(super::landing_tag(""), "archipelago");
        assert_eq!(
            super::landing_tag("bifrost.example.org"),
            "bifrost.example.org · archipelago"
        );
        assert_eq!(super::landing_tag("a<b"), "a&lt;b · archipelago");
    }

    #[test]
    fn the_home_pill_says_its_label() {
        let mut inst = crate::db::Instance::default();
        assert_eq!(super::home_pill(&inst), "", "no link, no pill");
        inst.home_url = "https://example.org/?a=1&b=2".into();
        assert_eq!(
            super::home_pill(&inst),
            r#"<a class="pill home" href="https://example.org/?a=1&amp;b=2">HOME</a>"#
        );
        inst.home_label = "<Our site>".into();
        assert!(super::home_pill(&inst).ends_with(">&lt;Our site&gt;</a>"));
    }
}
