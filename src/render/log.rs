//! The activity log: feed rendering, filters and paging.

use super::*;

/// (css class, short label) for an activity kind.
pub(super) fn ev_kind(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "send" => ("send", "sent"),
        "goal" => ("goal", "goal"),
        "release" => ("release", "release"),
        "collect" => ("collect", "collect"),
        _ => ("", "·"),
    }
}

/// Render activity rows into a `.feed`. `ref_ts = None` means relative times run
/// against "now" (live); `Some(t)` freezes them to `t` (an archived season's end
/// time) — emitted as `data-ref` so the client keeps the same frozen value. The
/// exact timestamp rides along in `data-ts`; the client fills the title tooltip.
/// Render a row's text, colouring the sent item by its importance class. The
/// stored send text is "sender → receiver: item"; the item is wrapped in
/// `.itm` so `.ev.ic-<class> .itm` colours just the item, not the names.
pub(super) fn ev_text_html(r: &crate::db::ActivityRow) -> String {
    if r.kind == "send"
        && r.class.is_some()
        && let Some((pre, rest)) = r.text.split_once(" → ")
        && let Some((recv, item)) = rest.split_once(": ")
    {
        return format!(
            r#"{} → {}: <span class="itm">{}</span>"#,
            esc(pre),
            esc(recv),
            esc(item)
        );
    }
    esc(&r.text)
}

