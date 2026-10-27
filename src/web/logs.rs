//! The activity log page: filters and paging.

use super::*;

/// The `#logbody` (feed + pager) for the full-log live poller, for the current
/// filters/page/rpp (query carried through). The filter form is not re-sent.
pub(crate) async fn log_fragment_view(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    Query(q): Query<LogQuery>,
) -> Html<String> {
    let season = ctx.id;
    let kinds = selected_kinds(&q);
    let classes = selected_classes(&q);
    let filter = act_filter(&q, &kinds, &classes);
    let total = st.db.activity_count(season, &filter).await;
    let (rpp, page, pages, offset) = paginate(&q, total);
    let rows = st.db.activity_page(season, &filter, offset, rpp).await;
    let qs = filter_qs(&q);
    let lf = log_filter(&q, &[], &qs);
    render::log_body_fragment(&rows, page, pages, rpp, None, "log", &lf)
}

/// Default page size for the activity log before the client fits it to the
/// viewport, and the clamp bounds for the viewer-supplied `rpp`.
pub(crate) const LOG_RPP_DEFAULT: i64 = 25;
pub(crate) const LOG_RPP_MIN: i64 = 5;
pub(crate) const LOG_RPP_MAX: i64 = 300;

#[derive(Deserialize, Default)]
pub(crate) struct LogQuery {
    /// 1-based page, newest first (page 1 = newest, page n = oldest).
    pub(crate) page: Option<i64>,
    /// Rows per page — set by the client to fill the viewport.
    pub(crate) rpp: Option<i64>,
    // Filters. Checkboxes submit `=1` only when checked (absent = unchecked);
    // none checked = all kinds. since/until are epoch seconds (hidden fields the
    // client fills from local datetime-local inputs); `within` is a relative
    // window in seconds (0 = any).
    pub(crate) kind_send: Option<String>,
    pub(crate) kind_goal: Option<String>,
    pub(crate) kind_release: Option<String>,
    pub(crate) kind_collect: Option<String>,
    pub(crate) class_prog: Option<String>,
    pub(crate) class_useful: Option<String>,
    pub(crate) class_trap: Option<String>,
    pub(crate) class_filler: Option<String>,
    pub(crate) sender: Option<String>,
    pub(crate) receiver: Option<String>,
    pub(crate) within: Option<i64>,
    pub(crate) since: Option<String>,
    pub(crate) until: Option<String>,
}

/// A query string value trimmed to `None` when empty.
pub(crate) fn opt_ne(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|v| !v.is_empty())
}
/// Parse an epoch-second query field (empty/invalid/non-positive → None).
pub(crate) fn parse_ts(s: &Option<String>) -> Option<i64> {
    opt_ne(s)
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|t| *t > 0)
}

/// Event kinds selected in the filter (empty = all).
pub(crate) fn selected_kinds(q: &LogQuery) -> Vec<&'static str> {
    let mut v = Vec::new();
    for (on, k) in [
        (q.kind_send.is_some(), "send"),
        (q.kind_goal.is_some(), "goal"),
        (q.kind_release.is_some(), "release"),
        (q.kind_collect.is_some(), "collect"),
    ] {
        if on {
            v.push(k);
        }
    }
    v
}

/// Item-importance classes selected in the filter (empty = all).
pub(crate) fn selected_classes(q: &LogQuery) -> Vec<&'static str> {
    let mut v = Vec::new();
    for (on, c) in [
        (q.class_prog.is_some(), "progression"),
        (q.class_useful.is_some(), "useful"),
        (q.class_trap.is_some(), "trap"),
        (q.class_filler.is_some(), "filler"),
    ] {
        if on {
            v.push(c);
        }
    }
    v
}

