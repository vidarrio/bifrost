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
display:flex;flex-direction:column;align-items:center}main{width:100%;max-width:900px;padding:16px;padding-block:24px 40px}
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
.prow{display:flex;justify-content:space-between;align-items:center;gap:8px}
header{width:100%;border-bottom:1px solid var(--bd);background:var(--panel)}
header .hwrap{max-width:900px;margin:0 auto;padding:0 16px;height:52px;display:flex;align-items:center;gap:14px}
header .brand{font-weight:700;font-size:16px;color:var(--fg);text-decoration:none}header .brand:hover{text-decoration:none}
header .spacer{flex:1}
.who{font-family:var(--mono);font-size:12px;color:var(--muted)}.who b{color:var(--fg)}.who a{color:var(--accent)}
.pill{display:inline-flex;align-items:center;font-family:var(--mono);font-size:11px;line-height:1;letter-spacing:.04em;text-transform:uppercase;color:var(--muted);text-decoration:none;border:1px solid var(--bd);border-radius:999px;padding:4px 11px}.pill:hover{border-color:var(--accent);color:var(--accent);text-decoration:none}
#adminslot{display:contents}
.matches{max-width:460px;margin:-4px 0 12px;display:grid;max-height:240px;overflow:auto}.matches .match{font-family:var(--mono);font-size:12px;padding:5px 8px;border:1px solid var(--bd);border-top:0;cursor:pointer;background:var(--panel)}.matches .match:hover,.matches .match.sel{background:var(--bd)}
.hintrow{display:flex;justify-content:space-between;align-items:center;gap:10px;font-family:var(--mono);font-size:12.5px;background:var(--panel);border:1px solid var(--bd);border-radius:8px;padding:7px 10px;margin:6px 0}
button{background:var(--accent);color:#0d1117;border:0;border-radius:7px;padding:5px 12px;font-weight:600;font-size:12px;cursor:pointer}
button.ghost{background:transparent;border:1px solid var(--bd);color:var(--muted)}
.feed{display:grid;gap:6px}.ev{display:flex;gap:8px;font-size:13px;background:var(--panel);border:1px solid var(--bd);border-radius:8px;padding:7px 10px}
.ev .i{font-family:var(--mono);font-size:10px;color:var(--muted);min-width:44px}.ev.goal .i{color:var(--ok)}.ev.death .i{color:var(--bad)}.ev.trap .i{color:var(--warn)}
.empty{color:var(--muted);font-size:13px}
button:disabled{opacity:.45;cursor:not-allowed}
.stepper{display:flex;gap:8px;margin:0 0 22px;padding:0;list-style:none;font-family:var(--mono);font-size:12px;max-width:520px}
.stepper li{flex:1;display:flex;align-items:center;gap:8px;color:var(--muted);border-top:2px solid var(--bd);padding-top:8px}
.stepper li.active{color:var(--fg);border-top-color:var(--accent)}.stepper li.done{color:var(--ok);border-top-color:var(--ok)}
.stepper .n{display:inline-flex;width:20px;height:20px;border-radius:50%;background:var(--bd);color:var(--fg);align-items:center;justify-content:center;font-size:11px}
.stepper li.active .n{background:var(--accent);color:#0d1117}.stepper li.done .n{background:var(--ok);color:#0d1117}
.wstep{background:var(--panel);border:1px solid var(--bd);border-radius:14px;padding:20px 22px;max-width:520px}
.wstep h2{margin:0 0 14px;color:var(--fg);font-size:16px;letter-spacing:0;text-transform:none}
.wstep .hint{color:var(--muted);font-size:13px;margin:0 0 10px}
.choice{display:grid;gap:10px;margin-bottom:6px}
.choice label{display:flex;gap:10px;align-items:flex-start;background:var(--bg);border:1px solid var(--bd);border-radius:9px;padding:11px 13px;cursor:pointer;font-size:14px}
.choice label:has(input:checked){border-color:var(--accent)}
.wstep input[type=file],.wstep input[type=text]{width:100%;background:var(--bg);color:var(--fg);border:1px solid var(--bd);border-radius:7px;padding:9px 11px;font-size:13px;margin:2px 0}
.wstep input[type=file]{cursor:pointer}
.wstep input[type=file]::file-selector-button{background:transparent;border:1px solid var(--bd);color:var(--muted);border-radius:6px;padding:5px 11px;margin-right:12px;font-family:var(--font);font-size:12px;font-weight:600;cursor:pointer}
.wstep input[type=file]::file-selector-button:hover{border-color:var(--accent);color:var(--accent)}
#apbtn,#checkbtn{margin-top:12px}
.nav{display:flex;justify-content:space-between;gap:10px;margin-top:18px}.nav:has(>:only-child){justify-content:flex-end}
.nav .back{background:transparent;border:1px solid var(--bd);color:var(--muted)}
.inlinemsg{font-family:var(--mono);font-size:12.5px;margin-top:8px;min-height:1em;word-break:break-word}.inlinemsg.ok{color:var(--ok)}.inlinemsg.bad{color:var(--bad)}
.chk{display:flex;gap:9px;align-items:center;font-size:14px;margin-bottom:8px}
.btnrow{display:flex;flex-wrap:wrap;gap:8px;margin:8px 0}
button.danger{background:var(--bad);color:#fff}
.statline{font-family:var(--mono);font-size:12.5px;color:var(--muted);margin:2px 0 6px}.statline b{color:var(--fg)}
.console{display:flex;gap:8px;margin:8px 0}
.console .acwrap{position:relative;flex:1;min-width:0}
.console input[type=text]{width:100%;background:var(--bg);color:var(--fg);border:1px solid var(--bd);border-radius:7px;padding:8px 10px;font-family:var(--mono);font-size:13px}
.acmenu{position:absolute;left:0;right:0;bottom:100%;z-index:6;margin-bottom:6px;max-height:260px;overflow:auto;background:var(--panel);border:1px solid var(--bd);border-radius:8px;box-shadow:0 -6px 20px rgba(0,0,0,.35)}
#conform{margin-bottom:28px}
.acmenu .m{padding:7px 10px;font-family:var(--mono);font-size:12.5px;cursor:pointer;border-bottom:1px solid var(--bd)}
.acmenu .m:last-child{border-bottom:0}.acmenu .m:hover,.acmenu .m.sel{background:var(--bd)}
pre.out{background:#0b0f14;border:1px solid var(--bd);border-radius:8px;padding:12px;overflow-x:auto;font-family:var(--mono);font-size:12.5px;white-space:pre-wrap;word-break:break-word;color:var(--fg)}
#conlog{max-height:320px;overflow:auto;margin-bottom:8px}
.warnnote{color:var(--warn);font-size:12px;margin:4px 0}
</style></head><body>
<header><div class="hwrap"><a class="brand" href="/">Bifrost</a><span class="spacer"></span><span class="who" id="who"></span><span id="adminslot"></span><a class="pill" href="https://home.example.org/">home</a></div></header>
<script>
fetch('/api/me').then(function(r){return r.json()}).then(function(m){
 var w=document.getElementById('who');
 if(w){if(m&&m.user){w.innerHTML='signed in as <b></b>';w.querySelector('b').textContent=m.user}else{w.innerHTML='<a href="/signup">sign in</a>'}}
 if(m&&m.admin){var a=document.getElementById('adminslot');if(a)a.innerHTML='<a class="pill" href="/admin">admin</a>'}
}).catch(function(){});
</script>
<main>"#;

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
    let mut out = page_head(None);
    out.push_str(&format!(
        r#"<h1>Live board</h1><div class="crumb"><a href="/">← Bifrost</a> · {}</div>"#,
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
        for e in snap.events.iter().take(15) {
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
    let mut out = page_head(Some(me));
    out.push_str(&format!(
        r#"<h1>Claim a slot</h1><div class="crumb"><a href="/board">← board</a>{}</div>
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

pub fn signup_page(subs: &[SubmissionRow], me: &str, is_admin: bool, open: bool) -> Html<String> {
    let mut b = page_head(Some(me));
    b.push_str(r#"<h1>Season signup</h1><div class="crumb"><a href="/board">← board</a></div>"#);
    if !open {
        b.push_str(
            r#"<div class="banner bad">Signups are closed right now. They'll open again before the next season.</div>"#,
        );
        b.push_str(&roster(subs, me, is_admin));
        b.push_str("</main></body></html>");
        return Html(b);
    }
    b.push_str(
        r#"<p class="empty">Three quick steps. We check everything against the server as you go, so the seed rolls clean when the season is generated.</p>
<ol class="stepper">
  <li data-step="1"><span class="n">1</span> Game</li>
  <li data-step="2"><span class="n">2</span> YAML</li>
  <li data-step="3"><span class="n">3</span> Players</li>
</ol>
<form id="wiz" method="post" action="/signup" enctype="multipart/form-data">
  <section class="wstep" data-step="1">
    <h2>Your game</h2>
    <div class="choice">
      <label><input type="radio" name="gtype" value="core" checked> <span>A core / officially supported game</span></label>
      <label><input type="radio" name="gtype" value="custom"> <span>A custom (unofficial) game — I have its <code>.apworld</code></span></label>
    </div>
    <div id="apbox" hidden>
      <p class="hint">Upload the world's <code>.apworld</code>. It installs on the server right away so we can validate your YAML next.</p>
      <input type="file" id="apfile" accept=".apworld">
      <button type="button" id="apbtn" class="back">Install apworld</button>
      <div id="apmsg" class="inlinemsg"></div>
    </div>
    <div class="nav"><button type="button" id="tostep2" data-goto="2">Next →</button></div>
  </section>
  <section class="wstep" data-step="2" hidden>
    <h2>Options YAML</h2>
    <p class="hint">Upload your options file. It's rolled against the installed worlds — it has to pass before you continue.</p>
    <input type="file" name="yaml" id="yamlfile" accept=".yaml,.yml" required>
    <button type="button" id="checkbtn" class="back">Check it</button>
    <div id="checkmsg" class="inlinemsg"></div>
    <div class="nav"><button type="button" class="back" data-goto="1">← Back</button><button type="button" id="tostep3" data-goto="3" disabled>Next →</button></div>
  </section>
  <section class="wstep" data-step="3" hidden>
    <h2>Players</h2>
    <label class="chk"><input type="checkbox" id="coopchk"> Sharing this game between people (co-op, e.g. Don't Starve Together)</label>
    <div id="coopbox" hidden>
      <label class="hint">Extra player names, comma-separated<input type="text" name="coop" placeholder="e.g. Awoo, Abi"></label>
    </div>
    <input type="hidden" name="is_custom" id="iscustom" value="">
    <div class="nav"><button type="button" class="back" data-goto="2">← Back</button><button type="submit">Sign me up</button></div>
  </section>
</form>
<script>
(function(){
 var form=document.getElementById('wiz');
 function show(n){
   form.querySelectorAll('.wstep').forEach(function(s){s.hidden=s.dataset.step!=String(n)});
   document.querySelectorAll('.stepper li').forEach(function(li){var s=Number(li.dataset.step);li.classList.toggle('active',s==n);li.classList.toggle('done',s<n)});
 }
 form.querySelectorAll('[data-goto]').forEach(function(btn){btn.addEventListener('click',function(){if(!btn.disabled)show(Number(btn.dataset.goto))})});
 var installed=false,next1=document.getElementById('tostep2');
 function refresh1(){
   var custom=form.querySelector('input[name=gtype]:checked').value=='custom';
   document.getElementById('apbox').hidden=!custom;
   document.getElementById('iscustom').value=custom?'1':'';
   next1.disabled=custom&&!installed;
 }
 form.querySelectorAll('input[name=gtype]').forEach(function(r){r.addEventListener('change',refresh1)});
 refresh1();
 var apbtn=document.getElementById('apbtn'),apmsg=document.getElementById('apmsg');
 apbtn.addEventListener('click',function(){
   var f=document.getElementById('apfile').files[0];
   if(!f){apmsg.className='inlinemsg bad';apmsg.textContent='Choose an .apworld file first.';return;}
   apmsg.className='inlinemsg';apmsg.textContent='Installing…';apbtn.disabled=true;
   var fd=new FormData();fd.append('apworld',f);
   fetch('/signup/apworld',{method:'POST',body:fd}).then(function(r){return r.json()}).then(function(j){
     apbtn.disabled=false;
     if(j.ok){installed=true;apmsg.className='inlinemsg ok';apmsg.textContent='Installed '+j.world+'. You can continue.';next1.disabled=false;}
     else{installed=false;apmsg.className='inlinemsg bad';apmsg.textContent=j.error||'Install failed.';next1.disabled=true;}
   }).catch(function(){apbtn.disabled=false;apmsg.className='inlinemsg bad';apmsg.textContent='Install failed (network).';});
 });
 var chkbtn=document.getElementById('checkbtn'),chkmsg=document.getElementById('checkmsg'),next2=document.getElementById('tostep3'),yamlfile=document.getElementById('yamlfile');
 yamlfile.addEventListener('change',function(){next2.disabled=true;chkmsg.textContent='';});
 chkbtn.addEventListener('click',function(){
   var f=yamlfile.files[0];
   if(!f){chkmsg.className='inlinemsg bad';chkmsg.textContent='Choose your YAML first.';return;}
   chkmsg.className='inlinemsg';chkmsg.textContent='Checking…';chkbtn.disabled=true;
   var fd=new FormData();fd.append('yaml',f);
   fetch('/signup/check',{method:'POST',body:fd}).then(function(r){return r.json()}).then(function(j){
     chkbtn.disabled=false;
     if(j.ok){chkmsg.className='inlinemsg ok';chkmsg.textContent='✓ '+(j.name||'slot')+' — '+(j.game||'?')+' rolled cleanly.';next2.disabled=false;}
     else{next2.disabled=true;chkmsg.className='inlinemsg bad';chkmsg.textContent=(j.errors&&j.errors.length?j.errors.join('; '):(j.error||'Did not pass.'));}
   }).catch(function(){chkbtn.disabled=false;chkmsg.className='inlinemsg bad';chkmsg.textContent='Check failed (network).';});
 });
 var coopchk=document.getElementById('coopchk');
 coopchk.addEventListener('change',function(){document.getElementById('coopbox').hidden=!coopchk.checked;});
 show(1);
})();
</script>"#,
    );

    b.push_str(&roster(subs, me, is_admin));
    b.push_str("</main></body></html>");
    Html(b)
}

/// The "Signed up" roster of submissions, with a Remove control on your own (or,
/// for an admin, any) entry — posts to `/signup/delete`.
fn roster(subs: &[SubmissionRow], me: &str, is_admin: bool) -> String {
    let mut b = String::from(r#"<h2>Signed up</h2>"#);
    if subs.is_empty() {
        b.push_str(r#"<div class="empty">Nobody yet.</div>"#);
        return b;
    }
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
    b
}

/// A one-field POST form rendered as a single button (optionally with a
/// confirm() guard and a danger style).
fn action_btn(
    action: &str,
    fields: &[(&str, &str)],
    label: &str,
    danger: bool,
    confirm: &str,
) -> String {
    let hidden: String = fields
        .iter()
        .map(|(k, v)| {
            format!(
                r#"<input type="hidden" name="{}" value="{}">"#,
                esc(k),
                esc(v)
            )
        })
        .collect();
    let cls = if danger { "danger" } else { "ghost" };
    let onclick = if confirm.is_empty() {
        String::new()
    } else {
        format!(r#" onclick="return confirm('{}')""#, esc(confirm))
    };
    format!(
        r#"<form method="post" action="{}" style="display:inline">{hidden}<button class="{cls}" type="submit"{onclick}>{}</button></form>"#,
        esc(action),
        esc(label)
    )
}

/// The admin control centre: generate, open/close signups, manage the deployed
/// players + apworlds, reset, and run server commands. Assumes the caller has
/// already confirmed the viewer is an admin.
#[allow(clippy::too_many_arguments)]
pub fn admin_page(
    connected: bool,
    seed: Option<&str>,
    signups_open: bool,
    running: bool,
    gen_msg: &str,
    subs: &[SubmissionRow],
    apworlds: &[String],
    acdata: &str,
) -> Html<String> {
    let mut b = page_head(None);
    b.push_str(
        r#"<h1>Season admin</h1><div class="crumb"><a href="/board">← board</a> · admin</div>"#,
    );

    let conn = if connected { "connected" } else { "waiting" };
    let seed_txt = seed.filter(|s| !s.is_empty()).unwrap_or("—");
    b.push_str(&format!(
        r#"<div class="statline">server <b>{conn}</b> · seed <b>{}</b></div>"#,
        esc(seed_txt)
    ));

    // Signups
    b.push_str(r#"<h2>Signups</h2>"#);
    if signups_open {
        b.push_str(r#"<div class="statline">Signups are <b>open</b>.</div>"#);
        b.push_str(&format!(
            r#"<div class="btnrow">{}</div>"#,
            action_btn(
                "/admin/signups",
                &[("open", "0")],
                "Close signups",
                false,
                ""
            )
        ));
    } else {
        b.push_str(r#"<div class="statline">Signups are <b>closed</b>.</div>"#);
        b.push_str(&format!(
            r#"<div class="btnrow">{}</div>"#,
            action_btn(
                "/admin/signups",
                &[("open", "1")],
                "Open signups",
                false,
                ""
            )
        ));
    }

    // Generate
    b.push_str(r#"<h2>Generate</h2>"#);
    b.push_str(&format!(
        r#"<p class="empty">{} signup(s) queued. Generating rolls a new seed, maps slots + auto-claims, deploys it (the game restarts), and closes signups.</p>"#,
        subs.len()
    ));
    let cls = if running { "banner" } else { "banner ok" };
    let mark = if running { "⏳ " } else { "" };
    b.push_str(&format!(
        r#"<div class="{cls}">{mark}{}</div>"#,
        esc(gen_msg)
    ));
    if running {
        b.push_str(r#"<script>setTimeout(function(){location.reload()},4000)</script>"#);
    } else {
        b.push_str(&format!(
            r#"<div class="btnrow">{}</div>"#,
            action_btn(
                "/admin/generate",
                &[],
                "Generate the season",
                false,
                "Generate a new season now? This restarts the game."
            )
        ));
    }

    // Signups (the queue for the next season)
    b.push_str(r#"<h2>Signups</h2>"#);
    if subs.is_empty() {
        b.push_str(r#"<div class="empty">No signups.</div>"#);
    } else {
        for s in subs {
            let custom = if s.is_custom { " · custom" } else { "" };
            b.push_str(&format!(
                r#"<div class="hintrow"><span>{} · {}{custom} · by {}</span>{}</div>"#,
                esc(&s.slot_name),
                esc(&s.game),
                esc(&s.sso),
                action_btn(
                    "/admin/delete-player",
                    &[("id", &s.id.to_string())],
                    "delete",
                    true,
                    &format!("Delete {}'s signup?", s.slot_name)
                )
            ));
        }
    }
    b.push_str(&format!(
        r#"<div class="btnrow">{}</div>"#,
        action_btn(
            "/admin/clear",
            &[("what", "signups")],
            "Clear all signups",
            true,
            "Delete every signup and its uploaded files? (The current season's players stay.)"
        )
    ));

    // Apworlds (installed)
    b.push_str(r#"<h2>Installed apworlds</h2>"#);
    if apworlds.is_empty() {
        b.push_str(r#"<div class="empty">None installed.</div>"#);
    } else {
        for name in apworlds {
            b.push_str(&format!(
                r#"<div class="hintrow"><span>{}</span>{}</div>"#,
                esc(name),
                action_btn(
                    "/admin/delete-apworld",
                    &[("name", name)],
                    "delete",
                    true,
                    &format!("Delete {name}?")
                )
            ));
        }
    }
    b.push_str(&format!(
        r#"<div class="btnrow">{}</div>"#,
        action_btn(
            "/admin/clear",
            &[("what", "apworlds")],
            "Clear all apworlds",
            true,
            "Delete every installed custom apworld?"
        )
    ));

    // Maintenance + reset
    b.push_str(r#"<h2>Maintenance</h2>"#);
    b.push_str(&format!(
        r#"<div class="btnrow">{}{}{}</div>"#,
        action_btn(
            "/admin/clear",
            &[("what", "players")],
            "Clear current players",
            true,
            "Delete this season's players — all Bifrost slots, their claims, and the staged YAMLs? (Signups stay.)"
        ),
        action_btn(
            "/admin/clear",
            &[("what", "save")],
            "Clear session save",
            true,
            "Delete the running session's save? Progress resets on next server load."
        ),
        action_btn(
            "/admin/clear",
            &[("what", "world")],
            "Clear generated world",
            true,
            "Delete the generated world (current.zip + archives)?"
        ),
    ));
    b.push_str(r#"<p class="warnnote">Full reset removes all apworlds, players, slots, the save, and the world, and closes signups — a clean slate for the next season.</p>"#);
    b.push_str(&format!(
        r#"<div class="btnrow">{}</div>"#,
        action_btn(
            "/admin/clear",
            &[("what", "reset")],
            "Full reset",
            true,
            "FULL RESET: wipe apworlds, players, slots, save, and world, and close signups. This cannot be undone. Continue?"
        )
    ));

    // AP admin console — commands run over fetch and append to a live log, no
    // page reload (see CONSOLE_JS).
    b.push_str(r#"<h2>Server console</h2>"#);
    b.push_str(r#"<p class="empty">Run a server command as admin — e.g. <code>/send Player Item</code>, <code>/hint Player Item</code>, <code>/players</code>. The <code>!admin</code> prefix is added for you.</p>"#);
    b.push_str(r#"<pre class="out" id="conlog" hidden></pre>"#);
    b.push_str(
        r#"<form class="console" id="conform" method="post" action="/admin/console" autocomplete="off"><div class="acwrap"><input type="text" name="cmd" id="concmd" placeholder="/players" autocomplete="off" required><div class="acmenu" id="accands" hidden></div></div><button type="submit">Run</button></form>"#,
    );
    // Command autocomplete: ACDATA (players/games/items/locations) is embedded,
    // then CONSOLE_JS reads it. Split so the JS's braces don't fight format!.
    b.push_str("<script>\n(function(){\nvar ACDATA=");
    b.push_str(&acdata.replace("</", "<\\/"));
    b.push_str(";\n");
    b.push_str(CONSOLE_JS);
    b.push_str("\n})();\n</script>");

    b.push_str("</main></body></html>");
    Html(b)
}

/// The per-game hint search. Expects `ITEMS` (this world's item names) defined
/// just above it. Substring filter with a live dropdown; ↑/↓ highlight (and
/// scroll into view), Tab accepts, Enter submits the form.
const HINT_JS: &str = r#"var q=document.getElementById('hintq'),box=document.getElementById('hintmatches');
var hits=[],hsel=-1;
function hrender(){
 box.innerHTML='';
 hits.forEach(function(n,i){var d=document.createElement('div');d.className='match'+(i===hsel?' sel':'');d.textContent=n;d.addEventListener('mousedown',function(e){e.preventDefault();q.value=n;hits=[];box.innerHTML='';q.focus();});box.appendChild(d);});
 var s=box.querySelector('.sel');if(s)s.scrollIntoView({block:'nearest'});
}
q.addEventListener('input',function(){
 var v=q.value.toLowerCase().trim();if(!v){hits=[];box.innerHTML='';return;}
 var w=v.split(/\s+/);
 hits=ITEMS.filter(function(n){var l=n.toLowerCase();return w.every(function(x){return l.indexOf(x)>=0})}).slice(0,15);
 hsel=hits.length?0:-1;hrender();
});
q.addEventListener('keydown',function(e){
 if(!hits.length)return;
 if(e.key==='ArrowDown'){e.preventDefault();hsel=(hsel+1)%hits.length;hrender();}
 else if(e.key==='ArrowUp'){e.preventDefault();hsel=(hsel-1+hits.length)%hits.length;hrender();}
 else if(e.key==='Tab'){e.preventDefault();if(hsel>=0){q.value=hits[hsel];hits=[];box.innerHTML='';}}
 else if(e.key==='Escape'){hits=[];box.innerHTML='';}
});"#;

/// The admin console's client-side command completer. Expects `ACDATA` (from
/// `Board::autocomplete_data`) to be defined just above it. Suggests commands,
/// then player names, then item/location names for the target player's game —
/// the trailing name arg is matched against the whole remainder (names have
/// spaces). Tab accepts the highlighted suggestion; Enter submits.
const CONSOLE_JS: &str = r#"var SPEC={
 '/help':{},'/license':{},'/options':{},'/save':{},'/players':{},'/status':{},'/exit':{},'/datastore':{},'/countdown':{},'/option':{},
 '/collect':{f:['player']},'/release':{f:['player']},'/allow_release':{f:['player']},'/forbid_release':{f:['player']},'/alias':{f:['player']},
 '/send':{f:['player'],t:'item'},'/send_multiple':{f:['num','player'],t:'item'},'/hint':{f:['player'],t:'item'},
 '/send_location':{f:['player'],t:'location'},'/hint_location':{f:['player'],t:'location'}
};
var CMDS=Object.keys(SPEC);
var inp=document.getElementById('concmd'),menu=document.getElementById('accands');
var cands=[],sel=-1;
function has(q){q=q.toLowerCase();return function(s){return s.toLowerCase().indexOf(q)>=0}}
function analyze(val){
 if(!/\s/.test(val)){var q=val.toLowerCase();return CMDS.filter(function(c){return c.indexOf(q)===0}).slice(0,12).map(function(c){return {label:c,full:c+' '}});}
 var toks=val.trim().split(/\s+/),cmd=toks[0],spec=SPEC[cmd];
 if(!spec)return [];
 var fixed=spec.f||[],tail=spec.t,args=toks.slice(1),trail=/\s$/.test(val);
 var idx=trail?args.length:args.length-1,cur=trail?'':(args[idx]||'');
 if(idx<fixed.length){
  if(fixed[idx]!=='player')return [];
  return ACDATA.players.filter(has(cur)).slice(0,15).map(function(p){var k=args.slice(0,idx);k.push(p);return {label:p,full:[cmd].concat(k).join(' ')+' '}});
 }
 if(!tail)return [];
 var pIdx=fixed.indexOf('player'),pName=pIdx>=0?args[pIdx]:null,g=pName?ACDATA.game[pName]:null;
 var pool=(tail==='item'?ACDATA.items[g]:ACDATA.locations[g])||[];
 var q=args.slice(fixed.length).join(' ').trim().toLowerCase();
 var lead=[cmd].concat(args.slice(0,fixed.length)).join(' ');
 return pool.filter(function(x){return x.toLowerCase().indexOf(q)>=0}).slice(0,20).map(function(x){return {label:x,full:lead+' '+x}});
}
function render(){
 if(!cands.length){menu.hidden=true;menu.innerHTML='';return;}
 menu.innerHTML='';
 cands.forEach(function(c,i){var d=document.createElement('div');d.className='m'+(i===sel?' sel':'');d.textContent=c.label;d.addEventListener('mousedown',function(e){e.preventDefault();pick(i);});menu.appendChild(d);});
 menu.hidden=false;
 var s=menu.querySelector('.sel');if(s)s.scrollIntoView({block:'nearest'});
}
function refresh(){cands=analyze(inp.value);sel=cands.length?0:-1;render();}
function pick(i){if(i<0||i>=cands.length)return;inp.value=cands[i].full;inp.focus();refresh();}
inp.addEventListener('input',refresh);
inp.addEventListener('focus',refresh);
inp.addEventListener('blur',function(){setTimeout(function(){menu.hidden=true;},150);});
inp.addEventListener('keydown',function(e){
 if(menu.hidden||!cands.length)return;
 if(e.key==='ArrowDown'){e.preventDefault();sel=(sel+1)%cands.length;render();}
 else if(e.key==='ArrowUp'){e.preventDefault();sel=(sel-1+cands.length)%cands.length;render();}
 else if(e.key==='Tab'){e.preventDefault();pick(sel<0?0:sel);}
 else if(e.key==='Escape'){menu.hidden=true;}
});
var form=document.getElementById('conform'),log=document.getElementById('conlog'),run=form.querySelector('button');
function append(t){log.hidden=false;log.textContent+=(log.textContent?'\n':'')+t;log.scrollTop=log.scrollHeight;}
form.addEventListener('submit',function(e){
 e.preventDefault();
 var cmd=inp.value.trim();if(!cmd)return;
 menu.hidden=true;append('$ '+cmd);run.disabled=true;
 var fd=new FormData();fd.append('cmd',cmd);
 fetch('/admin/console',{method:'POST',body:fd}).then(function(r){return r.json()}).then(function(j){
  append(j&&j.output?j.output:'(no output)');run.disabled=false;inp.value='';inp.focus();cands=[];render();
 }).catch(function(){append('error: request failed');run.disabled=false;});
});"#;

pub fn signup_result(text: &str, ok: bool) -> Html<String> {
    let mut b = page_head(None);
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
    let mut b = page_head(None);
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

/// A small result/feedback page with a message and a link back.
pub fn notice(text: &str, ok: bool, back: &str) -> Html<String> {
    let mut b = page_head(None);
    let cls = if ok { "banner ok" } else { "banner bad" };
    b.push_str(&format!(
        r#"<h1>Heads up</h1><div class="crumb"><a href="{}">← back</a></div>
<div class="{cls}">{}</div>"#,
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
    let mut out = page_head(Some(me));
    out.push_str(&format!(
        r#"<h1>{}</h1><div class="crumb"><a href="/board">← board</a>{}</div>
<div class="meta">world {} · sent {} · received {} · {} items out in other worlds</div>"#,
        esc(game),
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
            r#"<div class="slot idle"><div class="prow"><span class="who">{}</span><span>{by} {action}</span></div></div>"#,
            esc(&r.name)
        ));
    }
    out.push_str("</div>");

    // Hinting — shown only to a player who claims a seat on this slot. Admins do
    // not get it here; they hint via the server console instead.
    let hint_slot = here
        .iter()
        .find(|r| r.claimed_by.as_deref() == Some(me))
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
<div id="hintmatches" class="matches"></div>"#
        ));
        out.push_str("<script>\nvar ITEMS=");
        out.push_str(&items_json);
        out.push_str(";\n");
        out.push_str(HINT_JS);
        out.push_str("\n</script>");
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

/// Page shell. The header bar (brand · "signed in as …" · home) lives in `HEAD`
/// and is filled client-side from `/api/me`, so the identity isn't needed here.
fn page_head(_me: Option<&str>) -> String {
    String::from(HEAD)
}
