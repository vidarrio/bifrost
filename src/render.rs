//! HTML for the live board and the claim page. Both combine live figures
//! (`BoardSnapshot`, keyed by AP slot) with the DB's Bifrost slots + claims,
//! grouping co-op Bifrost slots under their shared AP slot.

use axum::response::Html;

use crate::ap::BoardSnapshot;
use crate::db::{SlotRow, SubmissionRow};

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn status_label(status: i64) -> (&'static str, &'static str) {
    match status {
        30 => ("goal", "goaled"),
        20 => ("playing", "playing"),
        10 => ("ready", "ready"),
        5 => ("conn", "connected"),
        _ => ("idle", "—"),
    }
}

const HEAD: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>Bifrost</title><style>
:root{--bg:#0d1117;--panel:#161c26;--bd:#2a3542;--fg:#e6e9ef;--muted:#98a3b6;--accent:#5aa9e6;--ok:#28a56d;--bad:#e24d5f;--warn:#d98324;
--font:"IBM Plex Sans",system-ui,sans-serif;--mono:"IBM Plex Mono",ui-monospace,Menlo,monospace}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font-family:var(--font);line-height:1.5;
display:flex;flex-direction:column;align-items:center}main{width:100%;max-width:900px;padding:16px;padding-block:5vh 40px}
a{color:var(--accent);text-decoration:none}a:hover{text-decoration:underline}
h1{font-size:30px;margin:0 0 2px}.crumb{font-family:var(--mono);font-size:12px;color:var(--muted);margin-bottom:20px}
.dot{display:inline-block;width:8px;height:8px;border-radius:50%;margin-right:6px;vertical-align:middle}.on{background:var(--ok)}.off{background:var(--bad)}
h2{font-size:12px;letter-spacing:.09em;text-transform:uppercase;color:var(--muted);margin:26px 0 10px}
.grid{display:grid;grid-template-columns:repeat(2,1fr);gap:10px}@media(max-width:560px){.grid{grid-template-columns:1fr}}
.slot{background:var(--panel);border:1px solid var(--bd);border-left-width:4px;border-radius:10px;padding:11px 13px}
.slot.goal{border-left-color:var(--ok)}.slot.playing{border-left-color:var(--accent)}.slot.idle{border-left-color:var(--bd)}
.slot .game{font-weight:600;font-size:14px}.slot .st{font-family:var(--mono);font-size:11px;color:var(--muted);float:right}
.slot.goal .st{color:var(--ok)}
.slot .meta{font-family:var(--mono);font-size:11px;color:var(--muted);margin:4px 0 8px}
.seat{display:flex;justify-content:space-between;align-items:center;gap:8px;padding:5px 0;border-top:1px solid var(--bd)}
.seat .who{font-family:var(--mono);font-size:13px}.seat .by{font-family:var(--mono);font-size:11px;color:var(--muted)}
.seat .by.mine{color:var(--ok)}.seat .free{color:var(--warn)}
.slot .hint{font-family:var(--mono);font-size:11px;color:var(--warn);margin-top:4px;padding-left:8px;border-left:2px solid var(--bd)}
.hintform{display:flex;gap:6px;margin:4px 0 8px}.hintform input{flex:1;min-width:0;background:var(--bg);color:var(--fg);border:1px solid var(--bd);border-radius:6px;padding:5px 8px;font-size:12px}
.banner{border-radius:10px;padding:12px 16px;margin:0 0 14px}.banner.ok{background:rgba(40,165,109,.15);border:1px solid var(--ok)}.banner.bad{background:rgba(226,77,95,.13);border:1px solid var(--bad)}
.signup{display:grid;gap:12px;max-width:460px;margin-bottom:10px}.signup label{display:grid;gap:4px;font-size:13px;color:var(--muted)}.signup label.chk{display:flex;flex-direction:row;align-items:center;gap:8px}
.signup input[type=file],.signup input[type=text]{background:var(--bg);color:var(--fg);border:1px solid var(--bd);border-radius:6px;padding:6px 8px;font-size:13px}
code{font-family:var(--mono);font-size:12px;color:var(--accent)}
.cardlink{text-decoration:none;color:inherit;display:block;transition:border-color .1s}.cardlink:hover{border-color:var(--accent);text-decoration:none}
.cardlink:hover .game{color:var(--accent)}
.matches{max-width:460px;margin:-4px 0 12px;display:grid}.matches .match{font-family:var(--mono);font-size:12px;padding:5px 8px;border:1px solid var(--bd);border-top:0;cursor:pointer;background:var(--panel)}.matches .match:hover{background:var(--bd)}
.hintrow{display:flex;justify-content:space-between;align-items:center;gap:10px;font-family:var(--mono);font-size:12.5px;background:var(--panel);border:1px solid var(--bd);border-radius:8px;padding:7px 10px;margin:6px 0}
button{background:var(--accent);color:#0d1117;border:0;border-radius:7px;padding:5px 12px;font-weight:600;font-size:12px;cursor:pointer}
button.ghost{background:transparent;border:1px solid var(--bd);color:var(--muted)}
.feed{display:grid;gap:6px}.ev{display:flex;gap:8px;font-size:13px;background:var(--panel);border:1px solid var(--bd);border-radius:8px;padding:7px 10px}
.ev .i{font-family:var(--mono);font-size:10px;color:var(--muted);min-width:44px}.ev.goal .i{color:var(--ok)}.ev.death .i{color:var(--bad)}.ev.trap .i{color:var(--warn)}
.empty{color:var(--muted);font-size:13px}
</style></head><body><main>"#;

struct Group<'a> {
    ap_slot: i64,
    game: &'a str,
    rows: Vec<&'a SlotRow>,
}