/// Effective lower time bound = later of absolute `since` and relative `within`.
pub(crate) fn since_bound(q: &LogQuery) -> Option<i64> {
    let abs = parse_ts(&q.since);
    let rel = q.within.filter(|w| *w > 0).map(|w| now_secs() - w);
    match (abs, rel) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// Percent-encode a query-string value.
pub(crate) fn pct(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                o.push(b as char)
            }
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

/// Serialize the active filters (not page/rpp) for pager links, `&amp;`-joined
/// so the result drops straight into an HTML href.
pub(crate) fn filter_qs(q: &LogQuery) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (on, key) in [
        (q.kind_send.is_some(), "kind_send"),
        (q.kind_goal.is_some(), "kind_goal"),
        (q.kind_release.is_some(), "kind_release"),
        (q.kind_collect.is_some(), "kind_collect"),
        (q.class_prog.is_some(), "class_prog"),
        (q.class_useful.is_some(), "class_useful"),
        (q.class_trap.is_some(), "class_trap"),
        (q.class_filler.is_some(), "class_filler"),
    ] {
        if on {
            parts.push(format!("{key}=1"));
        }
    }
    if let Some(s) = opt_ne(&q.sender) {
        parts.push(format!("sender={}", pct(s)));
    }
    if let Some(r) = opt_ne(&q.receiver) {
        parts.push(format!("receiver={}", pct(r)));
    }
    if let Some(w) = q.within.filter(|w| *w > 0) {
        parts.push(format!("within={w}"));
    }
    if let Some(t) = parse_ts(&q.since) {
        parts.push(format!("since={t}"));
    }
    if let Some(t) = parse_ts(&q.until) {
        parts.push(format!("until={t}"));
    }
    parts.join("&amp;")
}

/// Resolve (rows-per-page, page, total-pages, offset) from the query + a total.
pub(crate) fn paginate(q: &LogQuery, total: i64) -> (i64, i64, i64, i64) {
    let rpp = q
        .rpp
        .unwrap_or(LOG_RPP_DEFAULT)
        .clamp(LOG_RPP_MIN, LOG_RPP_MAX);
    let pages = ((total + rpp - 1) / rpp).max(1);
    let page = q.page.unwrap_or(1).clamp(1, pages);
    (rpp, page, pages, (page - 1) * rpp)
}

/// The DB filter for a query.
pub(crate) fn act_filter<'a>(
    q: &'a LogQuery,
    kinds: &'a [&'a str],
    classes: &'a [&'a str],
) -> db::ActFilter<'a> {
    db::ActFilter {
        kinds,
        classes,
        sender: opt_ne(&q.sender),
        receiver: opt_ne(&q.receiver),
        since: since_bound(q),
        until: parse_ts(&q.until),
    }
}

/// The render-side filter state for the filter bar + pager links.
pub(crate) fn log_filter<'a>(
    q: &'a LogQuery,
    participants: &'a [String],
    qs: &'a str,
) -> render::LogFilter<'a> {
    render::LogFilter {
        kinds_selected: [
            q.kind_send.is_some(),
            q.kind_goal.is_some(),
            q.kind_release.is_some(),
            q.kind_collect.is_some(),
        ],
        classes_selected: [
            q.class_prog.is_some(),
            q.class_useful.is_some(),
            q.class_trap.is_some(),
            q.class_filler.is_some(),
        ],
        sender: opt_ne(&q.sender).unwrap_or(""),
        receiver: opt_ne(&q.receiver).unwrap_or(""),
        within: q.within.unwrap_or(0).max(0),
        since: parse_ts(&q.since),
        until: parse_ts(&q.until),
        participants,
        qs,
    }
}

/// The full, paginated, filterable activity log for the current (live) season.
pub(crate) async fn log_view(
    State(st): State<AppState>,
    ctx: scope::SeasonCtx,
    Query(q): Query<LogQuery>,
) -> Html<String> {
    let season = ctx.id;
    let kinds = selected_kinds(&q);
    let classes = selected_classes(&q);
    let filter = act_filter(&q, &kinds, &classes);
    let total = st.db.activity_count(season, &filter).await;
    let (rpp, page, pages, offset) = paginate(&q, total);
    let rows = st.db.activity_page(season, &filter, offset, rpp).await;
    let participants = st.db.activity_participants(season).await;
    let qs = filter_qs(&q);
    let lf = log_filter(&q, &participants, &qs);
    render::log_page(&rows, page, pages, rpp, None, "log", &lf)
}
