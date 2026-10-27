//! The live board and the claim page.

use super::*;

/// Board-only overrides: let the column fill the viewport (header to match) and
/// render the slot cards as a viewport-responsive grid (as many ~280px columns
/// as fit), so even ultrawides pack more games per row instead of the default
/// two. The activity feed keeps its own readable width (`.feed` max-width), so
/// widening the grid doesn't stretch the log's item↔timestamp gap.
pub(super) const BOARD_STYLE: &str = r#"<style>
main{max-width:none;padding-inline:clamp(20px,4vw,80px)}
header .hwrap{max-width:none;padding-inline:clamp(20px,4vw,80px)}
.boardcols{display:flex;gap:28px;align-items:flex-start}
.boardgrid{flex:1 1 0;min-width:0}
.boardlog{flex:0 0 540px;min-width:0}
.grid.board{grid-template-columns:repeat(auto-fill,minmax(min(400px,100%),1fr))}
.loghdr,.legend,.feed{max-width:760px;margin-inline:auto}
@media(max-width:1000px){.boardcols{flex-direction:column;align-items:stretch}.boardgrid,.boardlog{flex:none;width:100%}}
.seasonprog{margin:2px 0 22px}
.seasonprog .splabel{display:flex;justify-content:space-between;align-items:baseline;gap:12px;font-family:var(--mono);font-size:12px;letter-spacing:.04em;color:var(--muted);margin-bottom:7px}
.seasonprog .splabel b{color:var(--fg);font-weight:600}
.seasonprog .sptrack{height:10px;border-radius:999px;background:var(--panel);border:1px solid var(--bd);overflow:hidden}
.seasonprog .spfill{height:100%;border-radius:999px;background:linear-gradient(90deg,var(--accent),#c58af9);transition:width .6s ease;min-width:2px}
.slot .cardprog{height:5px;border-radius:999px;background:var(--bg);overflow:hidden;margin:2px 0 9px}
.slot .cardprog .cpfill{display:block;height:100%;border-radius:999px;background:linear-gradient(90deg,var(--accent),#c58af9);transition:width .6s ease}
</style>"#;

/// Season-wide location-check progress: the sum of every world's checked
/// locations (`sent`, i.e. `location_checks`) over the sum of their total
/// locations (`world_size`) — the same per-game measure the archive uses,
/// aggregated. Returns "" before any world is known (so the bar stays hidden
/// until Bifrost is connected and has location totals). Co-op slots share one
/// `ap_slot`, so dedup by it to avoid counting a world twice.
pub fn season_progress(snap: &BoardSnapshot, slots: &[SlotRow]) -> String {
    let mut seen = std::collections::HashSet::new();
    let (mut done, mut total) = (0u64, 0u64);
    for s in slots {
        if !seen.insert(s.ap_slot) {
            continue;
        }
        if let Some(a) = snap.ap.get(&s.ap_slot) {
            done += a.sent;
            total += a.world_size as u64;
        }
    }
    if total == 0 {
        return String::new();
    }
    let pct = done * 100 / total;
    let width = done as f64 * 100.0 / total as f64;
    format!(
        r#"<div class="seasonprog"><div class="splabel"><span>Season progress</span><span><b>{done}</b> / {total} checks · <b>{pct}%</b></span></div>
<div class="sptrack" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="{pct}"><div class="spfill" style="width:{width:.1}%"></div></div></div>"#,
    )
}

/// A single slot card's check-progress mini-bar (`sent / world_size`, the same
/// per-game measure as the season bar and archive). Empty until the world's
/// total locations are known (`world_size == 0`), so an unconnected / not-yet-
/// loaded card shows no bar. Width is clamped to 100% (a goaled/released world
/// can report `sent == world_size`).
pub(super) fn card_progress(sent: u64, world_size: u32) -> String {
    if world_size == 0 {
        return String::new();
    }
    let width = (sent as f64 * 100.0 / world_size as f64).min(100.0);
    format!(
        r#"<div class="cardprog" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="{}"><span class="cpfill" style="width:{width:.1}%"></span></div>"#,
        width as u32
    )
}

pub fn board_page(
    snap: &BoardSnapshot,
    slots: &[SlotRow],
    prios: &std::collections::HashSet<(i64, i64, i64)>,
    mc_addr: Option<&str>,
    activity: &[crate::db::ActivityRow],
    inlogic: &std::collections::HashMap<i64, i64>,
    connect: Option<&str>,
) -> Html<String> {
    let mut out = page_head(None);
    out.push_str(BOARD_STYLE);
    let connect = connect
        .map(|a| format!(r#" · AP server <b class="mono">{}</b>"#, esc(a)))
        .unwrap_or_default();
    out.push_str(&format!(
        r#"<h1>Live board</h1><div class="crumb"><a href="/">← Bifrost</a> · <span id="connline">{}</span>{connect}</div>"#,
        conn_line(snap)
    ));

    out.push_str(&format!(
        r#"<div id="seasonprog">{}</div>"#,
        season_progress(snap, slots)
    ));
    out.push_str(r#"<div class="boardcols">"#);
    board_inner(&mut out, snap, slots, prios, mc_addr, activity, inlogic);
    out.push_str("</div></main></body></html>");
    Html(out)
}

/// The inner content of `.boardcols` — the slots grid + the activity sidebar.
/// Shared by the full board page and the `/board/fragment` live-update endpoint
/// (polled by the client so the board updates without a full refresh).
pub(super) fn board_inner(
    out: &mut String,
    snap: &BoardSnapshot,
    slots: &[SlotRow],
    prios: &std::collections::HashSet<(i64, i64, i64)>,
    mc_addr: Option<&str>,
    activity: &[crate::db::ActivityRow],
    inlogic: &std::collections::HashMap<i64, i64>,
) {
    out.push_str(
        r#"<div class="boardgrid"><h2>Slots — tap one to claim, hint, and see its hints</h2>"#,
    );
    if slots.is_empty() {
        out.push_str(r#"<div class="empty">No slots yet — the board fills once Bifrost connects to a running game.</div>"#);
    } else {
        out.push_str(r#"<div class="grid board">"#);
        for g in group_by_ap(slots) {
            let live = snap.ap.get(&g.ap_slot).cloned().unwrap_or_default();
            let (cls, label) = card_status(&live);
            let avail = inlogic
                .get(&g.ap_slot)
                .map(|n| format!(" · {n} available"))
                .unwrap_or_default();
            out.push_str(&format!(
                r#"<a class="slot cardlink {cls}" href="game/{}"><span class="st">{label}</span><div class="game">{}</div>
<div class="meta">{} locations · sent {} · recv {} · {} left{avail}</div>"#,
                g.ap_slot,
                esc(g.game),
                live.world_size,
                live.sent,
                live.received,
                live.still_to_find,
            ));
            out.push_str(&card_progress(live.sent, live.world_size));
            if g.is_common {
                out.push_str(r#"<div class="seat"><span class="who">Everyone</span><span class="by free">shared game</span></div>"#);
                if let Some(addr) = mc_addr {
                    out.push_str(&format!(
                        r#"<div class="seat"><span class="who">join at</span><span class="by">{}</span></div>"#,
                        esc(addr)
                    ));
                }
            } else {
                for r in &g.rows {
                    let by = match &r.claimed_by {
                        Some(who) => format!(r#"<span class="by">{}</span>"#, esc(who)),
                        None => r#"<span class="by free">unclaimed</span>"#.to_string(),
                    };
                    out.push_str(&format!(
                        r#"<div class="seat"><span class="who">{}</span>{by}</div>"#,
                        esc(&r.name)
                    ));
                }
            }
            // Prioritized hints showcase on the FINDER's card ("what others want
            // from you"). `prios` is keyed by (finding, location, item).
            let wanted: Vec<&crate::ap::HintView> = live
                .finder_views
                .iter()
                .filter(|h| !h.found && prios.contains(&(h.finding, h.location, h.item)))
                .collect();
            if !wanted.is_empty() {
                out.push_str(r#"<div class="hintcap">★ wanted from you</div>"#);
                for h in wanted {
                    out.push_str(&format!(r#"<div class="hint">{}</div>"#, esc(&h.text)));
                }
            }
            out.push_str("</a>");
        }
        out.push_str("</div>");
    }

    out.push_str(r#"</div><aside class="boardlog"><h2 class="loghdr"><span>Activity</span><a href="log">full log →</a></h2>"#);
    if activity.is_empty() {
        out.push_str(r#"<div class="empty">Quiet so far. Item sends, goals, releases and collects show up here.</div>"#);
    } else {
        out.push_str(&class_legend());
        render_feed(out, activity, None);
    }
    out.push_str("</aside>");
}

/// Live-update fragment: just the inner `.boardcols` HTML, served (with the
/// current conn line) at `/board/fragment` and swapped in by the board's poller.
pub fn board_fragment(
    snap: &BoardSnapshot,
    slots: &[SlotRow],
    prios: &std::collections::HashSet<(i64, i64, i64)>,
    mc_addr: Option<&str>,
    activity: &[crate::db::ActivityRow],
    inlogic: &std::collections::HashMap<i64, i64>,
) -> String {
    let mut out = String::new();
    board_inner(&mut out, snap, slots, prios, mc_addr, activity, inlogic);
    out
}

pub fn claim_page(
    snap: &BoardSnapshot,
    slots: &[SlotRow],
    me: &str,
    is_admin: bool,
) -> Html<String> {
    let mut out = page_head(Some(me));
    out.push_str(&format!(
        r#"<h1>Claim a slot</h1><div class="crumb"><a href="board">← board</a>{}</div>
<p class="empty">Claim your slot so only you can hint on it. You can unclaim it again if you grabbed the wrong one.</p>"#,
        if is_admin { " · admin" } else { "" },
    ));

    if slots.is_empty() {
        out.push_str(r#"<div class="empty">No slots to claim yet.</div>"#);
        out.push_str("</main></body></html>");
        return Html(out);
    }

    out.push_str(r#"<div class="grid">"#);
    for g in group_by_ap(slots) {
        // The common game is shared by everyone — it isn't claimed.
        if g.is_common {
            continue;
        }
        let live = snap.ap.get(&g.ap_slot).cloned().unwrap_or_default();
        let (cls, _) = card_status(&live);
        out.push_str(&format!(
            r#"<div class="slot {cls}"><div class="game">{}</div>"#,
            esc(g.game)
        ));
        for r in &g.rows {
            let mine = r.claimed_by.as_deref() == Some(me);
            let (by, action) = match &r.claimed_by {
                Some(_) if mine => (
                    r#"<span class="by mine">you</span>"#.to_string(),
                    claim_button(r.id, "unclaim", "Unclaim", true, "/claim"),
                ),
                Some(who) => (
                    format!(r#"<span class="by">{}</span>"#, esc(who)),
                    if is_admin {
                        claim_button(r.id, "unclaim", "Force unclaim", true, "/claim")
                    } else {
                        String::new()
                    },
                ),
                None => (
                    r#"<span class="by free">unclaimed</span>"#.to_string(),
                    claim_button(r.id, "claim", "Claim", false, "/claim"),
                ),
            };
            out.push_str(&format!(
                r#"<div class="seat"><span class="who">{}</span><span>{by} {action}</span></div>"#,
                esc(&r.name)
            ));
            if mine {
                out.push_str(&format!(
                    r#"<form class="hintform" method="post" action="hint">
<input type="hidden" name="slot_id" value="{}"><input name="item" placeholder="hint one of your items…" required>
<button type="submit">Hint</button></form><div class="inlinemsg"></div>"#,
                    r.id
                ));
            }
        }
        out.push_str("</div>");
    }
    out.push_str(CLAIM_HINT_JS);
    out.push_str("</div></main></body></html>");
    Html(out)
}

/// Claim-page inline quick-hint: submit via fetch and show inline feedback
/// instead of navigating to the raw JSON (the game page has its own richer
/// handler). Wires every `.hintform` on the claim page.
pub(super) const CLAIM_HINT_JS: &str = r#"<script>
document.querySelectorAll('form.hintform').forEach(function(f){
 f.addEventListener('submit',function(e){
  e.preventDefault();
  var inp=f.querySelector('input[name=item]'),btn=f.querySelector('button'),msg=f.nextElementSibling;
  var item=inp.value.trim();if(!item)return;
  btn.disabled=true;if(msg){msg.className='inlinemsg';msg.textContent='Hinting…';}
  fetch('hint',{method:'POST',body:new URLSearchParams(new FormData(f))}).then(function(r){return r.json()}).then(function(j){
   btn.disabled=false;
   if(msg){msg.className='inlinemsg '+(j&&j.ok?'ok':'bad');msg.textContent=(j&&j.message)?j.message:(j&&j.ok?'Hinted.':'Hint failed.');}
   if(j&&j.ok)inp.value='';
  }).catch(function(){btn.disabled=false;if(msg){msg.className='inlinemsg bad';msg.textContent='Request failed.';}});
 });
});
</script>"#;

pub(super) fn claim_button(id: i64, op: &str, label: &str, ghost: bool, back: &str) -> String {
    let cls = if ghost { "ghost" } else { "" };
    format!(
        r#"<form method="post" action="claim" style="display:inline;margin-left:6px">
<input type="hidden" name="slot_id" value="{id}"><input type="hidden" name="op" value="{op}">
<input type="hidden" name="back" value="{back}">
<button class="{cls}" type="submit">{label}</button></form>"#
    )
}