fn group_by_ap(slots: &[SlotRow]) -> Vec<Group<'_>> {
    let mut groups: Vec<Group> = Vec::new();
    for r in slots {
        match groups.last_mut() {
            Some(g) if g.ap_slot == r.ap_slot => g.rows.push(r),
            _ => groups.push(Group {
                ap_slot: r.ap_slot,
                game: &r.game,
                rows: vec![r],
            }),
        }
    }
    groups
}

fn conn_line(snap: &BoardSnapshot) -> String {
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

pub fn board_page(
    snap: &BoardSnapshot,
    slots: &[SlotRow],
    prios: &std::collections::HashSet<(i64, i64, i64)>,
) -> Html<String> {
    let mut out = String::from(HEAD);
    out.push_str(&format!(
        r#"<h1>Live board</h1><div class="crumb"><a href="/">← Bifrost</a> · {} · <a href="/signup">signup</a></div>"#,
        conn_line(snap)
    ));

    out.push_str(r#"<h2>Slots — tap one to claim, hint, and see its hints</h2>"#);
    if slots.is_empty() {
        out.push_str(r#"<div class="empty">No slots yet — the board fills once Bifrost connects to a running game.</div>"#);
    } else {
        out.push_str(r#"<div class="grid">"#);
        for g in group_by_ap(slots) {
            let live = snap.ap.get(&g.ap_slot).cloned().unwrap_or_default();
            let (cls, label) = status_label(live.status);
            out.push_str(&format!(
                r#"<a class="slot cardlink {cls}" href="/game/{}"><span class="st">{label}</span><div class="game">{}</div>
<div class="meta">world {} · sent {} · recv {} · {} out elsewhere</div>"#,
                g.ap_slot,
                esc(g.game),
                live.world_size,
                live.sent,
                live.received,
                live.items_elsewhere,
            ));
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
            for h in live
                .hint_views
                .iter()
                .filter(|h| prios.contains(&(h.finding, h.location, h.item)))
            {
                out.push_str(&format!(r#"<div class="hint">★ {}</div>"#, esc(&h.text)));
            }
            out.push_str("</a>");
        }
        out.push_str("</div>");
    }

    out.push_str(r#"<h2>Activity</h2>"#);
    if snap.events.is_empty() {
        out.push_str(r#"<div class="empty">Quiet so far. Item finds, goals and DeathLinks show up here.</div>"#);
    } else {
        out.push_str(r#"<div class="feed">"#);
        for e in &snap.events {
            out.push_str(&format!(
                r#"<div class="ev {}"><span class="i">{}</span><span>{}</span></div>"#,
                e.icon,
                e.icon,
                esc(&e.text)
            ));
        }
        out.push_str("</div>");
    }
    out.push_str("</main></body></html>");
    Html(out)
}

pub fn claim_page(
    snap: &BoardSnapshot,
    slots: &[SlotRow],
    me: &str,
    is_admin: bool,
) -> Html<String> {
    let mut out = String::from(HEAD);
    out.push_str(&format!(
        r#"<h1>Claim a slot</h1><div class="crumb"><a href="/board">← board</a> · signed in as <b>{}</b>{}</div>
<p class="empty">Claim your slot so only you can hint on it. You can unclaim it again if you grabbed the wrong one.</p>"#,
        esc(me),
        if is_admin { " · admin" } else { "" },
    ));

    if slots.is_empty() {
        out.push_str(r#"<div class="empty">No slots to claim yet.</div>"#);
        out.push_str("</main></body></html>");
        return Html(out);
    }

    out.push_str(r#"<div class="grid">"#);
    for g in group_by_ap(slots) {
        let live = snap.ap.get(&g.ap_slot).cloned().unwrap_or_default();
        let (cls, _) = status_label(live.status);
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
                    r#"<form class="hintform" method="post" action="/hint">
<input type="hidden" name="slot_id" value="{}"><input name="item" placeholder="hint one of your items…" required>
<button type="submit">Hint</button></form>"#,
                    r.id
                ));
            }
        }
        out.push_str("</div>");
    }
    out.push_str("</div></main></body></html>");
    Html(out)
}

pub fn signup_page(subs: &[SubmissionRow], me: &str, is_admin: bool) -> Html<String> {
    let mut b = String::from(HEAD);
    b.push_str(&format!(
        r#"<h1>Season signup</h1><div class="crumb"><a href="/board">← board</a> · signed in as <b>{}</b></div>
<p class="empty">Upload your options YAML for the next season. It's checked against the installed worlds before it's accepted. Playing a custom game? Tick the box and upload its <code>.apworld</code> too. Sharing one game between people (like Don't Starve Together)? List the extra players.</p>
<form method="post" action="/signup" enctype="multipart/form-data" class="signup">
  <label>Options YAML<input type="file" name="yaml" accept=".yaml,.yml" required></label>
  <label class="chk"><input type="checkbox" name="is_custom" value="1"> Custom (non-core) game</label>
  <label>apworld (custom games only)<input type="file" name="apworld" accept=".apworld"></label>
  <label>Co-op players — extra names, comma-separated<input type="text" name="coop" placeholder="e.g. Awoo"></label>
  <button type="submit">Sign up</button>
</form>"#,
        esc(me)
    ));

    b.push_str(r#"<h2>Signed up</h2>"#);
    if subs.is_empty() {
        b.push_str(r#"<div class="empty">Nobody yet.</div>"#);
    } else {
        b.push_str(r#"<div class="grid">"#);
        for s in subs {
            let custom = if s.is_custom {
                r#" <span class="by free">custom</span>"#
            } else {
                ""
            };
            let coop = if s.coop_names.is_empty() {
                String::new()
            } else {
                format!(r#"<div class="meta">+ {}</div>"#, esc(&s.coop_names))
            };
            let del = if is_admin || s.sso == me {
                format!(
                    r#"<form method="post" action="/signup/delete" style="display:inline">
<input type="hidden" name="id" value="{}"><button class="ghost" type="submit">Remove</button></form>"#,
                    s.id
                )
            } else {
                String::new()
            };
            b.push_str(&format!(
                r#"<div class="slot idle"><div class="game">{}{custom}</div>
<div class="meta">{} · by {}</div>{coop}<div class="seat"><span></span>{del}</div></div>"#,
                esc(&s.slot_name),
                esc(&s.game),
                esc(&s.sso),
            ));
        }
        b.push_str("</div>");
    }
    b.push_str("</main></body></html>");
    Html(b)
}

pub fn generate_page(msg: &str, running: bool, count: usize, is_admin: bool) -> Html<String> {
    let mut b = String::from(HEAD);
    b.push_str(
        r#"<h1>Generate season</h1><div class="crumb"><a href="/board">← board</a> · admin</div>"#,
    );
    if !is_admin {
        b.push_str(r#"<div class="banner bad">Admins only.</div></main></body></html>"#);
        return Html(b);
    }
    b.push_str(&format!(
        r#"<p class="empty">{count} submission(s) queued. Generating rolls a new seed from the signups, maps slots + auto-claims, and deploys it — the game then restarts on the new seed.</p>"#
    ));
    let cls = if running { "banner" } else { "banner ok" };
    let mark = if running { "⏳ " } else { "" };
    b.push_str(&format!(r#"<div class="{cls}">{mark}{}</div>"#, esc(msg)));
    if running {
        b.push_str(r#"<script>setTimeout(function(){location.reload()},4000)</script>"#);
    } else {
        b.push_str(
            r#"<form method="post" action="/generate"><button type="submit">Generate the season</button></form>"#,
        );
    }
    b.push_str("</main></body></html>");
    Html(b)
}

pub fn signup_result(text: &str, ok: bool) -> Html<String> {
    let mut b = String::from(HEAD);
    let cls = if ok { "banner ok" } else { "banner bad" };
    b.push_str(&format!(
        r#"<h1>Signup</h1><div class="crumb"><a href="/signup">← signup</a></div>
<div class="{cls}">{}</div>"#,
        esc(text)
    ));
    b.push_str("</main></body></html>");
    Html(b)
}

pub fn hint_result(text: &str, ok: bool, back: &str) -> Html<String> {
    let mut b = String::from(HEAD);
    let cls = if ok { "banner ok" } else { "banner bad" };
    b.push_str(&format!(
        r#"<h1>Hint</h1><div class="crumb"><a href="{}">← back</a></div>
<div class="{cls}">{}</div><div><a href="/board">see the board →</a></div>"#,
        esc(back),
        esc(text)
    ));
    b.push_str("</main></body></html>");
    Html(b)
}

fn claim_button(id: i64, op: &str, label: &str, ghost: bool, back: &str) -> String {
    let cls = if ghost { "ghost" } else { "" };
    format!(
        r#"<form method="post" action="/claim" style="display:inline;margin-left:6px">
<input type="hidden" name="slot_id" value="{id}"><input type="hidden" name="op" value="{op}">
<input type="hidden" name="back" value="{back}">
<button class="{cls}" type="submit">{label}</button></form>"#
    )
}

/// The per-game page: claim/unclaim its players, hint with live item search,
/// and all hints in this world with a "prioritize" toggle.
#[allow(clippy::too_many_arguments)]
pub fn game_page(
    ap_slot: i64,
    game: &str,
    snap: &BoardSnapshot,
    slots: &[SlotRow],
    prios: &std::collections::HashSet<(i64, i64, i64)>,
    item_names: &[String],
    me: &str,
    is_admin: bool,
    cost: i64,
    points: Option<i64>,
) -> Html<String> {
    let live = snap.ap.get(&ap_slot).cloned().unwrap_or_default();
    let back = format!("/game/{ap_slot}");
    let here: Vec<&SlotRow> = slots.iter().filter(|r| r.ap_slot == ap_slot).collect();
    let mut out = String::from(HEAD);
    out.push_str(&format!(
        r#"<h1>{}</h1><div class="crumb"><a href="/board">← board</a> · signed in as <b>{}</b>{}</div>
<div class="meta">world {} · sent {} · received {} · {} items out in other worlds</div>"#,
        esc(game),
        esc(me),
        if is_admin { " · admin" } else { "" },
        live.world_size,
        live.sent,
        live.received,
        live.items_elsewhere,
    ));

    // Players (claim / unclaim)
    out.push_str(r#"<h2>Players</h2><div class="grid">"#);
    for r in &here {
        let mine = r.claimed_by.as_deref() == Some(me);
        let (by, action) = match &r.claimed_by {
            Some(_) if mine => (
                r#"<span class="by mine">you</span>"#.to_string(),
                claim_button(r.id, "unclaim", "Unclaim", true, &back),
            ),
            Some(who) => (
                format!(r#"<span class="by">{}</span>"#, esc(who)),
                if is_admin {
                    claim_button(r.id, "unclaim", "Force unclaim", true, &back)
                } else {
                    String::new()
                },
            ),
            None => (
                r#"<span class="by free">unclaimed</span>"#.to_string(),
                claim_button(r.id, "claim", "Claim", false, &back),
            ),
        };
        out.push_str(&format!(
            r#"<div class="slot idle"><div class="seat"><span class="who">{}</span><span>{by} {action}</span></div></div>"#,
            esc(&r.name)
        ));
    }
    out.push_str("</div>");

    // Hinting — only if the viewer claims a seat on this slot (or is admin)
    let hint_slot = here
        .iter()
        .find(|r| r.claimed_by.as_deref() == Some(me))
        .or_else(|| if is_admin { here.first() } else { None })
        .map(|r| r.id);
    if let Some(sid) = hint_slot {
        let items_json = serde_json::to_string(item_names).unwrap_or_else(|_| "[]".into());
        let have = match points {
            Some(p) => format!("You have <b>{p}</b> hint points. "),
            None => String::new(),
        };
        out.push_str(&format!(
            r#"<h2>Send a hint</h2>
<p class="empty">{have}Each hint in this world costs <b>{cost}</b> points. Search your pool and hint one.</p>
<form method="post" action="/hint" class="hintform">
<input type="hidden" name="slot_id" value="{sid}"><input type="hidden" name="back" value="{back}">
<input id="hintq" name="item" placeholder="search your items…" autocomplete="off" required>
<button type="submit">Hint</button></form>
<div id="hintmatches" class="matches"></div>
<script>
const ITEMS={items_json};
const q=document.getElementById('hintq'),box=document.getElementById('hintmatches');
q.addEventListener('input',function(){{
  const v=q.value.toLowerCase().trim();box.innerHTML='';
  if(!v)return;const w=v.split(/\s+/);
  ITEMS.filter(function(n){{const l=n.toLowerCase();return w.every(function(x){{return l.includes(x)}})}}).slice(0,15).forEach(function(n){{
    const d=document.createElement('div');d.className='match';d.textContent=n;
    d.onclick=function(){{q.value=n;box.innerHTML=''}};box.appendChild(d);}});
}});
</script>"#
        ));
    }

    // All hints in this world, with prioritize toggles
    out.push_str(r#"<h2>Hints in this world</h2>"#);
    if live.hint_views.is_empty() {
        out.push_str(r#"<div class="empty">No hints here yet.</div>"#);
    } else {
        for h in &live.hint_views {
            let on = prios.contains(&(h.finding, h.location, h.item));
            let (cls, label) = if on {
                ("", "★ prioritized")
            } else {
                ("ghost", "☆ prioritize")
            };
            out.push_str(&format!(
                r#"<div class="hintrow"><span>{}</span><form method="post" action="/prioritize" style="display:inline">
<input type="hidden" name="finding" value="{}"><input type="hidden" name="location" value="{}">
<input type="hidden" name="item" value="{}"><input type="hidden" name="back" value="{back}">
<button class="{cls}" type="submit">{label}</button></form></div>"#,
                esc(&h.text),
                h.finding,
                h.location,
                h.item,
            ));
        }
    }

    out.push_str("</main></body></html>");
    Html(out)
}
