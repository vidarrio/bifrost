//! The settings page's live load panel and charts.

use super::*;

/// The live-load panel's contents (also served alone, every 10 s, by
/// `/admin/settings/load`). `names`: season id → name.
pub fn load_panel(
    load: Option<&crate::load::Load>,
    names: &std::collections::HashMap<i64, String>,
) -> String {
    const MIB: u64 = 1 << 20;
    let Some(l) = load.filter(|l| l.latest().is_some()) else {
        return r#"<p class="statline">Collecting the first sample…</p>"#.into();
    };
    let s = l.latest().unwrap();
    let mut b = String::new();
    for w in crate::load::warnings(l) {
        b.push_str(&format!(r#"<div class="banner warn">⚠ {}</div>"#, esc(&w)));
    }
    let pct = |used: u64, total: u64| {
        if total == 0 {
            0.0
        } else {
            used as f32 * 100.0 / total as f32
        }
    };
    let mem_pct: Vec<f32> = l
        .history
        .iter()
        .map(|x| pct(x.mem_used, x.mem_total))
        .collect();
    let cpu: Vec<f32> = l.history.iter().map(|x| x.cpu).collect();
    let mem_what = if l.cgroup { "container limit" } else { "host" };
    b.push_str(r#"<div class="loadcharts">"#);
    b.push_str(&load_chart(
        "Memory used",
        &format!("{:.0}%", pct(s.mem_used, s.mem_total)),
        &format!(
            "{} of {} MiB · {mem_what}",
            s.mem_used / MIB,
            s.mem_total / MIB
        ),
        &mem_pct,
        s.at,
        &l.history.iter().map(|x| x.at).collect::<Vec<_>>(),
    ));
    b.push_str(&load_chart(
        "Host CPU busy",
        &format!("{:.0}%", s.cpu),
        &format!("of {} core{}", l.cores, if l.cores == 1 { "" } else { "s" }),
        &cpu,
        s.at,
        &l.history.iter().map(|x| x.at).collect::<Vec<_>>(),
    ));
    b.push_str("</div>");
    b.push_str(&format!(
        r#"<p class="statline">Data disk: {} MiB free of {} MiB</p>"#,
        l.disk_free / MIB,
        l.disk_total / MIB
    ));
    b.push_str(r#"<table class="utable"><tr><th>What</th><th>Memory</th><th>CPU (% of one core)</th></tr>"#);
    let row = |what: String, (rss, c): (u64, f32)| {
        format!(
            "<tr><td>{what}</td><td>{} MiB</td><td>{c:.0}%</td></tr>",
            rss / MIB
        )
    };
    for (id, v) in &s.servers {
        let name = names
            .get(id)
            .map_or_else(|| format!("season {id}"), |n| esc(n));
        b.push_str(&row(format!("{name} — AP server"), *v));
    }
    if s.servers.is_empty() {
        b.push_str(r#"<tr><td colspan="3" class="muted">No AP server running.</td></tr>"#);
    }
    let (hr, hc, hn) = s.helpers;
    b.push_str(&row(
        format!("Other processes ({hn} running: checks, generation, in-logic, installs)"),
        (hr, hc),
    ));
    b.push_str(&row("Bifrost".into(), s.bifrost));
    b.push_str("</table>");
    b
}

/// One metric over the last hour: a 0–100% line, a hover readout per sample.
pub(super) fn load_chart(
    title: &str,
    now: &str,
    sub: &str,
    values: &[f32],
    at_now: i64,
    at: &[i64],
) -> String {
    let (w, h) = (300.0f32, 80.0f32);
    let n = values.len().max(2) as f32 - 1.0;
    let x = |i: usize| i as f32 * w / n;
    let y = |v: f32| h - v.clamp(0.0, 100.0) * h / 100.0;
    let points: Vec<String> = values
        .iter()
        .enumerate()
        .map(|(i, v)| format!("{:.1},{:.1}", x(i), y(*v)))
        .collect();
    let step = w / n;
    let hits: String = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let ago = (at_now - at.get(i).copied().unwrap_or(at_now)).max(0);
            let when = if ago < 60 { "now".to_string() } else { format!("{} min ago", ago / 60) };
            format!(
                r#"<rect class="hit" x="{:.1}" y="0" width="{step:.1}" height="{h}"><title>{when}: {v:.0}%</title></rect>"#,
                x(i) - step / 2.0
            )
        })
        .collect();
    let mins = at.first().map_or(0, |a| (at_now - a) / 60);
    let span = if mins < 1 {
        "minute".to_string()
    } else {
        format!("{mins} min")
    };
    let since = if mins < 1 {
        "1 min ago".to_string()
    } else {
        format!("{mins} min ago")
    };
    format!(
        r#"<div class="loadchart"><h3>{} <span class="muted">· last {span}</span></h3><div class="now">{}</div><div class="statline">{}</div><div class="plot"><svg viewBox="0 0 {w} {h}" preserveAspectRatio="none" role="img" aria-label="{} over the last {span}, now {}"><line class="grid" x1="0" y1="{h}" x2="{w}" y2="{h}"/><line class="grid" x1="0" y1="{:.1}" x2="{w}" y2="{:.1}" stroke-dasharray="3 3"/><polyline class="line" vector-effect="non-scaling-stroke" points="{}"/>{hits}</svg><span class="ylab">50%</span></div><div class="axis"><span>{since}</span><span>now</span></div></div>"#,
        esc(title),
        esc(now),
        esc(sub),
        esc(title),
        esc(now),
        h / 2.0,
        h / 2.0,
        points.join(" "),
    )
}
