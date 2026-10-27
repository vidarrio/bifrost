//! An archived season's summary page.

use super::*;

/// An archived season's detail page: final standings per game + this season's
/// frozen minesweeper leaderboards.
pub fn season_page(
    s: &crate::season::SeasonSummary,
    activity: &[crate::db::ActivityRow],
    page: i64,
    pages: i64,
    rpp: i64,
    f: &LogFilter,
) -> Html<String> {
    let mut out = page_head(None);
    out.push_str(&format!(
        r#"<h1>{}</h1><div class="crumb"><a href="/">← Bifrost</a> · {} player{} · finalized {}</div>"#,
        esc(&s.name),
        s.player_count,
        if s.player_count == 1 { "" } else { "s" },
        fmt_date(s.finalized_at),
    ));

    out.push_str(r#"<h2>Final standings</h2>"#);
    if s.games.is_empty() {
        out.push_str(r#"<div class="empty">No games recorded.</div>"#);
    } else {
        out.push_str(r#"<div class="grid">"#);
        for g in &s.games {
            out.push_str(&format!(
                r#"<div class="slot {}"><span class="st">{}</span><div class="game">{}</div>
<div class="meta">{} / {} checks · {}%</div><div class="seat"><span class="who">{}</span></div></div>"#,
                esc(&g.status),
                esc(&g.label),
                esc(&g.game),
                g.checks,
                g.total,
                g.pct,
                esc(&g.players.join(" & ")),
            ));
        }
        out.push_str("</div>");
    }

    if s.minesweeper.iter().any(|m| !m.is_empty()) {
        out.push_str(r#"<h2>Minesweeper — this season</h2>"#);
        for m in &s.minesweeper {
            if m.is_empty() {
                continue;
            }
            out.push_str(&mine_leaderboard_html(&m.label, &m.time, &m.rate, 5));
        }
    }

    // Season activity log, replayed with "x ago" frozen to the season-end time.
    // Shown whenever the season has any logged activity (participants non-empty),
    // so the filter bar + "no matches" still render under an active filter.
    if !f.participants.is_empty() {
        out.push_str(r#"<h2>Activity log</h2>"#);
        render_log_section(
            &mut out,
            "season",
            activity,
            page,
            pages,
            rpp,
            Some(s.finalized_at),
            f,
            false,
        );
    }

    out.push_str("</main></body></html>");
    Html(out)
}

pub fn conn_line(snap: &BoardSnapshot) -> String {
    let mut s = String::new();
    if snap.connected {
        s.push_str(r#"<span class="dot on"></span>connected"#);
    } else {
        s.push_str(r#"<span class="dot off"></span>waiting for the server…"#);
    }
    if let Some(seed) = &snap.seed {
        s.push_str(&format!(" · seed {}", esc(seed)));
    }
    s
}
