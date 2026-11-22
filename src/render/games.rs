//! The Games page: what can be played on this server, before anyone signs up.

use super::*;
use crate::ap_adapter::GameEntry;

/// What the Games page shows.
pub struct GamesView<'a> {
    /// The Archipelago version new seasons use (upstream, as clients see it).
    pub ap_version: &'a str,
    /// Its games, once listed.
    pub core: Option<&'a [GameEntry]>,
    pub catalog: &'a [crate::db::CatalogWorld],
    /// Common games, by name.
    pub commons: &'a [String],
    pub custom_allowed: bool,
    pub manual: bool,
}

const GAMES_STYLE: &str = r#"<style>
.gamesintro{max-width:680px}
.gamefilter{width:100%;max-width:360px;background:var(--bg);color:var(--fg);border:1px solid var(--bd);border-radius:7px;padding:8px 10px;font-size:13px;margin:4px 0 12px}
.gamelist{list-style:none;padding:0;margin:0 0 8px;columns:2 240px;column-gap:24px}
.gamelist li{break-inside:avoid;display:flex;justify-content:space-between;gap:12px;padding:6px 0;border-bottom:1px solid var(--bd);font-size:14px}
.gamelist li[hidden]{display:none}
.gamelist .v{font-family:var(--mono);font-size:12px;color:var(--muted);white-space:nowrap}
.gamelist .note{display:block;color:var(--muted);font-size:12.5px}
</style>"#;

/// Narrow the core list as you type.
const GAMES_JS: &str = r#"<script>
(function(){var f=document.getElementById('gamefilter');if(!f)return;
var items=[].slice.call(document.querySelectorAll('#corelist li')),none=document.getElementById('nomatch');
f.addEventListener('input',function(){var q=f.value.trim().toLowerCase(),shown=0;
items.forEach(function(li){var ok=!q||li.textContent.toLowerCase().indexOf(q)>=0;li.hidden=!ok;if(ok)shown++;});
none.hidden=shown>0;});})();
</script>"#;

pub fn games_page(v: &GamesView) -> Html<String> {
    let mut b = page_head(None);
    b.push_str(GAMES_STYLE);
    b.push_str(r#"<h1>Games</h1><div class="crumb"><a href="/">← Bifrost</a> · <a href="/signups">sign up for a season</a></div>"#);
    b.push_str(r#"<p class="empty gamesintro">What you can play on this server. Pick one when you sign up for a season.</p>"#);

    b.push_str("<h2>Core games</h2>");
    b.push_str(&format!(
        r#"<p class="statline">Archipelago {} — the version new seasons use. A season keeps the version it was generated with. A version beside a game is that world's own.</p>"#,
        esc(v.ap_version)
    ));
    match v.core {
        Some(games) if !games.is_empty() => {
            b.push_str(r#"<input type="search" id="gamefilter" class="gamefilter" placeholder="Filter games" aria-label="Filter games">"#);
            b.push_str(r#"<ul class="gamelist" id="corelist">"#);
            for g in games {
                let ver = g
                    .version
                    .as_deref()
                    .map(|x| format!(r#"<span class="v">{}</span>"#, esc(x)))
                    .unwrap_or_default();
                b.push_str(&format!("<li><span>{}</span>{ver}</li>", esc(&g.game)));
            }
            b.push_str(r#"</ul><p class="empty" id="nomatch" hidden>No game matches.</p>"#);
            b.push_str(GAMES_JS);
        }
        _ => b.push_str(
            r#"<div class="empty">The list of games is being gathered. Reload in a minute.</div>"#,
        ),
    }

    b.push_str("<h2>Custom games</h2>");
    if !v.custom_allowed {
        b.push_str(
            r#"<div class="empty">Custom (unofficial) games are turned off on this server.</div>"#,
        );
    } else {
        if v.catalog.is_empty() {
            b.push_str(r#"<div class="empty">The server's admins haven't vetted any custom games yet.</div>"#);
        } else {
            b.push_str(r#"<p class="statline">Vetted by the server's admins: ready to use.</p><ul class="gamelist">"#);
            for w in v.catalog {
                let name = if w.game.is_empty() {
                    w.file.trim_end_matches(".apworld")
                } else {
                    &w.game
                };
                let ver = if w.version.is_empty() {
                    String::new()
                } else {
                    format!(r#"<span class="v">{}</span>"#, esc(&w.version))
                };
                let note = if w.note.is_empty() {
                    String::new()
                } else {
                    format!(r#"<span class="note">{}</span>"#, esc(&w.note))
                };
                b.push_str(&format!("<li><span>{}{note}</span>{ver}</li>", esc(name)));
            }
            b.push_str("</ul>");
        }
        b.push_str(&format!(
            r#"<p class="statline">You can also bring another custom world's <code>.apworld</code> when you sign up{}.</p>"#,
            if v.manual {
                "; a moderator has to approve it before it can run"
            } else {
                ""
            }
        ));
    }

    if !v.commons.is_empty() {
        b.push_str("<h2>Common games</h2>");
        b.push_str(r#"<p class="statline">Hosted by the server and played together: a season can include them, and everyone with a game in it can join.</p><ul class="gamelist">"#);
        for c in v.commons {
            b.push_str(&format!("<li><span>{}</span></li>", esc(c)));
        }
        b.push_str("</ul>");
    }
    b.push_str("</main></body></html>");
    Html(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view<'a>(
        core: Option<&'a [GameEntry]>,
        catalog: &'a [crate::db::CatalogWorld],
        custom: bool,
    ) -> GamesView<'a> {
        GamesView {
            ap_version: "0.6.8",
            core,
            catalog,
            commons: &[],
            custom_allowed: custom,
            manual: true,
        }
    }

    #[test]
    fn the_games_page_lists_what_there_is() {
        let core = [
            GameEntry {
                game: "A <Hat>".into(),
                version: None,
            },
            GameEntry {
                game: "Aquaria".into(),
                version: Some("1.5.2".into()),
            },
        ];
        let html = games_page(&view(Some(&core), &[], true)).0;
        assert!(html.contains("Archipelago 0.6.8"), "{html}");
        assert!(
            html.contains("<li><span>A &lt;Hat&gt;</span></li>"),
            "{html}"
        );
        assert!(
            html.contains(r#"<li><span>Aquaria</span><span class="v">1.5.2</span></li>"#),
            "{html}"
        );
        assert!(
            html.contains("haven't vetted any custom games yet"),
            "{html}"
        );
        assert!(html.contains("a moderator has to approve it"), "{html}");
    }

    #[test]
    fn the_games_page_before_the_list_and_without_custom_games() {
        let html = games_page(&view(None, &[], false)).0;
        assert!(html.contains("being gathered"), "{html}");
        assert!(
            html.contains("Custom (unofficial) games are turned off"),
            "{html}"
        );
        assert!(!html.contains("bring another custom world"), "{html}");
    }
}