/// A compact colour legend for sent-item importance (used on the board, where
/// there are no filter chips to teach the colours).
pub(super) fn class_legend() -> String {
    let mut s = String::from(r#"<div class="legend">"#);
    for c in ["progression", "useful", "trap", "filler"] {
        s.push_str(&format!(
            r#"<span class="lg ic-{c}"><span class="dotc"></span>{c}</span>"#
        ));
    }
    s.push_str("</div>");
    s
}

pub(super) fn render_feed(out: &mut String, rows: &[crate::db::ActivityRow], ref_ts: Option<i64>) {
    let now = ref_ts.unwrap_or_else(now_secs);
    out.push_str(r#"<div class="feed">"#);
    for r in rows {
        let (cls, label) = ev_kind(&r.kind);
        let ic = r
            .class
            .as_deref()
            .map(|c| format!(" ic-{c}"))
            .unwrap_or_default();
        let refattr = match ref_ts {
            Some(t) => format!(r#" data-ref="{t}""#),
            None => String::new(),
        };
        out.push_str(&format!(
            r#"<div class="ev {cls}{ic}"><span class="i">{label}</span><span class="txt">{}</span><time class="ago" data-ts="{}"{refattr} title="{}">{}</time></div>"#,
            ev_text_html(r),
            r.ts,
            fmt_date(r.ts),
            fmt_ago(now - r.ts),
        ));
    }
    out.push_str("</div>");
}

/// The windowed page numbers for a classic pager: 1 2 3 … x-1 x x+1 … n-2 n-1 n.
/// `None` marks an ellipsis gap. Page 1 is newest, `n` is oldest.
pub(super) fn page_numbers(cur: i64, n: i64) -> Vec<Option<i64>> {
    let mut set: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
    for p in [1, 2, 3, cur - 1, cur, cur + 1, n - 2, n - 1, n] {
        if (1..=n).contains(&p) {
            set.insert(p);
        }
    }
    let mut out = Vec::new();
    let mut prev = 0i64;
    for p in set {
        if prev != 0 && p > prev + 1 {
            out.push(None);
        }
        out.push(Some(p));
        prev = p;
    }
    out
}

/// A numbered pager (hidden when there's a single page). Links carry the active
/// filter query string `qs` (HTML-escaped, `&amp;`-joined) plus `rpp` so
/// navigation keeps the filters + the viewer's fitted page size.
pub(super) fn activity_pager(base: &str, qs: &str, page: i64, pages: i64, rpp: i64) -> String {
    if pages <= 1 {
        return String::new();
    }
    let pre = if qs.is_empty() {
        String::new()
    } else {
        format!("{qs}&amp;")
    };
    // NB: the arrow class is `pgnav`, NOT `nav` — `.nav` is a global wizard class
    // (margin-top:18px) that would shove the arrow boxes out of line.
    let link = |p: i64| format!(r#"{base}?{pre}page={p}&amp;rpp={rpp}"#);
    let mut out = String::from(r#"<nav class="pager">"#);
    if page > 1 {
        out.push_str(&format!(
            r#"<a class="pg pgnav" href="{}" title="newer">‹</a>"#,
            link(page - 1)
        ));
    } else {
        out.push_str(r#"<span class="pg pgnav disabled">‹</span>"#);
    }
    for item in page_numbers(page, pages) {
        match item {
            None => out.push_str(r#"<span class="gap">…</span>"#),
            Some(p) if p == page => out.push_str(&format!(r#"<span class="pg cur">{p}</span>"#)),
            Some(p) => out.push_str(&format!(r#"<a class="pg" href="{}">{p}</a>"#, link(p))),
        }
    }
    if page < pages {
        out.push_str(&format!(
            r#"<a class="pg pgnav" href="{}" title="older">›</a>"#,
            link(page + 1)
        ));
    } else {
        out.push_str(r#"<span class="pg pgnav disabled">›</span>"#);
    }
    out.push_str("</nav>");
    out
}

/// Render-side state for the activity-log filter bar + pager links.
pub struct LogFilter<'a> {
    /// Checked kinds, in order: send, goal, release, collect.
    pub kinds_selected: [bool; 4],
    /// Checked item classes, in order: progression, useful, trap, filler.
    pub classes_selected: [bool; 4],
    pub sender: &'a str,
    pub receiver: &'a str,
    /// Relative window in seconds (0 = any time).
    pub within: i64,
    /// Absolute bounds as epoch seconds (the hidden fields; the client shows
    /// them as local datetime-local inputs).
    pub since: Option<i64>,
    pub until: Option<i64>,
    /// Distinct participant names for the sender/receiver autocomplete.
    pub participants: &'a [String],
    /// Active-filter query string for pager links (HTML-escaped, `&amp;`-joined).
    pub qs: &'a str,
}

/// The filter bar: event-kind checkboxes, sender/receiver (autocompleted from
/// the season's participants), a relative window, and an absolute from/to range.
pub(super) fn filter_form(base: &str, f: &LogFilter) -> String {
    let mut out =
        format!(r#"<form class="logfilter" id="logfilter" method="get" action="{base}">"#);
    out.push_str(r#"<div class="ff kinds">"#);
    // Collect isn't offered as a filter — in auto mode it always rides along
    // with a goal/release, so it's not a distinct thing to filter on. Collect
    // events still appear in the unfiltered feed.
    for (i, (name, label)) in [
        ("kind_send", "sends"),
        ("kind_goal", "goals"),
        ("kind_release", "releases"),
    ]
    .iter()
    .enumerate()
    {
        let ck = if f.kinds_selected[i] { " checked" } else { "" };
        out.push_str(&format!(
            r#"<label class="kindchip"><input type="checkbox" name="{name}" value="1"{ck}>{label}</label>"#
        ));
    }
    out.push_str("</div>");
    out.push_str(&format!(
        r#"<div class="ff"><span class="acwrap"><input type="text" name="sender" placeholder="sender" autocomplete="off" data-ac value="{}"><div class="acbox"></div></span></div>"#,
        esc(f.sender)
    ));
    out.push_str(&format!(
        r#"<div class="ff"><span class="acwrap"><input type="text" name="receiver" placeholder="receiver" autocomplete="off" data-ac value="{}"><div class="acbox"></div></span></div>"#,
        esc(f.receiver)
    ));
    // Row 2: item-importance filter — the colour dots double as the legend for
    // the colour-coded items in the feed.
    out.push_str(r#"<div class="ffbreak"></div><div class="ff classrow">"#);
    for (i, (name, label)) in [
        ("class_prog", "progression"),
        ("class_useful", "useful"),
        ("class_trap", "trap"),
        ("class_filler", "filler"),
    ]
    .iter()
    .enumerate()
    {
        let ck = if f.classes_selected[i] {
            " checked"
        } else {
            ""
        };
        out.push_str(&format!(
            r#"<label class="kindchip classchip ic-{label}"><input type="checkbox" name="{name}" value="1"{ck}><span class="dotc"></span>{label}</label>"#
        ));
    }
    out.push_str("</div>");
    // Row 3: time filters on their own line.
    out.push_str(r#"<div class="ffbreak"></div>"#);
    out.push_str(r#"<div class="ff"><select name="within">"#);
    for (val, label) in [
        (0, "any time"),
        (3600, "last hour"),
        (21600, "last 6h"),
        (86400, "last 24h"),
        (604800, "last 7d"),
        (2592000, "last 30d"),
    ] {
        let sel = if f.within == val { " selected" } else { "" };
        out.push_str(&format!(r#"<option value="{val}"{sel}>{label}</option>"#));
    }
    out.push_str("</select></div>");
    let ts_val = |t: Option<i64>| t.map(|v| v.to_string()).unwrap_or_default();
    out.push_str(&format!(
        r#"<div class="ff"><label class="ffl">from</label><input type="datetime-local" id="since_dt"><input type="hidden" name="since" id="since_ts" value="{}"></div>"#,
        ts_val(f.since)
    ));
    out.push_str(&format!(
        r#"<div class="ff"><label class="ffl">to</label><input type="datetime-local" id="until_dt"><input type="hidden" name="until" id="until_ts" value="{}"></div>"#,
        ts_val(f.until)
    ));
    out.push_str(&format!(
        r#"<div class="ff ffbtns"><button type="submit">Filter</button><a class="ghost pill" href="{base}">Clear</a></div>"#
    ));
    out.push_str("</form>");
    // Participant pool for the custom (stylable) sender/receiver autocomplete,
    // replacing the native <datalist> popup (which can't be styled).
    let players = serde_json::to_string(f.participants)
        .unwrap_or_else(|_| "[]".to_string())
        .replace("</", "<\\/");
    out.push_str(&format!(r#"<script>window.bifPlayers={players}</script>"#));
    out
}

/// Shared body for the log pages: filter bar + feed + numbered pager.
/// The swappable part of the log: the feed + pager (no filter form). Served at
/// `/log/fragment` and swapped into `#logbody` by the live poller.
pub(super) fn render_log_body(
    out: &mut String,
    base: &str,
    rows: &[crate::db::ActivityRow],
    page: i64,
    pages: i64,
    rpp: i64,
    ref_ts: Option<i64>,
    f: &LogFilter,
) {
    if rows.is_empty() {
        out.push_str(r#"<div class="empty">No activity matches these filters.</div>"#);
    } else {
        render_feed(out, rows, ref_ts);
        out.push_str(&activity_pager(base, f.qs, page, pages, rpp));
    }
}

/// Filter form (static) + the swappable `#logbody` (feed + pager). `live` marks
/// the body for the client poller (true only for the current season's `/log`).
#[allow(clippy::too_many_arguments)]
pub(super) fn render_log_section(
    out: &mut String,
    base: &str,
    rows: &[crate::db::ActivityRow],
    page: i64,
    pages: i64,
    rpp: i64,
    ref_ts: Option<i64>,
    f: &LogFilter,
    live: bool,
) {
    out.push_str(&filter_form(base, f));
    out.push_str(if live {
        r#"<div id="logbody" data-live>"#
    } else {
        r#"<div id="logbody">"#
    });
    render_log_body(out, base, rows, page, pages, rpp, ref_ts, f);
    out.push_str("</div>");
}

/// Just the `#logbody` inner HTML, for the `/log/fragment` live poller.
pub fn log_body_fragment(
    rows: &[crate::db::ActivityRow],
    page: i64,
    pages: i64,
    rpp: i64,
    ref_ts: Option<i64>,
    base: &str,
    f: &LogFilter,
) -> Html<String> {
    let mut out = String::new();
    render_log_body(&mut out, base, rows, page, pages, rpp, ref_ts, f);
    Html(out)
}

/// The full-log page is just the 760px log column, so narrow `main` + the
/// header to match — otherwise the header's right-side info (home/admin/signed-
/// in) sits at the 900px edge, out past where the content ends.
pub(super) const LOG_STYLE: &str = r#"<style>
main{max-width:760px}
header .hwrap{max-width:760px}
</style>"#;

/// The full, paginated, filterable activity log. `ref_ts = None` → relative
/// times live; `Some(t)` → frozen (archived season).
pub fn log_page(
    rows: &[crate::db::ActivityRow],
    page: i64,
    pages: i64,
    rpp: i64,
    ref_ts: Option<i64>,
    base: &str,
    f: &LogFilter,
) -> Html<String> {
    let mut out = page_head(None);
    out.push_str(LOG_STYLE);
    out.push_str(
        r#"<h1>Activity log</h1><div class="crumb"><a href="board">← board</a> · item sends, goals, releases</div>"#,
    );
    render_log_section(&mut out, base, rows, page, pages, rpp, ref_ts, f, true);
    out.push_str("</main></body></html>");
    Html(out)
}
