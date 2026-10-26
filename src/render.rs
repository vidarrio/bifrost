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

/// Percent-encode a value for use in a query string (AP output filenames are
/// usually already filename-safe, but slot names can carry spaces/punctuation).
fn urlq(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// Human-readable byte size for the admin file list.
fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.0} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

/// The signature favicon — a dark rounded square with a gold `›` caret,
/// matching the home launcher. Served at `/favicon.svg`.
pub const FAVICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" width="32" height="32">
  <rect width="32" height="32" rx="7" fill="#121a26"/>
  <path d="M13 9.5 L21 16 L13 22.5" fill="none" stroke="#d8b46b" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>
</svg>"##;

fn status_label(status: i64) -> (&'static str, &'static str) {
    match status {
        30 => ("goal", "goaled"),
        20 => ("playing", "playing"),
        10 => ("ready", "ready"),
        5 => ("conn", "connected"),
        _ => ("idle", "—"),
    }
}

/// Status shown on a slot card. `/release` (and a natural 100%) checks every
/// location in a world, so a non-goaled slot with all locations checked reads as
/// "released" (done/out) rather than its stale connection status — mirroring how
/// a goaled slot reads "goaled".
pub fn card_status(live: &crate::ap::ApLive) -> (&'static str, &'static str) {
    if live.status != 30 && live.world_size > 0 && live.sent >= live.world_size as u64 {
        ("released", "released")
    } else {
        status_label(live.status)
    }
}

const HEAD: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>Bifrost</title>
<link rel="icon" type="image/svg+xml" href="/favicon.svg"><style>
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
.slot.released{border-left-color:var(--warn)}
.slot .game{font-weight:600;font-size:14px}.slot .st{font-family:var(--mono);font-size:11px;color:var(--muted);float:right}
.slot.goal .st{color:var(--ok)}
.slot.released .st{color:var(--warn)}
.slot .meta{font-family:var(--mono);font-size:11px;color:var(--muted);margin:4px 0 8px}
.seat{display:flex;justify-content:space-between;align-items:center;gap:8px;padding:5px 0;border-top:1px solid var(--bd)}
.seat .who{font-family:var(--mono);font-size:13px}.seat .by{font-family:var(--mono);font-size:11px;color:var(--muted)}
.seat .by.mine{color:var(--ok)}.seat .free{color:var(--warn)}
.slot .hint{font-family:var(--mono);font-size:11px;color:var(--warn);margin-top:4px;padding-left:8px;border-left:2px solid var(--bd)}
.slot .hintcap{font-size:10px;color:var(--muted);margin-top:9px;text-transform:uppercase;letter-spacing:.05em}
.hintform{display:flex;gap:6px;margin:4px 0 8px}.hintform input{flex:1;min-width:0;background:var(--bg);color:var(--fg);border:1px solid var(--bd);border-radius:6px;padding:5px 8px;font-size:12px}
.banner{border-radius:10px;padding:12px 16px;margin:0 0 14px}.banner.ok{background:rgba(40,165,109,.15);border:1px solid var(--ok)}.banner.bad{background:rgba(226,77,95,.13);border:1px solid var(--bad)}
.signup{display:grid;gap:12px;max-width:460px;margin-bottom:10px}.signup label{display:grid;gap:4px;font-size:13px;color:var(--muted)}.signup label.chk{display:flex;flex-direction:row;align-items:center;gap:8px}
.signup input[type=file],.signup input[type=text]{background:var(--bg);color:var(--fg);border:1px solid var(--bd);border-radius:6px;padding:6px 8px;font-size:13px}
code{font-family:var(--mono);font-size:12px;color:var(--accent)}
.mcjoin{margin:4px 0 10px;padding-left:20px;font-size:13.5px;line-height:1.55}.mcjoin li{margin:4px 0}.mcjoin code{background:var(--bg);padding:1px 5px;border-radius:4px}
details.prism{margin:6px 0 4px;border:1px solid var(--bd);border-radius:7px;padding:2px 12px;background:var(--bg)}
details.prism summary{cursor:pointer;padding:9px 0;font-size:13px;font-weight:600;color:var(--fg);list-style:none}
details.prism summary::-webkit-details-marker{display:none}
details.prism summary::before{content:"\25B8  ";color:var(--accent)}
details.prism[open] summary::before{content:"\25BE  "}
details.prism[open] summary{border-bottom:1px solid var(--bd);margin-bottom:4px}
details.prism .mcjoin{margin-bottom:8px}
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
.hintrow>span{min-width:0;overflow-wrap:anywhere}
.hintrow>form{flex:none}
/* The two states have different-length labels ("★ prioritized" vs
   "☆ prioritize"), so each button sized to its own text. A shared min-width,
   set to about the longer label's natural width, makes both the same width. */
.hintrow button{white-space:nowrap;min-width:112px}
button{background:var(--accent);color:#0d1117;border:0;border-radius:7px;padding:5px 12px;font-weight:600;font-size:12px;cursor:pointer}
button.ghost{background:transparent;border:1px solid var(--bd);color:var(--muted)}
.foundhints{margin:6px 0}
.foundhints>summary{cursor:pointer;font-family:var(--mono);font-size:12px;color:var(--muted);padding:5px 2px;list-style:none;user-select:none}
.foundhints>summary::-webkit-details-marker{display:none}
.foundhints>summary::before{content:"▸ ";color:var(--muted)}
.foundhints[open]>summary::before{content:"▾ "}
.hintrow.found{opacity:.6;color:var(--muted)}
.feed{display:grid;gap:6px;max-width:760px}.ev{display:flex;align-items:center;gap:8px;font-size:13px;background:var(--panel);border:1px solid var(--bd);border-radius:8px;padding:7px 10px}
.ev .i{font-family:var(--mono);font-size:10px;color:var(--muted);min-width:52px}.ev.goal .i{color:var(--ok)}.ev.death .i{color:var(--bad)}.ev.trap .i{color:var(--warn)}.ev.send .i{color:var(--accent)}.ev.release .i{color:var(--warn)}.ev.collect .i{color:var(--muted)}
.ev .txt{flex:1;min-width:0;word-break:break-word}
.ev .ago{margin-left:auto;font-family:var(--mono);font-size:11px;color:var(--muted);white-space:nowrap;cursor:help}
.ic-progression{--c:#d18bff}.ic-useful{--c:#4dd0e1}.ic-trap{--c:#ff6b6b}.ic-filler{--c:#9aa5b1}
.ev .itm{color:var(--c);font-weight:600}
.legend{display:flex;flex-wrap:wrap;gap:4px 14px;margin:-2px 0 10px;font-family:var(--mono);font-size:11px;color:var(--muted)}
.legend .lg{display:inline-flex;align-items:center;gap:6px}
.legend .dotc{width:9px;height:9px;border-radius:50%;background:var(--c);flex:none}
.loghdr{display:flex;align-items:baseline;justify-content:space-between;gap:12px}.loghdr a{font-size:12px}
.pager{display:flex;flex-wrap:wrap;justify-content:center;align-items:center;gap:6px;margin:16px 0 4px;max-width:760px;font-family:var(--mono);font-size:12.5px}
.pager .pg{min-width:32px;height:32px;box-sizing:border-box;line-height:1;display:inline-flex;align-items:center;justify-content:center;padding:0 8px;border:1px solid var(--bd);border-radius:7px;color:var(--muted);text-decoration:none;background:var(--panel)}
.pager a.pg:hover{border-color:var(--accent);color:var(--accent);text-decoration:none}
.pager .pg.cur{background:var(--accent);color:#0d1117;border-color:var(--accent);font-weight:700}
.pager .pg.disabled{opacity:.4}
.pager .pgnav{font-size:16px}
.pager .gap{display:inline-flex;align-items:center;justify-content:center;height:32px;color:var(--muted);padding:0 2px}
.logfilter{display:flex;flex-wrap:wrap;align-items:center;gap:8px 10px;margin:0 0 14px;padding:10px 12px;background:var(--panel);border:1px solid var(--bd);border-radius:10px;font-size:12.5px;max-width:760px;box-sizing:border-box}
.logfilter .ff{display:flex;align-items:center;gap:6px}
.logfilter .ffbreak{flex-basis:100%;height:0;margin:0;padding:0;border:0}
.logfilter .kinds{gap:6px;flex-wrap:wrap}
.logfilter .kindchip{display:inline-flex;align-items:center;height:30px;box-sizing:border-box;margin:0;padding:0 12px;border:1px solid var(--bd);border-radius:7px;background:var(--bg);color:var(--muted);font-size:12.5px;cursor:pointer;user-select:none}
.logfilter .kindchip:has(input:checked){border-color:var(--accent);color:var(--accent)}
.logfilter .kindchip input{display:none}
.logfilter .classchip{gap:7px}
.logfilter .classchip .dotc{width:9px;height:9px;border-radius:50%;background:var(--c);flex:none}
.logfilter .classchip:has(input:checked){border-color:var(--c);color:var(--fg)}
.logfilter .classrow{gap:6px}
.logfilter input[type=text],.logfilter select,.logfilter input[type=datetime-local],.logfilter .ffbtns button,.logfilter .ffbtns .pill{height:30px;box-sizing:border-box}
.logfilter input[type=text],.logfilter select,.logfilter input[type=datetime-local]{background:var(--bg);color:var(--fg);border:1px solid var(--bd);border-radius:7px;padding:0 8px;font-size:12.5px;font-family:var(--font)}
.logfilter input[type=text]{width:120px}
.logfilter .ffl{color:var(--muted)}
.logfilter .ffbtns{margin-left:auto;gap:8px}
.logfilter .ffbtns button,.logfilter .ffbtns .pill{display:inline-flex;align-items:center}
.logfilter .acwrap{position:relative;display:inline-flex}
.acbox{position:absolute;top:calc(100% + 3px);left:0;min-width:150px;z-index:7;max-height:220px;overflow:auto;background:var(--panel);border:1px solid var(--bd);display:none}
.acbox .aci{padding:6px 9px;font-size:12.5px;cursor:pointer;border-bottom:1px solid var(--bd);white-space:nowrap}
.acbox .aci:last-child{border-bottom:0}
.acbox .aci:hover,.acbox .aci.sel{background:var(--bd);color:var(--fg)}
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
.coopown{display:flex;align-items:center;gap:8px;background:var(--bg);border:1px solid var(--bd);border-radius:7px;padding:9px 11px;margin:0 0 12px;font-size:13px}.coopown b{color:var(--fg)}
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
button.finish{background:var(--ok);color:#0d1117}
.adminfin{margin:2px 0 16px}.adminfin .finnote{font-family:var(--mono);font-size:12px;color:var(--ok)}
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
// Fill the header from a cached identity synchronously (before paint) so it
// doesn't flicker empty→filled on reload, then confirm/update from /api/me.
function bifApplyMe(m){
 var w=document.getElementById('who');
 if(w){if(m&&m.user){w.innerHTML='signed in as <b></b>'+(m.sign_out?' · <a href="'+m.sign_out+'" id="signout">sign out</a>':'');w.querySelector('b').textContent=m.user;var so=document.getElementById('signout');if(so)so.onclick=function(e){e.preventDefault();if(m.sign_out==='/logout'){var f=document.createElement('form');f.method='post';f.action='/logout';document.body.appendChild(f);f.submit()}else location.href=m.sign_out}}else{w.innerHTML='<a href="'+((m&&m.sign_in)||'/login')+'">sign in</a>'}}
 var a=document.getElementById('adminslot');
 if(a)a.innerHTML=(m&&m.admin)?'<a class="pill" href="/admin">admin</a>':'';
}
try{var _m=localStorage.getItem('bifme');if(_m)bifApplyMe(JSON.parse(_m));}catch(e){}
fetch('/api/me').then(function(r){return r.json()}).then(function(m){
 bifApplyMe(m);
 try{localStorage.setItem('bifme',JSON.stringify({user:(m&&m.user)||null,admin:!!(m&&m.admin),sign_in:m&&m.sign_in,sign_out:m&&m.sign_out}));}catch(e){}
}).catch(function(){});
// Relative "x ago" for activity rows. data-ts = event epoch secs; optional
// data-ref = frozen reference epoch (archived seasons) — else live "now".
// The tooltip (title) shows the exact local date/time in the viewer's zone.
function bifAgo(sec){
 if(sec<0)sec=0;
 if(sec<60)return sec+'s ago';
 var m=Math.floor(sec/60);if(m<60)return m+'m ago';
 var h=Math.floor(m/60);if(h<24)return h+'h ago';
 var d=Math.floor(h/24);if(d<7)return d+'d ago';
 var w=Math.floor(d/7);return w+'w ago';
}
function bifTick(){
 var now=Math.floor(Date.now()/1000);
 var els=document.querySelectorAll('.ago[data-ts]');
 for(var i=0;i<els.length;i++){
  var el=els[i];var ts=parseInt(el.getAttribute('data-ts'),10);if(isNaN(ts))continue;
  var r=el.getAttribute('data-ref');var ref=r?parseInt(r,10):now;
  el.textContent=bifAgo(ref-ts);
  el.title=new Date(ts*1000).toLocaleString();
 }
}
document.addEventListener('DOMContentLoaded',function(){bifTick();setInterval(bifTick,30000);});
// Size the activity log to the viewport: measure how many rows fit, and if the
// current rows-per-page (rpp) differs, reload with the fitted value (preserving
// the top item's position). Runs only on paginated log pages (those have .pager).
function bifFitLog(){
 var pager=document.querySelector('.pager');var feed=document.querySelector('.feed');
 if(!pager||!feed)return;
 var rows=feed.querySelectorAll('.ev');if(rows.length<1)return;
 var stride=rows.length>=2
  ?rows[1].getBoundingClientRect().top-rows[0].getBoundingClientRect().top
  :rows[0].getBoundingClientRect().height+6;
 if(!stride||stride<1)return;
 var avail=window.innerHeight-feed.getBoundingClientRect().top-pager.getBoundingClientRect().height-24;
 var fit=Math.floor(avail/stride);if(fit<5)fit=5;if(fit>300)fit=300;
 var url=new URL(window.location.href);
 // Absent rpp means the server used its default (25); compare against that so a
 // viewport that already fits the default doesn't trigger a gratuitous reload.
 var curR=parseInt(url.searchParams.get('rpp')||'25',10);
 if(curR===fit)return;
 var curPage=parseInt(url.searchParams.get('page')||'1',10);
 var topIndex=(curPage-1)*curR;
 url.searchParams.set('rpp',fit);url.searchParams.set('page',Math.floor(topIndex/fit)+1);
 window.location.replace(url.toString());
}
window.addEventListener('load',bifFitLog);
// Activity-log filter: show the absolute from/to as the viewer's local time but
// submit epoch seconds (hidden fields), so time filtering is timezone-correct.
function bifFilterForm(){
 var form=document.getElementById('logfilter');if(!form)return;
 function toDTL(ep){var d=new Date(ep*1000),p=function(n){return String(n).padStart(2,'0')};
  return d.getFullYear()+'-'+p(d.getMonth()+1)+'-'+p(d.getDate())+'T'+p(d.getHours())+':'+p(d.getMinutes());}
 var sTs=document.getElementById('since_ts'),uTs=document.getElementById('until_ts'),
     sDt=document.getElementById('since_dt'),uDt=document.getElementById('until_dt');
 if(sTs.value)sDt.value=toDTL(parseInt(sTs.value,10));
 if(uTs.value)uDt.value=toDTL(parseInt(uTs.value,10));
 form.addEventListener('submit',function(){
  sTs.value=sDt.value?Math.floor(new Date(sDt.value).getTime()/1000):'';
  uTs.value=uDt.value?Math.floor(new Date(uDt.value).getTime()/1000):'';
 });
}
document.addEventListener('DOMContentLoaded',bifFilterForm);
// Custom sender/receiver autocomplete (replaces the unstylable native datalist
// popup). Reads the participant pool from window.bifPlayers.
function bifAutocomplete(){
 var players=window.bifPlayers||[];
 function esch(s){return s.replace(/[&<>]/g,function(c){return {'&':'&amp;','<':'&lt;','>':'&gt;'}[c];});}
 document.querySelectorAll('input[data-ac]').forEach(function(inp){
  var box=inp.parentNode.querySelector('.acbox');if(!box)return;var sel=-1,items=[];
  function hide(){box.style.display='none';box.innerHTML='';sel=-1;items=[];}
  function show(){
   var q=inp.value.trim().toLowerCase();
   items=players.filter(function(p){return p.toLowerCase().indexOf(q)>-1;}).slice(0,8);
   if(!items.length){hide();return;}
   box.innerHTML=items.map(function(p,i){return '<div class="aci'+(i===sel?' sel':'')+'">'+esch(p)+'</div>';}).join('');
   box.style.display='block';
   Array.prototype.forEach.call(box.children,function(d,i){
    d.addEventListener('mousedown',function(e){e.preventDefault();inp.value=items[i];hide();});});
  }
  inp.addEventListener('input',function(){sel=-1;show();});
  inp.addEventListener('focus',show);
  inp.addEventListener('blur',function(){setTimeout(hide,150);});
  inp.addEventListener('keydown',function(e){
   if(box.style.display!=='block')return;
   if(e.key==='ArrowDown'){sel=Math.min(sel+1,items.length-1);show();e.preventDefault();}
   else if(e.key==='ArrowUp'){sel=Math.max(sel-1,0);show();e.preventDefault();}
   else if(e.key==='Enter'&&sel>=0){inp.value=items[sel];hide();e.preventDefault();}
   else if(e.key==='Escape'){hide();}
  });
 });
}
document.addEventListener('DOMContentLoaded',bifAutocomplete);
// Live board: poll the fragment and swap the columns in place (no full reload),
// so slot stats/statuses and the activity feed update on their own. Only runs
// on the board (the only page with .boardcols).
function bifBoardLive(){
 var cols=document.querySelector('.boardcols');if(!cols)return;
 setInterval(function(){
  fetch('/board/fragment').then(function(r){return r.json()}).then(function(j){
   if(j&&j.body&&j.body.indexOf('boardgrid')>-1){
    cols.innerHTML=j.body;
    var c=document.getElementById('connline');if(c&&typeof j.conn==='string')c.innerHTML=j.conn;
    var sp=document.getElementById('seasonprog');if(sp&&typeof j.progress==='string')sp.innerHTML=j.progress;
    bifTick();
   }
  }).catch(function(){});
 },10000);
}
document.addEventListener('DOMContentLoaded',bifBoardLive);
// Live full-log: poll the current view (filters+page+rpp carried in the query)
// and swap just the feed+pager (#logbody), leaving the filter form untouched.
function bifLogLive(){
 var body=document.getElementById('logbody');if(!body||!body.hasAttribute('data-live'))return;
 setInterval(function(){
  fetch('/log/fragment'+window.location.search).then(function(r){return r.text()}).then(function(h){
   if(h!=null){body.innerHTML=h;bifTick();}
  }).catch(function(){});
 },10000);
}
document.addEventListener('DOMContentLoaded',bifLogLive);
</script>
<main>"#;

struct Group<'a> {
    ap_slot: i64,
    game: &'a str,
    rows: Vec<&'a SlotRow>,
    is_common: bool,
}

/// Group Bifrost slots under their shared AP slot, then order the groups
/// alphabetically by game name (case-insensitive) — the board/claim pages read
/// top-to-bottom by game. `slots` must arrive ordered by ap_slot (as db::slots
/// returns it) so consecutive rows of one AP slot land in the same group.
fn group_by_ap(slots: &[SlotRow]) -> Vec<Group<'_>> {
    let mut groups: Vec<Group> = Vec::new();
    for r in slots {
        match groups.last_mut() {
            Some(g) if g.ap_slot == r.ap_slot => {
                g.is_common |= r.is_common;
                g.rows.push(r);
            }
            _ => groups.push(Group {
                ap_slot: r.ap_slot,
                game: &r.game,
                rows: vec![r],
                is_common: r.is_common,
            }),
        }
    }
    groups.sort_by(|a, b| a.game.to_ascii_lowercase().cmp(&b.game.to_ascii_lowercase()));
    groups
}

/// Build the landing page's season roster grid from live data: one card per AP
/// slot (game + its player(s)), co-op slots listing every player with the AP
/// slot's own player first, ordered alphabetically by that lead player's name.
/// `ap_names` maps an AP slot to its connect name (from the live board) so we
/// can tell which co-op member is the slot's primary.
pub fn landing_roster(
    slots: &[SlotRow],
    ap_names: &std::collections::HashMap<i64, String>,
) -> String {
    if slots.is_empty() {
        return r#"<div class="empty" style="grid-column:1/-1">The roster fills in once the season's game is running.</div>"#
            .to_string();
    }
    struct Entry {
        key: String,
        game: String,
        who: String,
    }
    let mut entries: Vec<Entry> = group_by_ap(slots)
        .into_iter()
        .map(|g| {
            // The common game is shared — show it as everyone's, sorted first.
            if g.is_common {
                return Entry {
                    key: String::new(),
                    game: g.game.to_string(),
                    who: "Everyone".to_string(),
                };
            }
            let mut names: Vec<String> = g.rows.iter().map(|r| r.name.clone()).collect();
            names.sort_by_key(|n| n.to_ascii_lowercase());
            // Put the AP slot's own player (the primary) first, rest alphabetical.
            if let Some(lead) = ap_names.get(&g.ap_slot) {
                if let Some(pos) = names.iter().position(|n| n.eq_ignore_ascii_case(lead)) {
                    let p = names.remove(pos);
                    names.insert(0, p);
                }
            }
            let key = names
                .first()
                .map(|n| n.to_ascii_lowercase())
                .unwrap_or_default();
            Entry {
                key,
                game: g.game.to_string(),
                who: names.join(" & "),
            }
        })
        .collect();
    entries.sort_by(|a, b| a.key.cmp(&b.key));

    let mut out = String::new();
    for e in &entries {
        out.push_str(&format!(
            r#"<div class="slot"><div class="who">{}</div><div class="game">{}</div></div>"#,
            esc(&e.who),
            esc(&e.game)
        ));
    }
    out
}

/// Frontpage roster for the gap between seasons (no live game yet): the people
/// signed up for the *next* season, straight from the submission queue, so the
/// section fills in live as they register. Mirrors `landing_roster`'s card style
/// (who + game) so the section looks identical whether a season is live or not.
pub fn landing_signups(subs: &[SubmissionRow]) -> String {
    if subs.is_empty() {
        return r#"<div class="empty" style="grid-column:1/-1">No sign-ups yet — be the first to <a href="/signup">claim a game for next season</a>.</div>"#
            .to_string();
    }
    struct Entry {
        key: String,
        game: String,
        who: String,
    }
    let mut entries: Vec<Entry> = subs
        .iter()
        .map(|s| {
            // Primary player first, then any co-op seats, matching the live roster.
            let mut names = vec![s.slot_name.clone()];
            names.extend(
                s.coop_names
                    .split(',')
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_string),
            );
            Entry {
                key: s.slot_name.to_ascii_lowercase(),
                game: s.game.clone(),
                who: names.join(" & "),
            }
        })
        .collect();
    entries.sort_by(|a, b| a.key.cmp(&b.key));

    let mut out = String::new();
    for e in &entries {
        out.push_str(&format!(
            r#"<div class="slot"><div class="who">{}</div><div class="game">{}</div></div>"#,
            esc(&e.who),
            esc(&e.game)
        ));
    }
    out
}

/// A unix timestamp (seconds) as `D Mon YYYY` in UTC (Howard Hinnant's civil
/// calendar algorithm — no chrono dependency).
fn fmt_date(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    const MON: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!("{d} {} {year}", MON[((m - 1).clamp(0, 11)) as usize])
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A whole-unit "x ago" using the largest unit that fits: s, m, h, d, w.
fn fmt_ago(sec: i64) -> String {
    let s = sec.max(0);
    if s < 60 {
        return format!("{s}s ago");
    }
    let m = s / 60;
    if m < 60 {
        return format!("{m}m ago");
    }
    let h = m / 60;
    if h < 24 {
        return format!("{h}h ago");
    }
    let d = h / 24;
    if d < 7 {
        return format!("{d}d ago");
    }
    format!("{}w ago", d / 7)
}

/// (css class, short label) for an activity kind.
fn ev_kind(kind: &str) -> (&'static str, &'static str) {
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
fn ev_text_html(r: &crate::db::ActivityRow) -> String {
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
fn class_legend() -> String {
    let mut s = String::from(r#"<div class="legend">"#);
    for c in ["progression", "useful", "trap", "filler"] {
        s.push_str(&format!(
            r#"<span class="lg ic-{c}"><span class="dotc"></span>{c}</span>"#
        ));
    }
    s.push_str("</div>");
    s
}

fn render_feed(out: &mut String, rows: &[crate::db::ActivityRow], ref_ts: Option<i64>) {
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

/// The frontpage "Past seasons" block — a card per archived season linking to
/// its detail page. Empty string when there are none (so the section hides).
pub fn past_seasons_section(seasons: &[(String, String, i64)]) -> String {
    if seasons.is_empty() {
        return String::new();
    }
    let mut out = String::from(r#"<h2 class="sec">Past seasons</h2><div class="grid roster">"#);
    for (seed, name, ts) in seasons {
        out.push_str(&format!(
            r#"<a class="slot cardlink" href="/season/{}"><div class="game">{}</div><div class="who">{}</div></a>"#,
            esc(seed),
            esc(name),
            fmt_date(*ts),
        ));
    }
    out.push_str("</div>");
    out
}

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
            &format!("/season/{}", esc(&s.seed)),
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

/// Board-only overrides: let the column fill the viewport (header to match) and
/// render the slot cards as a viewport-responsive grid (as many ~280px columns
/// as fit), so even ultrawides pack more games per row instead of the default
/// two. The activity feed keeps its own readable width (`.feed` max-width), so
/// widening the grid doesn't stretch the log's item↔timestamp gap.
const BOARD_STYLE: &str = r#"<style>
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
fn card_progress(sent: u64, world_size: u32) -> String {
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
) -> Html<String> {
    let mut out = page_head(None);
    out.push_str(BOARD_STYLE);
    out.push_str(&format!(
        r#"<h1>Live board</h1><div class="crumb"><a href="/">← Bifrost</a> · <span id="connline">{}</span></div>"#,
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
fn board_inner(
    out: &mut String,
    snap: &BoardSnapshot,
    slots: &[SlotRow],
    prios: &std::collections::HashSet<(i64, i64, i64)>,
    mc_addr: Option<&str>,
    activity: &[crate::db::ActivityRow],
    inlogic: &std::collections::HashMap<i64, i64>,
) {
    out.push_str(r#"<div class="boardgrid"><h2>Slots — tap one to claim, hint, and see its hints</h2>"#);
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
                r#"<a class="slot cardlink {cls}" href="/game/{}"><span class="st">{label}</span><div class="game">{}</div>
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

    out.push_str(r#"</div><aside class="boardlog"><h2 class="loghdr"><span>Activity</span><a href="/log">full log →</a></h2>"#);
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

/// The windowed page numbers for a classic pager: 1 2 3 … x-1 x x+1 … n-2 n-1 n.
/// `None` marks an ellipsis gap. Page 1 is newest, `n` is oldest.
fn page_numbers(cur: i64, n: i64) -> Vec<Option<i64>> {
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
fn activity_pager(base: &str, qs: &str, page: i64, pages: i64, rpp: i64) -> String {
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
        out.push_str(&format!(r#"<a class="pg pgnav" href="{}" title="newer">‹</a>"#, link(page - 1)));
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
        out.push_str(&format!(r#"<a class="pg pgnav" href="{}" title="older">›</a>"#, link(page + 1)));
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
fn filter_form(base: &str, f: &LogFilter) -> String {
    let mut out = format!(r#"<form class="logfilter" id="logfilter" method="get" action="{base}">"#);
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
        let ck = if f.classes_selected[i] { " checked" } else { "" };
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
fn render_log_body(
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
fn render_log_section(
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
const LOG_STYLE: &str = r#"<style>
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
        r#"<h1>Activity log</h1><div class="crumb"><a href="/board">← board</a> · item sends, goals, releases</div>"#,
    );
    render_log_section(&mut out, base, rows, page, pages, rpp, ref_ts, f, true);
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
                    r#"<form class="hintform" method="post" action="/hint">
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
const CLAIM_HINT_JS: &str = r#"<script>
document.querySelectorAll('form.hintform').forEach(function(f){
 f.addEventListener('submit',function(e){
  e.preventDefault();
  var inp=f.querySelector('input[name=item]'),btn=f.querySelector('button'),msg=f.nextElementSibling;
  var item=inp.value.trim();if(!item)return;
  btn.disabled=true;if(msg){msg.className='inlinemsg';msg.textContent='Hinting…';}
  fetch('/hint',{method:'POST',body:new URLSearchParams(new FormData(f))}).then(function(r){return r.json()}).then(function(j){
   btn.disabled=false;
   if(msg){msg.className='inlinemsg '+(j&&j.ok?'ok':'bad');msg.textContent=(j&&j.message)?j.message:(j&&j.ok?'Hinted.':'Hint failed.');}
   if(j&&j.ok)inp.value='';
  }).catch(function(){btn.disabled=false;if(msg){msg.className='inlinemsg bad';msg.textContent='Request failed.';}});
 });
});
</script>"#;

pub fn signup_page(subs: &[SubmissionRow], me: &str, is_admin: bool, open: bool) -> Html<String> {
    let mut b = page_head(Some(me));
    b.push_str(r#"<h1>Season signup</h1><div class="crumb"><a href="/">← Bifrost</a></div>"#);
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
    <div class="coopown">You: <b id="myname">your slot</b> <span class="by free">from your YAML — already in</span></div>
    <label class="chk"><input type="checkbox" id="coopchk"> Sharing this game between people (co-op, e.g. Don't Starve Together)</label>
    <div id="coopbox" hidden>
      <label class="hint">You're already included above — just list the <b>other</b> people sharing this game, comma-separated. They can claim their seat once the season is generated.<input type="text" name="coop" placeholder="e.g. Robin, Sam"></label>
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
     if(j.ok){chkmsg.className='inlinemsg ok';chkmsg.textContent='✓ '+(j.name||'slot')+' — '+(j.game||'?')+' rolled cleanly.';var mn=document.getElementById('myname');if(mn&&j.name)mn.textContent=j.name;next2.disabled=false;}
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
    minesweeper_on: bool,
    mc_on: bool,
    connect_pw: &str,
    running: bool,
    gen_msg: &str,
    subs: &[SubmissionRow],
    apworlds: &[String],
    acdata: &str,
    archived: Option<&str>,
    outputs: &[crate::generate::OutputFile],
    out_slots: &[SlotRow],
) -> Html<String> {
    let mut b = page_head(None);
    b.push_str(
        r#"<h1>Season admin</h1><div class="crumb"><a href="/board">← board</a> · admin · <a href="/admin/users">users &amp; links</a></div>"#,
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

    // Minesweeper minigame
    b.push_str(r#"<h2>Minesweeper</h2>"#);
    if minesweeper_on {
        b.push_str(r#"<div class="statline">The minesweeper minigame is <b>enabled</b> — players can farm bonus hints.</div>"#);
        b.push_str(&format!(
            r#"<div class="btnrow">{}</div>"#,
            action_btn(
                "/admin/minesweeper",
                &[("open", "0")],
                "Disable minesweeper",
                false,
                ""
            )
        ));
    } else {
        b.push_str(r#"<div class="statline">The minesweeper minigame is <b>disabled</b>.</div>"#);
        b.push_str(&format!(
            r#"<div class="btnrow">{}</div>"#,
            action_btn(
                "/admin/minesweeper",
                &[("open", "1")],
                "Enable minesweeper",
                false,
                ""
            )
        ));
    }

    // Common game (shared Minecraft server) — read at generation time to decide
    // whether the communal in-pool slot is included. Toggle it before generating.
    b.push_str(r#"<h2>Common game (Minecraft)</h2>"#);
    if mc_on {
        b.push_str(r#"<div class="statline">The shared Minecraft common game is <b>enabled</b> — the next season generated will include the communal slot.</div>"#);
        b.push_str(&format!(
            r#"<div class="btnrow">{}</div>"#,
            action_btn("/admin/mc", &[("open", "0")], "Disable common game", false, "")
        ));
    } else {
        b.push_str(r#"<div class="statline">The shared Minecraft common game is <b>disabled</b> — toggle this <b>before generating</b> to include it in the season.</div>"#);
        b.push_str(&format!(
            r#"<div class="btnrow">{}</div>"#,
            action_btn("/admin/mc", &[("open", "1")], "Enable common game", false, "")
        ));
    }

    // Connect password (what players use to join the server)
    b.push_str(r#"<h2>Connect password</h2>"#);
    b.push_str(&format!(
        r#"<div class="statline">Players join with <b>{}</b> — share it in Discord.</div>"#,
        esc(connect_pw)
    ));
    b.push_str(
        r#"<form class="console" method="post" action="/admin/connect-password" autocomplete="off" onsubmit="return confirm('Change the connect password? Bifrost will restart briefly to apply it.')"><input type="text" name="password" placeholder="new connect password" style="flex:1;min-width:0" autocomplete="off" required><button type="submit">Set</button></form>
<p class="warnnote">Letters, numbers, - or _ (3–64 chars). Sets it on the running server and in host.yaml; Bifrost restarts to reconnect.</p>"#,
    );

    // Generate
    b.push_str(r#"<h2>Generate</h2>"#);
    b.push_str(&format!(
        r#"<p class="empty"><b>Generate a season</b> stages the {} queued signup(s), rolls a new seed, maps slots + auto-claims, deploys it (the game restarts), and closes signups — use it for a fresh signup round.<br><b>Regenerate</b> re-rolls the <em>current</em> lineup: it reuses the staged YAMLs + installed apworlds, produces a new world + fresh save (applying the current host.yaml), and keeps everyone's claims. Signups are left as they are.</p>"#,
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
            r#"<div class="btnrow">{}{}</div>"#,
            action_btn(
                "/admin/generate",
                &[],
                "Generate the season",
                false,
                "Generate a NEW season from the signups now? This restarts the game and closes signups."
            ),
            action_btn(
                "/admin/regenerate",
                &[],
                "Regenerate (re-roll current)",
                false,
                "Re-roll the current lineup into a new world with a fresh save? Claims are kept; the game restarts."
            )
        ));
    }

    // Generated per-player files (extracted from the current seed) for handing out.
    b.push_str(r#"<h2>Player files</h2>"#);
    if outputs.is_empty() {
        b.push_str(r#"<div class="empty">No per-player files in the current seed — most games connect directly and produce none; only patch/ROM-based games and a few others (BG3, Minecraft, …) emit one. (Nothing here until a season is generated.)</div>"#);
    } else {
        b.push_str(r#"<p class="empty">Files the generator produced for specific slots — give each to the right player (Discord, etc.). The slot name and game are taken straight from the filename, so there's no guessing which goes where.</p>"#);
        for o in outputs {
            let link = format!(
                r#"<a class="ghost" href="/admin/output?file={}">download</a>"#,
                urlq(&o.member)
            );
            if o.is_spoiler {
                b.push_str(&format!(
                    r#"<div class="hintrow"><span><b>Spoiler</b> · <code>.{}</code> · {} · admin reference, not for players</span>{link}</div>"#,
                    esc(&o.ext),
                    human_size(o.size),
                ));
            } else {
                let game = out_slots
                    .iter()
                    .find(|s| s.name.eq_ignore_ascii_case(&o.label))
                    .map(|s| s.game.as_str())
                    .unwrap_or("—");
                b.push_str(&format!(
                    r#"<div class="hintrow"><span><b>{}</b> — {} · <code>.{}</code> · {}</span>{link}</div>"#,
                    esc(&o.label),
                    esc(game),
                    esc(&o.ext),
                    human_size(o.size),
                ));
            }
        }
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

    // Archive this season (capture for the Past Seasons page before reset)
    b.push_str(r#"<h2>Archive this season</h2>"#);
    b.push_str(r#"<p class="empty">Freeze this season for <b>Past Seasons</b> — the roster, each game's final standing (goaled + checks), and this season's minesweeper leaderboards — <b>before</b> you reset, while the data is still live. Re-archiving just refreshes the snapshot.</p>"#);
    if let Some(name) = archived {
        b.push_str(&format!(
            r#"<div class="statline">✓ Archived as <b>{}</b>. <a href="/season/{}">View</a></div>"#,
            esc(name),
            esc(seed.unwrap_or("")),
        ));
    }
    b.push_str(
        r#"<form class="console" method="post" action="/admin/archive-season" autocomplete="off"><input type="text" name="name" placeholder="Season name (e.g. Season Sept 2026)" style="flex:1;min-width:0"><button type="submit">Archive &amp; finalize season</button></form>
<p class="warnnote">Also run the host season bundle (raw multidata, save, YAMLs, apworlds + log → Nextcloud) before reset — that's the replayable archive this snapshot can't hold.</p>"#,
    );

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
    b.push_str(r#"<p class="warnnote">Full reset removes all apworlds, players, slots, the save, and the world, and closes signups — a clean slate for the next season. <b>Archive the season first</b> (above); past-season snapshots and minesweeper history are kept.</p>"#);
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
});
var hform=document.getElementById('hintform'),hstatus=document.getElementById('hintstatus');
var hback=hform.querySelector('input[name=back]').value,hbtn=hform.querySelector('button');
function refreshHints(){
 var el=document.getElementById('worldhints');
 [0,1200,2800].forEach(function(d){setTimeout(function(){
  if(el)fetch(hback+'/hints').then(function(r){return r.text()}).then(function(h){el.innerHTML=h;}).catch(function(){});
  // Refresh the search pool too: a name whose last copy was just hinted drops out.
  fetch(hback+'/items').then(function(r){return r.json()}).then(function(a){if(Array.isArray(a))ITEMS=a;}).catch(function(){});
 },d);});
}
hform.addEventListener('submit',function(e){
 e.preventDefault();
 var item=q.value.trim();if(!item)return;
 hits=[];box.innerHTML='';hbtn.disabled=true;hstatus.className='inlinemsg';hstatus.textContent='Hinting…';
 fetch('/hint',{method:'POST',body:new URLSearchParams(new FormData(hform))}).then(function(r){return r.json()}).then(function(j){
  hbtn.disabled=false;
  hstatus.className='inlinemsg '+(j&&j.ok?'ok':'bad');
  hstatus.textContent=(j&&j.message)?j.message:(j&&j.ok?'Hinted.':'Hint failed.');
  if(j&&j.ok){q.value='';refreshHints();var pe=document.getElementById('hintpts');if(pe&&j.points!=null)pe.textContent=j.points;var ce=document.getElementById('bonushints');if(ce&&j.credits!=null)ce.textContent=j.credits;}
 }).catch(function(){hbtn.disabled=false;hstatus.className='inlinemsg bad';hstatus.textContent='Request failed.';});
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
 fetch('/admin/console',{method:'POST',body:new URLSearchParams({cmd:cmd})}).then(function(r){return r.json()}).then(function(j){
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

/// Client requirements for the shared Minecraft common game — these MUST match
/// what's installed on the Minecraft LXC (120). Update all three together when
/// the common game's versions change.
const MC_GAME_VERSION: &str = "26.2"; // Minecraft Java Edition version
const MC_LOADER: &str = "NeoForge 26.2.0.90"; // mod loader build
// The AP mod jar name is `crate::generate::MC_MOD_JAR` (shared with the download
// route that serves it), so the displayed name and the served file can't drift.

/// The per-game page: claim/unclaim its players, hint with live item search,
/// and all hints in this world with a "prioritize" toggle.
#[allow(clippy::too_many_arguments)]
/// Game-page tab styling (Hints | Universal Tracker). Distinctive class names:
/// the shared stylesheet uses short utility names, so a generic class would
/// silently inherit unrelated rules (see the header note in this file).
const GAME_STYLE: &str = r#"<style>
#tab-log>h2{text-align:center}
#tab-log .feed,#tab-log .pager{margin-inline:auto}
.gamelogbar{margin:0 0 10px;text-align:center}
.gamelogbar input[type=search]{width:100%;max-width:420px;box-sizing:border-box;background:var(--bg);color:var(--fg);border:1px solid var(--bd);border-radius:7px;padding:8px 11px;font-size:13px;font-family:var(--font)}
.gamelogbar input[type=search]:focus{outline:none;border-color:var(--accent)}
.gtabs{display:flex;gap:2px;border-bottom:1px solid var(--bd);margin:22px 0 16px}
.gtab{background:none;color:var(--muted);border:0;border-bottom:2px solid transparent;border-radius:0;margin-bottom:-1px;padding:8px 15px;font-family:var(--font);font-size:14px;font-weight:600;cursor:pointer}
.gtab:hover{color:var(--fg)}
.gtab.on{color:var(--accent);border-bottom-color:var(--accent)}
.utmeta{font-family:var(--mono);font-size:12px;color:var(--muted);margin:0 0 12px}
.utlist{display:flex;flex-direction:column;gap:4px;max-height:70vh;overflow:auto}
.utloc{font-size:13px;line-height:1.35;padding:6px 10px;background:var(--panel);border:1px solid var(--bd);border-radius:7px}
.utnote{font-size:12px;color:var(--muted);margin-top:14px;font-style:italic}
#tab-pt>h2{text-align:center}
.ptmap{width:100vw;max-width:100vw;margin-left:calc(50% - 50vw);margin-right:calc(50% - 50vw);padding-inline:clamp(16px,3vw,48px);box-sizing:border-box;text-align:center}
.ptmapwrap{position:relative;display:inline-block;max-width:100%;vertical-align:top;margin:0 7px 16px}
.ptgroup .ptgh{text-align:center}
/* Zoom/pan viewport: clips the transformed stage; touch-action:none so a drag
   pans instead of scrolling the page. */
.ptview{position:relative;overflow:hidden;width:fit-content;max-width:100%;border-radius:4px;touch-action:none}
.ptstage{position:relative;transform-origin:0 0}
.ptview img,.ptmapwrap img{display:block;max-width:100%;height:auto;user-select:none;-webkit-user-drag:none}
.ptmk{position:absolute;border:1px solid rgba(0,0,0,.65);box-sizing:border-box;border-radius:2px}
.ptmk.d{transform:rotate(45deg)}
.ptform{margin:10px 0;display:flex;gap:8px;align-items:center;flex-wrap:wrap}
.ptform input[type=file]{color:var(--muted);font-size:12.5px;max-width:100%}
.ptform input[type=file]::file-selector-button{background:transparent;border:1px solid var(--bd);color:var(--muted);border-radius:6px;padding:5px 11px;margin-right:10px;font-family:var(--font);font-size:12px;font-weight:600;cursor:pointer}
.ptform input[type=file]::file-selector-button:hover{border-color:var(--accent);color:var(--accent)}
.ptmanage{margin-top:16px;text-align:center}.ptmanage>summary{cursor:pointer;color:var(--muted);font-size:13px}
.ptmanage .ptform{justify-content:center}
.ptnest{margin:4px 0}
.pttabs{display:inline-flex;flex-wrap:wrap;gap:2px;justify-content:center;border-bottom:1px solid var(--bd);margin:0 0 10px}
.pttab{background:none;color:var(--muted);border:0;border-bottom:2px solid transparent;border-radius:0;padding:5px 11px;font-family:var(--font);font-size:13px;font-weight:600;cursor:pointer;margin-bottom:-1px}
.pttab:hover{color:var(--fg)}.pttab.on{color:var(--accent);border-bottom-color:var(--accent)}
.ptgroup{margin:6px 0}.ptgh{font-family:var(--mono);font-size:11px;letter-spacing:.06em;text-transform:uppercase;color:var(--muted);margin:10px 0 4px}
.ptdock{display:flex;flex-direction:column;gap:14px}
.ptdockmid{display:flex;flex-direction:row;flex-wrap:wrap;gap:16px;justify-content:center;align-items:flex-start}
/* A bottom-docked strip (e.g. the item panels) gets a divider from the map
   above it. Shrink the strip to its content and center it so the divider line
   spans the item strip, not the full map width. */
.ptdockmid+.ptdock{border-top:1px solid var(--bd);padding-top:12px;align-self:center;width:fit-content;max-width:100%}
.ptarr{display:flex;gap:10px;flex-wrap:wrap;justify-content:center}
.ptgrid{display:inline-block;margin:4px 0}
.ptgrow{display:flex;gap:3px;justify-content:flex-start}
.ptcell{position:relative;display:inline-flex;width:34px;height:34px;align-items:center;justify-content:center}
.ptcell.empty{visibility:hidden}
.ptitem{width:100%;height:100%;object-fit:contain;image-rendering:pixelated}
.ptitem.ptoff{opacity:.25;filter:grayscale(.7)}
.ptov{position:absolute;right:1px;bottom:0;font-family:var(--mono);font-size:10px;font-weight:700;color:#fff;text-shadow:0 0 2px #000,0 0 2px #000,0 0 2px #000;pointer-events:none}
</style>"#;

/// Toggles the game page's tab panels (delegated click on the tab bar).
const GAME_TAB_JS: &str = r#"<script>
(function(){var bar=document.querySelector('.gtabs');if(!bar)return;
function activate(id){var btn=bar.querySelector('.gtab[data-tab="'+id+'"]');if(!btn)return;
bar.querySelectorAll('.gtab').forEach(function(x){x.classList.toggle('on',x===btn);});
document.querySelectorAll('.gtabpanel').forEach(function(p){p.hidden=(p.id!==id);});}
bar.addEventListener('click',function(e){var b=e.target.closest('.gtab');if(!b)return;activate(b.getAttribute('data-tab'));});
// Open the tab named in the URL hash (e.g. #tab-pt after a pack upload).
var h=(location.hash||'').slice(1),el=h&&document.getElementById(h);
if(el&&el.classList.contains('gtabpanel'))activate(h);
})();
// The game's Log tab: a type-to-filter search + a feed whose page size is
// fitted to the viewport, with live refresh of the current page while open.
// Pager links (carrying the active search + rpp) are intercepted to swap in
// place. The first page is server-rendered; JS refits on show/resize.
(function(){var host=document.getElementById('gamelog');if(!host)return;
var slot=host.getAttribute('data-slot'),panel=document.getElementById('tab-log'),
    search=document.getElementById('gamelogq'),cur=null,tmr=null,rt=null,rpp=25;
function into(u){cur=u;fetch(u).then(function(r){return r.text();}).then(function(x){host.innerHTML=x;if(window.bifTick)bifTick();}).catch(function(){});}
function fit(){ // how many rows fit below the feed's top, within sane bounds
 var top=host.getBoundingClientRect().top,row=30,ev=host.querySelector('.ev');
 if(ev){var h=ev.getBoundingClientRect().height;if(h>0)row=h+6;}
 var n=Math.floor((window.innerHeight-top-64)/row);
 if(n<8)n=8;if(n>200)n=200;return n;}
function reload(page){rpp=fit();var q=search?search.value.trim():'';
 into('/game/'+slot+'/log/fragment?page='+(page||1)+'&rpp='+rpp+(q?('&q='+encodeURIComponent(q)):''));}
host.addEventListener('click',function(e){var a=e.target.closest('a.pg');if(!a)return;e.preventDefault();into(a.getAttribute('href'));});
if(search){search.addEventListener('input',function(){clearTimeout(tmr);tmr=setTimeout(function(){reload(1);},200);});}
var bar=document.querySelector('.gtabs');
if(bar){bar.addEventListener('click',function(e){var b=e.target.closest('.gtab');if(b&&b.getAttribute('data-tab')==='tab-log')reload(1);});}
window.addEventListener('resize',function(){if(panel&&panel.hidden)return;clearTimeout(rt);rt=setTimeout(function(){reload(1);},250);});
setInterval(function(){if(panel&&!panel.hidden&&cur)into(cur);},12000);
})();
</script>"#;

/// Tab bar + the opening tag of the (default-visible) Hints panel. The caller
/// closes the panel, then appends `inlogic_panel(...)` and `GAME_TAB_JS`.
fn tabs_open() -> &'static str {
    r#"<div class="gtabs"><button type="button" class="gtab on" data-tab="tab-hints">Hints</button><button type="button" class="gtab" data-tab="tab-ut">Universal Tracker</button><button type="button" class="gtab" data-tab="tab-pt">PopTracker</button><button type="button" class="gtab" data-tab="tab-log">Log</button></div><div class="gtabpanel" id="tab-hints">"#
}

/// Script that fills the PopTracker tab: fetch the computed tracker state and
/// draw each map image with accessibility-coloured markers at their pixel
/// coordinates (PopTracker palette: green available, yellow glitch, blue
/// viewable, red blocked, grey collected; mixed locations split diagonally),
/// polling while the tab is open.
const PT_JS: &str = r#"<script>
(function(){
 var host=document.getElementById('ptmap'); if(!host) return;
 var slot=host.getAttribute('data-slot'); var st={}; var ver='';
 // PopTracker's exact default marker colors, keyed by accessibility level.
 var PT_COL={reachable:'#20ff20',glitch:'#ffff20',inspect:'#3040ff',none:'#cf1010',done:'#3f3f3f',partial:'#ff9f20'};
 // Rank so a split shows the best state top-left, worst bottom-right.
 var PT_RANK={reachable:4,glitch:3,inspect:2,none:1};
 function secLevel(s){ return s.checked?'done':(s.level||(s.accessible?'reachable':'none')); }
 function secColor(s){ return PT_COL[secLevel(s)]||PT_COL.none; }
 function secWord(s){ var l=secLevel(s);
  return l==='done'?'collected':l==='reachable'?'available':l==='glitch'?'available (glitch)':l==='inspect'?'viewable only':'blocked'; }
 // Background for a marker. Collected checks drop out (as in PopTracker); the
 // color reflects the checks that remain:
 //   all collected        -> grey
 //   one remaining state   -> that solid color
 //   two remaining states  -> diagonal split, bottom-left to top-right
 //   three or more states  -> solid orange "partial"
 function markerBg(k){
  var secs=(k.sections||[]);
  if(!secs.length){ return k.checked?PT_COL.done:(k.accessible?PT_COL.reachable:PT_COL.none); }
  var rem=secs.filter(function(s){return !s.checked;});
  if(!rem.length) return PT_COL.done;
  var cols=rem.map(secColor).filter(function(c,i,a){return a.indexOf(c)===i;});
  if(cols.length===1) return cols[0];
  if(cols.length>=3) return PT_COL.partial;
  cols.sort(function(a,b){ return (ptRankOf(b)-ptRankOf(a)); });
  // 135deg puts the dividing line bottom-left -> top-right; first color top-left.
  return 'linear-gradient(135deg,'+cols[0]+' 50%,'+cols[1]+' 50%)';
 }
 function ptRankOf(hex){ for(var k in PT_COL){ if(PT_COL[k]===hex&&PT_RANK[k]) return PT_RANK[k]; } return 0; }
 // Tooltip: the location name, then each check's name with its cleared/total
 // count and state (PopTracker shows every section a location holds).
 function markerTitle(k){
  var secs=(k.sections||[]);
  if(secs.length<=1){
   var s0=secs[0];
   var word=s0?secWord(s0):(k.checked?'collected':(k.accessible?'available':'blocked'));
   if(s0&&s0.item_count>1){ return k.location+' ('+word+')\n'+s0.cleared+'/'+s0.item_count+' checks'; }
   return k.location+' ('+word+')';
  }
  var lines=[k.location];
  secs.forEach(function(s){ lines.push('• '+(s.name||'(section)')+': '+s.cleared+'/'+s.item_count+' ('+secWord(s)+')'); });
  return lines.join('\n');
 }
 function assetUrl(f){ return '/game/'+slot+'/poptracker/asset?file='+encodeURIComponent(f)+(ver?('&v='+ver):''); }
 function drawMap(mapName){
  var m=(st.maps||[]).filter(function(x){return x.name===mapName;})[0];
  var wrap=document.createElement('div'); wrap.className='ptmapwrap';
  if(!m){ wrap.textContent='(map '+mapName+' missing)'; return wrap; }
  // A clipping viewport (.ptview) holds a transformable stage (.ptstage) with
  // the image + markers; scroll zooms toward the cursor, drag pans (like native
  // PopTracker). Markers are % of the stage, so they ride the transform.
  var view=document.createElement('div'); view.className='ptview';
  var stage=document.createElement('div'); stage.className='ptstage';
  view.appendChild(stage); wrap.appendChild(view);
  var img=document.createElement('img'); img.loading='lazy'; img.draggable=false;
  img.src=assetUrl(m.img); stage.appendChild(img);
  var sz=m.location_size||16; var spans=[];
  (st.markers||[]).filter(function(k){return k.map===mapName;}).forEach(function(k){
   var s=k.size||sz; var mk=document.createElement('span');
   mk.className='ptmk'+(((k.shape||m.location_shape)==='diamond')?' d':'');
   mk.dataset.key=k.location+'@'+k.map; mk.dataset.loc=k.location;
   mk.title=markerTitle(k);
   mk.style.background=markerBg(k);
   mk._k=k; mk._s=s; spans.push(mk); stage.appendChild(mk);
  });
  // Position markers as PERCENTAGES of the image's natural size, so they scale
  // with the responsive (max-width:100%) image on any screen width.
  function place(){ var W=img.naturalWidth||1, H=img.naturalHeight||1;
   spans.forEach(function(mk){ var k=mk._k, s=mk._s;
    mk.style.left=((k.x-s/2)/W*100)+'%'; mk.style.top=((k.y-s/2)/H*100)+'%';
    mk.style.width=(s/W*100)+'%'; mk.style.height=(s/H*100)+'%';
   });
  }
  if(img.complete && img.naturalWidth) place(); else img.addEventListener('load',place);
  attachZoomPan(view,stage);
  return wrap;
 }
 // Scroll-to-zoom + drag-to-pan (mouse) and pinch/drag (touch) on one map view.
 function attachZoomPan(view,stage){
  var scale=1,tx=0,ty=0,MAX=8;
  function apply(){ stage.style.transform='translate('+tx+'px,'+ty+'px) scale('+scale+')';
   view.style.cursor=scale>1?'grab':''; view.classList.toggle('zoomed',scale>1); }
  function clamp(){ var r=view.getBoundingClientRect(), W=r.width, H=r.height;
   var minx=W*(1-scale), miny=H*(1-scale);
   tx=Math.min(0,Math.max(minx,tx)); ty=Math.min(0,Math.max(miny,ty));
   if(scale<=1){ tx=0; ty=0; } }
  function zoomAt(px,py,factor){ var ns=Math.min(MAX,Math.max(1,scale*factor));
   if(ns===scale) return; var cx=(px-tx)/scale, cy=(py-ty)/scale;
   scale=ns; tx=px-cx*scale; ty=py-cy*scale; clamp(); apply(); }
  view.addEventListener('wheel',function(e){ e.preventDefault(); var r=view.getBoundingClientRect();
   zoomAt(e.clientX-r.left, e.clientY-r.top, e.deltaY<0?1.2:1/1.2); }, {passive:false});
  view.addEventListener('dblclick',function(e){ e.preventDefault(); scale=1; tx=0; ty=0; apply(); });
  var pts={}, last=null, pinch=0;
  view.addEventListener('pointerdown',function(e){ pts[e.pointerId]={x:e.clientX,y:e.clientY};
   try{ view.setPointerCapture(e.pointerId); }catch(_){}
   if(Object.keys(pts).length===1){ last={x:e.clientX,y:e.clientY}; if(scale>1) view.style.cursor='grabbing'; } });
  view.addEventListener('pointermove',function(e){ if(!pts[e.pointerId]) return;
   pts[e.pointerId]={x:e.clientX,y:e.clientY}; var ids=Object.keys(pts);
   if(ids.length>=2){ var a=pts[ids[0]], b=pts[ids[1]], d=Math.hypot(a.x-b.x,a.y-b.y);
    var r=view.getBoundingClientRect(); if(pinch) zoomAt((a.x+b.x)/2-r.left,(a.y+b.y)/2-r.top,d/pinch);
    pinch=d; last=null; return; }
   if(last){ if(scale>1){ tx+=e.clientX-last.x; ty+=e.clientY-last.y; clamp(); apply(); }
    last={x:e.clientX,y:e.clientY}; } });
  function up(e){ delete pts[e.pointerId]; try{ view.releasePointerCapture(e.pointerId); }catch(_){}
   if(Object.keys(pts).length<2) pinch=0;
   if(!Object.keys(pts).length){ last=null; view.style.cursor=scale>1?'grab':''; } }
  view.addEventListener('pointerup',up); view.addEventListener('pointercancel',up);
  apply();
 }
 var itemsByCode={};
 function buildIndex(){ itemsByCode={}; (st.items||[]).forEach(function(it){ (it.codes||[]).forEach(function(c){ if(c) itemsByCode[(''+c).toLowerCase()]=it; }); }); }
 function itemCell(code){
  var cell=document.createElement('span'); cell.className='ptcell';
  if(!code){ cell.classList.add('empty'); return cell; }
  var it=itemsByCode[(''+code).toLowerCase()];
  if(it&&it.img){
   var im=document.createElement('img'); im.className='ptitem'+(it.active?'':' ptoff'); im.dataset.code=code; im.src=assetUrl(it.img); im.alt=it.name||code;
   cell.title=it.name||code; cell.appendChild(im);
   if(it.overlay){ var o=document.createElement('span'); o.className='ptov'; o.textContent=it.overlay; cell.appendChild(o); }
  }
  return cell;
 }
 function appendContent(parent,c){ if(c==null) return; if(Array.isArray(c)){ c.forEach(function(x){ parent.appendChild(render(x)); }); } else { parent.appendChild(render(c)); } }
 function render(n){
  if(n==null) return document.createComment('');
  if(Array.isArray(n)){ var f=document.createElement('div'); n.forEach(function(c){f.appendChild(render(c));}); return f; }
  if(typeof n!=='object') return document.createComment('');
  var t=n.type;
  if(t==='map'){ var d=document.createElement('div'); (n.maps||[]).forEach(function(mn){d.appendChild(drawMap(mn));}); return d; }
  if(t==='tabbed'){
   var box=document.createElement('div'); box.className='ptnest';
   var bar=document.createElement('div'); bar.className='pttabs';
   var body=document.createElement('div'); box.appendChild(bar); box.appendChild(body);
   (n.tabs||[]).forEach(function(tab,i){
    var btn=document.createElement('button'); btn.type='button'; btn.className='pttab'+(i===0?' on':''); btn.textContent=tab.title||('Tab '+(i+1));
    var panel=render(tab.content); panel.hidden=(i!==0);
    btn.addEventListener('click',function(){
     bar.querySelectorAll('.pttab').forEach(function(x){x.classList.remove('on');}); btn.classList.add('on');
     Array.prototype.forEach.call(body.children,function(p){p.hidden=true;}); panel.hidden=false;
    });
    bar.appendChild(btn); body.appendChild(panel);
   });
   return box;
  }
  if(t==='itemgrid'){ var g=document.createElement('div'); g.className='ptgrid';
   (n.rows||[]).forEach(function(row){ var r=document.createElement('div'); r.className='ptgrow'; (row||[]).forEach(function(code){ r.appendChild(itemCell(code)); }); g.appendChild(r); });
   return g;
  }
  if(t==='item'){ var ic=document.createElement('div'); ic.className='ptgrow'; ic.appendChild(itemCell(n.item||n.code||'')); return ic; }
  if(t==='array'){ var a=document.createElement('div'); a.className='ptarr';
   a.style.flexDirection=(n.orientation==='horizontal'?'row':'column'); a.style.alignItems=(n.orientation==='horizontal'?'flex-start':'center');
   appendContent(a,n.content); return a;
  }
  if(t==='group'){ var gr=document.createElement('div'); gr.className='ptgroup';
   if(n.header){ var h=document.createElement('div'); h.className='ptgh'; h.textContent=n.header; gr.appendChild(h); }
   appendContent(gr,n.content); return gr;
  }
  // PopTracker dock: children carry a `dock` edge (top/bottom/left/right) and
  // undocked children fill the center. We honor it so docked panels land on
  // their edge — e.g. Stardew's item grids are dock:"bottom", so they belong
  // under the map, not above it (array order alone would float them to the top).
  if(t==='dock'){
   var kids=[]; (function col(c){ if(c==null) return; if(Array.isArray(c)) c.forEach(col); else kids.push(c); })(n.content);
   var b={top:[],bottom:[],left:[],right:[],center:[]};
   kids.forEach(function(k){ (b[(k&&k.dock)]||b.center).push(k); });
   var box=document.createElement('div'); box.className='ptdock';
   b.top.forEach(function(k){ box.appendChild(render(k)); });
   if(b.left.length||b.center.length||b.right.length){
    var mid=document.createElement('div'); mid.className='ptdockmid';
    b.left.concat(b.center,b.right).forEach(function(k){ mid.appendChild(render(k)); });
    box.appendChild(mid);
   }
   b.bottom.forEach(function(k){ box.appendChild(render(k)); });
   return box;
  }
  if(n.content!==undefined){ var cc=document.createElement('div'); appendContent(cc,n.content); return cc; }
  return document.createComment(t||'');
 }
 function update(){
  var mm={}; (st.markers||[]).forEach(function(k){mm[k.location+'@'+k.map]=k;});
  host.querySelectorAll('.ptmk').forEach(function(mk){ var k=mm[mk.dataset.key]; if(k){ mk.style.background=markerBg(k); mk.title=markerTitle(k); } });
  host.querySelectorAll('.ptitem').forEach(function(im){ var it=itemsByCode[im.dataset.code]; if(!it) return;
   im.className='ptitem'+(it.active?'':' ptoff'); if(it.img) im.src=assetUrl(it.img);
   var ov=im.parentNode.querySelector('.ptov');
   if(it.overlay){ if(!ov){ ov=document.createElement('span'); ov.className='ptov'; im.parentNode.appendChild(ov); } ov.textContent=it.overlay; }
   else if(ov){ ov.remove(); }
  });
 }
 function draw(d){
  if(!d||!d.pack){ host.textContent='No pack.'; return; }
  if(d.error){ host.textContent='Tracker error: '+d.error; return; }
  ver=(d.version!=null?(''+d.version):''); st=d.state||{}; buildIndex();
  if(host.dataset.built){ update(); return; }
  host.innerHTML='';
  if(st.layout){ host.appendChild(render(st.layout)); }
  else if(st.maps&&st.maps.length){ st.maps.forEach(function(m){ host.appendChild(drawMap(m.name)); }); }
  else { host.textContent='This pack has no maps to show.'; return; }
  host.dataset.built='1';
 }
 function load(){ fetch('/game/'+slot+'/poptracker/state').then(function(r){return r.json();}).then(draw).catch(function(){}); }
 var panel=document.getElementById('tab-pt'); var bar=document.querySelector('.gtabs');
 if(bar){ bar.addEventListener('click',function(e){var b=e.target.closest('.gtab'); if(b&&b.getAttribute('data-tab')==='tab-pt') load();}); }
 load();
 setInterval(function(){ if(panel&&!panel.hidden) load(); },12000);
})();
</script>"#;

/// The PopTracker tab panel: the map host (filled by `PT_JS`) + upload/manage
/// controls for a claimant or admin, or a prompt/placeholder otherwise.
fn poptracker_panel(ap_slot: i64, has_pack: bool, can_manage: bool) -> String {
    let mut s = String::from(r#"<div class="gtabpanel" id="tab-pt" hidden><h2>PopTracker</h2>"#);
    if has_pack {
        s.push_str(&format!(
            r#"<div class="ptmap" id="ptmap" data-slot="{ap_slot}"><p class="empty">Loading tracker…</p></div>"#
        ));
        if can_manage {
            s.push_str(&format!(
                r#"<details class="ptmanage"><summary>Manage pack</summary>
<form method="post" action="/game/{ap_slot}/poptracker" enctype="multipart/form-data" class="ptform">
<input type="file" name="pack" accept=".zip,application/zip" required><button type="submit">Replace pack</button></form>
<form method="post" action="/game/{ap_slot}/poptracker/remove" onsubmit="return confirm('Remove this pack?')"><button type="submit" class="ghost">Remove pack</button></form></details>"#
            ));
        }
        s.push_str(PT_JS);
    } else if can_manage {
        s.push_str(&format!(
            r#"<p class="empty">Upload a <a href="https://github.com/black-sliver/PopTracker" target="_blank" rel="noopener">PopTracker</a> pack (.zip) for this game to get its map here, auto-tracked from the server — no setup for anyone viewing. The pack must support Archipelago auto-tracking.</p>
<form method="post" action="/game/{ap_slot}/poptracker" enctype="multipart/form-data" class="ptform">
<input type="file" name="pack" accept=".zip,application/zip" required><button type="submit">Upload pack</button></form>"#
        ));
    } else {
        s.push_str(r#"<p class="empty">No PopTracker pack has been uploaded for this game yet.</p>"#);
    }
    s.push_str("</div>");
    s
}

/// Game Log tab page size: the server default (before the client fits it to the
/// viewport) and the clamp bounds the fitted value must stay within.
pub const GAME_LOG_RPP: i64 = 25;
pub const GAME_LOG_RPP_MIN: i64 = 8;
pub const GAME_LOG_RPP_MAX: i64 = 200;

/// Pages for a given total at `rpp` (at least 1).
pub fn game_log_pages(total: i64, rpp: i64) -> i64 {
    ((total + rpp - 1) / rpp.max(1)).max(1)
}

/// Inner HTML of a game's Log feed (the `#gamelog` contents): one page of rows
/// plus a numbered pager. Reused by the tab and its live-refresh fragment; the
/// pager links carry the active search `qs` + `rpp` and are intercepted
/// client-side so navigation swaps in place.
pub fn game_log_feed(
    ap_slot: i64,
    rows: &[crate::db::ActivityRow],
    page: i64,
    pages: i64,
    rpp: i64,
    qs: &str,
) -> String {
    let mut s = String::new();
    if rows.is_empty() {
        s.push_str(r#"<p class="empty">Nothing matches.</p>"#);
        return s;
    }
    render_feed(&mut s, rows, None);
    let base = format!("/game/{ap_slot}/log/fragment");
    s.push_str(&activity_pager(&base, qs, page, pages, rpp));
    s
}

/// The game page's "Log" tab: this slot's own activity (items it sent or
/// received, its goals/releases/collects), newest first, with a type-to-filter
/// search box and a viewport-fitted paginated feed.
fn log_panel(ap_slot: i64, rows: &[crate::db::ActivityRow], total: i64) -> String {
    let mut s = String::from(
        r#"<div class="gtabpanel" id="tab-log" hidden><h2>This game's activity</h2>
<div class="gamelogbar"><input type="search" id="gamelogq" placeholder="Search this game's activity…" autocomplete="off"></div>"#,
    );
    s.push_str(&format!(r#"<div id="gamelog" class="gamelog" data-slot="{ap_slot}">"#));
    s.push_str(&game_log_feed(ap_slot, rows, 1, game_log_pages(total, GAME_LOG_RPP), GAME_LOG_RPP, ""));
    s.push_str("</div></div>");
    s
}

/// The Universal Tracker tab panel, from the slot's cached in-logic result.
fn inlogic_panel(inlogic: Option<&crate::db::InLogic>) -> String {
    let mut s =
        String::from(r#"<div class="gtabpanel" id="tab-ut" hidden><h2>Available now</h2>"#);
    match inlogic {
        None => s.push_str(
            r#"<p class="empty">Universal Tracker hasn't mapped this slot yet — it recomputes a few seconds after the slot's items or checks change. Check back shortly.</p>"#,
        ),
        Some(il) => {
            let locs: Vec<String> = serde_json::from_str(&il.locations).unwrap_or_default();
            if locs.is_empty() && il.err.is_some() {
                let msg = match il.err.as_deref() {
                    Some("disabled") => "Universal Tracker is turned off for this game by its developers, so live availability tracking isn't available here.",
                    _ => "Couldn't compute available locations for this slot right now — it'll retry automatically.",
                };
                s.push_str(&format!(r#"<p class="empty">{msg}</p>"#));
            } else {
                let n = locs.len();
                s.push_str(&format!(
                    r#"<p class="utmeta"><b>{n}</b> location{} available right now · as of <span class="ago" data-ts="{}"></span></p>"#,
                    if n == 1 { "" } else { "s" },
                    il.ts,
                ));
                if locs.is_empty() {
                    s.push_str(r#"<p class="empty">Nothing new is available right now.</p>"#);
                } else {
                    s.push_str(r#"<div class="utlist">"#);
                    for l in &locs {
                        s.push_str(&format!(r#"<div class="utloc">{}</div>"#, esc(l)));
                    }
                    s.push_str("</div>");
                }
                s.push_str(r#"<p class="utnote">Computed by Universal Tracker from this slot's current items. Randomized options (entrance randomizer / weighted choices) can make it differ slightly from the server.</p>"#);
            }
        }
    }
    s.push_str("</div>");
    s
}

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
    credits: i64,
    can_hint_common: bool,
    mc_username: Option<&str>,
    mc_addr: Option<&str>,
    files: &[crate::generate::OutputFile],
    inlogic: Option<&crate::db::InLogic>,
    log: &[crate::db::ActivityRow],
    log_total: i64,
    pt_has_pack: bool,
    pt_can_manage: bool,
) -> Html<String> {
    let live = snap.ap.get(&ap_slot).cloned().unwrap_or_default();
    let back = format!("/game/{ap_slot}");
    let here: Vec<&SlotRow> = slots.iter().filter(|r| r.ap_slot == ap_slot).collect();
    let is_common = here.iter().any(|r| r.is_common);
    let mut out = page_head(Some(me));
    out.push_str(GAME_STYLE);
    // The UT and Log tab panels are the same for the common and regular slot
    // branches; build them once.
    let ut_html = inlogic_panel(inlogic);
    let log_html = log_panel(ap_slot, log, log_total);
    // "available now" count from the slot's Universal Tracker result (if any).
    let avail = inlogic
        .filter(|il| il.err.is_none())
        .and_then(|il| serde_json::from_str::<Vec<String>>(&il.locations).ok())
        .map(|v| format!(" · {} available", v.len()))
        .unwrap_or_default();
    out.push_str(&format!(
        r#"<h1>{}</h1><div class="crumb"><a href="/board">← board</a>{}</div>
<div class="meta">{} locations · sent {} · received {} · {} left{avail}</div>"#,
        esc(game),
        if is_admin { " · admin" } else { "" },
        live.world_size,
        live.sent,
        live.received,
        live.still_to_find,
    ));

    // Admin: mark this slot finished (registers its goal → collects/releases its
    // items per server settings). Hidden once goaled; /release stays separate.
    if is_admin {
        if live.status == 30 {
            out.push_str(r#"<div class="adminfin"><span class="finnote">✓ Marked finished (goaled).</span></div>"#);
        } else {
            out.push_str(&format!(
                r#"<form class="adminfin" method="post" action="/game/{ap_slot}/finish" onsubmit="return confirm('Mark this game as finished? It registers their goal (announces it) and collects/releases their items per the server settings. This can\'t be undone from here.')">
<input type="hidden" name="back" value="{back}">
<button type="submit" class="finish">Mark finished</button></form>"#
            ));
        }
    }

    // The common game: shared by everyone, no per-person claim. Show how to join
    // the Minecraft server instead of claim/hint controls.
    if is_common {
        out.push_str(r#"<h2>Common game</h2>"#);
        out.push_str(r#"<p class="empty">A shared Minecraft world, open to everyone this season — drop in when you're softlocked, taking a break, or done with your own game. Its checks feed the common pool, so playing here releases items to the rest of the group.</p>"#);
        // How to join: the client must match the server exactly, or
        // NeoForge refuses the connection. Address comes last as the final step.
        let addr_step = match mc_addr {
            Some(addr) => format!(
                r#"Set your Minecraft username below so you're whitelisted, then open <b>Multiplayer</b> and connect to <b>{}</b>."#,
                esc(addr)
            ),
            None => "Open <b>Multiplayer</b> and connect — ask an admin for the server address.".to_string(),
        };
        // Bare address for the Prism steps (the full sentence above is addr_step).
        let addr = mc_addr.map(esc).unwrap_or_else(|| "ask an admin".to_string());
        // Just the NeoForge version (MC_LOADER is "NeoForge 26.2.0.90") for the
        // Prism "Install Loader → NeoForge → <version>" step.
        let loader_ver = MC_LOADER.strip_prefix("NeoForge ").unwrap_or(MC_LOADER);
        out.push_str(&format!(
            r#"<h3>How to join</h3>
<ol class="mcjoin">
<li>Get <b>Minecraft: Java Edition {ver}</b> on a genuine Mojang/Microsoft account (the server runs online-mode).</li>
<li>Install <b>{loader}</b> — <a href="/game/mc/neoforge">download the installer</a>, run it, and it creates a launch profile set to the matching Minecraft version.</li>
<li>Drop the mod <code>{mod_jar}</code> — <a href="/game/mc/mod">download</a> — into that profile's <code>mods/</code> folder. It's the <b>exact</b> jar the server runs; NeoForge drops you on any version mismatch.</li>
<li>{addr_step}</li>
</ol>
<p class="statline">The official Minecraft launcher and Prism both fetch the right Java for you; only a bare manual setup needs you to install <b>Java 25</b> yourself.</p>
<details class="prism"><summary>Step-by-step with Prism Launcher (recommended)</summary>
<ol class="mcjoin">
<li>Install <b>Prism Launcher</b> from <b>prismlauncher.org</b> and open it.</li>
<li>Top-right accounts button → <b>Manage Accounts</b> → <b>Add Microsoft</b> and sign in (the server is online-mode, so a genuine account is required).</li>
<li><b>Add Instance</b> → pick Minecraft <b>{ver}</b>, then in the <b>Mod Loader</b> section below the version list choose <b>NeoForge</b> → <b>{loader_ver}</b> → create. Prism downloads the matching Java (25) itself — accept if it offers. <i>(Already made the instance? Instead: <b>Edit → Version → Install Loader</b> in the right panel → NeoForge → {loader_ver}.)</i></li>
<li>Select the instance → <b>Edit</b> → <b>Mods</b> → <b>Add file…</b> and choose <code>{mod_jar}</code> (grab it from the <a href="/game/mc/mod">download</a> above). Nothing else goes in there.</li>
<li><b>Launch</b> the instance → <b>Multiplayer</b> → <b>Add Server</b> → address <b>{addr}</b> → join.</li>
</ol></details>"#,
            ver = MC_GAME_VERSION,
            loader = MC_LOADER,
            loader_ver = loader_ver,
            mod_jar = crate::generate::MC_MOD_JAR,
            addr = addr,
        ));

        // Whitelist self-service: the server is whitelisted, so a player with a
        // claimed game this season registers the exact Minecraft username they'll
        // join on; the host-side `mc-whitelist-sync` applies it to the server
        // itself. Same "must own a slot" gate as hinting.
        out.push_str(r#"<h3>Your Minecraft username</h3>"#);
        if can_hint_common {
            let cur = mc_username.unwrap_or("");
            let status = if cur.is_empty() {
                String::new()
            } else {
                format!(r#"<p class="statline">Whitelisted as <b>{}</b>.</p>"#, esc(cur))
            };
            let btn = if cur.is_empty() { "Whitelist me" } else { "Update" };
            out.push_str(&format!(
                r#"<p class="empty">The server is whitelisted. Set the exact username of the Mojang/Microsoft account you'll join on — you'll be let in within a minute or so.</p>
{status}<form method="post" action="/mc/username" class="hintform">
<input type="hidden" name="back" value="{back}">
<input name="mc_username" value="{cur}" placeholder="YourMinecraftName" pattern="[A-Za-z0-9_]{{3,16}}" maxlength="16" autocomplete="off" required>
<button type="submit">{btn}</button></form>"#,
                status = status,
                back = esc(&back),
                cur = esc(cur),
                btn = btn,
            ));
        } else {
            out.push_str(
                r#"<p class="empty">The server is whitelisted. <a href="/claim">Claim a game</a> this season, then come back here to register your Minecraft username and get let in.</p>"#,
            );
        }

        // Tabs: everything hint-related goes in the Hints panel; the Universal
        // Tracker panel (built above) sits beside it. The join/whitelist info
        // above stays outside the tabs.
        out.push_str(tabs_open());

        // Communal hinting: only players with a claimed game this season may spend
        // the common game's OWN (shared) AP hint points — no personal/bonus points.
        if can_hint_common {
            if let Some(sid) = here.first().map(|r| r.id) {
                let items_json =
                    serde_json::to_string(item_names).unwrap_or_else(|_| "[]".into());
                let have = match points {
                    Some(p) => format!(
                        r#"The common game has <b id="hintpts">{p}</b> shared hint points. "#
                    ),
                    None => String::new(),
                };
                out.push_str(&format!(
                    r#"<h2>Send a hint</h2>
<p class="empty">{have}Each hint in this world costs <b>{cost}</b> points. Any player with a game this season can spend them — they're the common game's own, earned as the group checks its locations. Search the pool and hint one.</p>
<form method="post" action="/hint" class="hintform" id="hintform">
<input type="hidden" name="slot_id" value="{sid}"><input type="hidden" name="back" value="{back}">
<input id="hintq" name="item" placeholder="search the common game's items…" autocomplete="off" required>
<button type="submit">Hint</button></form>
<div id="hintmatches" class="matches"></div>
<div id="hintstatus" class="inlinemsg"></div>"#
                ));
                out.push_str("<script>\nvar ITEMS=");
                out.push_str(&items_json);
                out.push_str(";\n");
                out.push_str(HINT_JS);
                out.push_str("\n</script>");
            }
        } else {
            out.push_str(
                r#"<h2>Send a hint</h2><p class="empty">Hinting the common game is open to anyone with a claimed game this season — its shared hint points are reserved for players with a slot of their own. Claim a game on the board to join in.</p>"#,
            );
        }

        // Where the common game's items are. Admins get the prioritize toggles.
        out.push_str(
            r#"<h2>Hints for this game's items</h2><p class="empty" style="margin-top:-4px">Where the common game's items are — in its own or others' worlds.</p><div id="worldhints">"#,
        );
        out.push_str(&hint_rows(
            &live.hint_views,
            &live.found_items,
            prios,
            &back,
            is_admin,
        ));
        out.push_str("</div>"); // close #worldhints
        out.push_str("</div>"); // close #tab-hints
        out.push_str(&ut_html);
        out.push_str(&poptracker_panel(ap_slot, pt_has_pack, pt_can_manage));
        out.push_str(&log_html);
        out.push_str(GAME_TAB_JS);
        out.push_str("</main></body></html>");
        return Html(out);
    }

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

    // Generated files for this slot — shown only to a claimant of it (or admin);
    // the caller passes an empty list to everyone else. Downloaded through the
    // claimant-gated /game/{ap_slot}/output route (not the admin one).
    if !files.is_empty() {
        out.push_str(r#"<h2>Your game files</h2>"#);
        out.push_str(r#"<p class="empty">The generator produced these for your slot — download and use them with your client to play this seed.</p>"#);
        out.push_str(r#"<div class="grid">"#);
        for f in files {
            out.push_str(&format!(
                r#"<div class="slot idle"><div class="prow"><span class="who"><code>.{}</code> file · {}</span><a class="ghost" href="/game/{ap_slot}/output?file={}">download</a></div></div>"#,
                esc(&f.ext),
                human_size(f.size),
                urlq(&f.member),
            ));
        }
        out.push_str("</div>");
    }

    // Tabs: the hint form + per-item hint list live in the Hints panel; the
    // Universal Tracker panel (built above) sits beside it. Players/files above
    // stay outside the tabs.
    out.push_str(tabs_open());

    // Hinting — shown only to a player who claims a seat on this slot. Admins do
    // not get it here; they hint via the server console instead.
    let hint_slot = here
        .iter()
        .find(|r| r.claimed_by.as_deref() == Some(me))
        .map(|r| r.id);
    if let Some(sid) = hint_slot {
        let items_json = serde_json::to_string(item_names).unwrap_or_else(|_| "[]".into());
        let have = match points {
            Some(p) => format!(r#"You have <b id="hintpts">{p}</b> hint points. "#),
            None => String::new(),
        };
        let bonus = if credits > 0 {
            format!(
                r#" You also have <b id="bonushints">{credits}</b> bonus hint(s) — spent automatically when you're out of points."#
            )
        } else {
            String::new()
        };
        out.push_str(&format!(
            r#"<h2>Send a hint</h2>
<p class="empty">{have}Each hint in this world costs <b>{cost}</b> points.{bonus} Search your pool and hint one.</p>
<p class="empty"><a href="/minesweeper?back={back}">Play minesweeper to earn bonus hints →</a></p>
<form method="post" action="/hint" class="hintform" id="hintform">
<input type="hidden" name="slot_id" value="{sid}"><input type="hidden" name="back" value="{back}">
<input id="hintq" name="item" placeholder="search your items…" autocomplete="off" required>
<button type="submit">Hint</button></form>
<div id="hintmatches" class="matches"></div>
<div id="hintstatus" class="inlinemsg"></div>"#
        ));
        out.push_str("<script>\nvar ITEMS=");
        out.push_str(&items_json);
        out.push_str(";\n");
        out.push_str(HINT_JS);
        out.push_str("\n</script>");
    }

    // Hints for this game's items — where each of this slot's items is located.
    // Only a claimant of this game (or an admin) gets the prioritize toggles.
    // Wrapped in #worldhints so a successful hint can live-refresh it.
    let can_prioritize = hint_slot.is_some() || is_admin;
    out.push_str(
        r#"<h2>Hints for this game's items</h2><p class="empty" style="margin-top:-4px">Where this slot's items are — in its own or others' worlds.</p><div id="worldhints">"#,
    );
    out.push_str(&hint_rows(
        &live.hint_views,
        &live.found_items,
        prios,
        &back,
        can_prioritize,
    ));
    out.push_str("</div>"); // close #worldhints
    out.push_str("</div>"); // close #tab-hints
    out.push_str(&ut_html);
    out.push_str(&poptracker_panel(ap_slot, pt_has_pack, pt_can_manage));
    out.push_str(&log_html);
    out.push_str(GAME_TAB_JS);

    out.push_str("</main></body></html>");
    Html(out)
}

/// The rows of the per-game hint list ("where this slot's items are"), also
/// served as a fragment for live refresh after a hint.
pub fn hint_rows(
    views: &[crate::ap::HintView],
    found_items: &[crate::ap::HintView],
    prios: &std::collections::HashSet<(i64, i64, i64)>,
    back: &str,
    can_prioritize: bool,
) -> String {
    // Active list = hints whose item is not yet found (these carry the
    // prioritize toggle). The Found section = every item the slot has received
    // (`found_items`), rendered like a found hint; it's a superset of the found
    // hints. Before the savefile has been read `found_items` is empty, so fall
    // back to the found hints from the live tracker.
    let mut active: Vec<&crate::ap::HintView> = views.iter().filter(|h| !h.found).collect();
    let mut found: Vec<&crate::ap::HintView> = if found_items.is_empty() {
        views.iter().filter(|h| h.found).collect()
    } else {
        found_items.iter().collect()
    };
    if active.is_empty() && found.is_empty() {
        return r#"<div class="empty">No hints here yet.</div>"#.to_string();
    }

    // Sort by item name (the `text` starts with it), case-insensitively.
    let name_key = |h: &&crate::ap::HintView| {
        h.text
            .split_once(" @ ")
            .map_or(h.text.as_str(), |(item, _)| item)
            .to_ascii_lowercase()
    };
    let is_prio = |h: &&crate::ap::HintView| prios.contains(&(h.finding, h.location, h.item));
    // Active: prioritized first (false < true, so negate), then alphabetical.
    active.sort_by_key(|h| (!is_prio(h), name_key(h)));
    found.sort_by_key(name_key);

    let mut out = String::new();
    if active.is_empty() {
        out.push_str(r#"<div class="empty">No unfound hints.</div>"#);
    }
    for h in &active {
        let on = prios.contains(&(h.finding, h.location, h.item));
        if !can_prioritize {
            // Non-claimants see the hints but can't reprioritize them; a
            // prioritized one is still marked.
            let star = if on { " ★" } else { "" };
            out.push_str(&format!(
                r#"<div class="hintrow"><span>{}{star}</span></div>"#,
                esc(&h.text)
            ));
            continue;
        }
        let (cls, label) = if on {
            ("", "★ prioritized")
        } else {
            ("ghost", "☆ prioritize")
        };
        out.push_str(&format!(
            r#"<div class="hintrow"><span>{}</span><form method="post" action="/prioritize" style="display:inline">
<input type="hidden" name="finding" value="{}"><input type="hidden" name="location" value="{}">
<input type="hidden" name="item" value="{}"><input type="hidden" name="receiving" value="{}">
<input type="hidden" name="back" value="{}">
<button class="{cls}" type="submit">{label}</button></form></div>"#,
            esc(&h.text),
            h.finding,
            h.location,
            h.item,
            h.receiving,
            esc(back),
        ));
    }
    if !found.is_empty() {
        out.push_str(&format!(
            r#"<details class="foundhints"><summary>Found ({})</summary>"#,
            found.len()
        ));
        for h in &found {
            out.push_str(&format!(
                r#"<div class="hintrow found"><span>{}</span></div>"#,
                esc(&h.text)
            ));
        }
        out.push_str("</details>");
    }
    out
}

/// A duration in milliseconds as `M:SS`.
fn fmt_ms(ms: i64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// One labelled stats block (for a single board kind).
fn mine_stats_block(label: &str, st: &crate::db::MineStats) -> String {
    let time = |o: Option<i64>| o.map(fmt_ms).unwrap_or_else(|| "—".to_string());
    let rate = if st.total() == 0 {
        "—".to_string()
    } else {
        format!("{}%", st.success_pct())
    };
    format!(
        r#"<div class="msblock"><h4>{label}</h4>
<div class="mstat"><span>Completed</span><b>{}</b></div>
<div class="mstat"><span>Failures</span><b>{}</b></div>
<div class="mstat"><span>Success rate</span><b>{}</b></div>
<div class="mstat sep"><span>Best time</span><b>{}</b></div>
<div class="mstat"><span>Average time</span><b>{}</b></div>
<div class="mstat"><span>Worst time</span><b>{}</b></div></div>"#,
        st.wins,
        st.losses,
        rate,
        time(st.best_ms),
        time(st.avg_ms),
        time(st.worst_ms),
    )
}

/// The minesweeper leaderboards for one board kind: best time and best
/// completion rate (min-games gated), top N each. `min_games` is shown as a note.
pub fn mine_leaderboard_html(
    kind_label: &str,
    time: &[(String, i64)],
    rate: &[(String, i64, i64)],
    min_games: i64,
) -> String {
    let mut out = format!(r#"<h2 class="lbhead">{kind_label} leaderboard</h2>"#);

    out.push_str(r#"<h3 class="lbsub">Best time</h3>"#);
    if time.is_empty() {
        out.push_str(r#"<p class="empty">No cleared boards yet.</p>"#);
    } else {
        out.push_str(r#"<table class="lb"><thead><tr><th>#</th><th>Player</th><th>Time</th></tr></thead><tbody>"#);
        for (i, (sso, ms)) in time.iter().enumerate() {
            out.push_str(&format!(
                r#"<tr><td class="rank">{}</td><td>{}</td><td class="num">{}</td></tr>"#,
                i + 1,
                esc(sso),
                fmt_ms(*ms),
            ));
        }
        out.push_str("</tbody></table>");
    }

    out.push_str(&format!(
        r#"<h3 class="lbsub">Best completion rate <small>(min {min_games} games)</small></h3>"#
    ));
    if rate.is_empty() {
        out.push_str(&format!(
            r#"<p class="empty">No one has played {min_games} games yet.</p>"#
        ));
    } else {
        out.push_str(r#"<table class="lb"><thead><tr><th>#</th><th>Player</th><th>Rate</th><th>Games</th></tr></thead><tbody>"#);
        for (i, (sso, wins, total)) in rate.iter().enumerate() {
            let pct = if *total > 0 { wins * 100 / total } else { 0 };
            out.push_str(&format!(
                r#"<tr><td class="rank">{}</td><td>{}</td><td class="num">{}%</td><td class="num">{}</td></tr>"#,
                i + 1,
                esc(sso),
                pct,
                total,
            ));
        }
        out.push_str("</tbody></table>");
    }
    out
}

/// The stats panel body, per board kind — reused for the initial page and the
/// live refresh the reveal endpoint returns when a game ends.
pub fn mine_stats_html(standard: &crate::db::MineStats, expert: &crate::db::MineStats) -> String {
    format!(
        "{}{}",
        mine_stats_block("Standard", standard),
        mine_stats_block("Expert", expert)
    )
}

/// The minesweeper minigame page: a server-authoritative board. `free` is the
/// standalone minigame mode (from the home Minigames section) — no bonus hints,
/// all-time leaderboard/stats; otherwise it's the seasonal, board-linked mode.
pub fn minesweeper_page(
    enabled: bool,
    free: bool,
    credits: i64,
    standard: &crate::db::MineStats,
    expert: &crate::db::MineStats,
    back: &str,
) -> Html<String> {
    let mut b = page_head(None);
    // Freeplay (from the home minigames section) goes home; seasonal goes back to
    // wherever you entered from — the game page you opened minesweeper for, else
    // the board.
    let crumb = if free {
        r#"<div class="crumb"><a href="/">← Bifrost</a></div>"#.to_string()
    } else {
        let label = if back.starts_with("/game/") {
            "← back to game"
        } else {
            "← board"
        };
        format!(r#"<div class="crumb"><a href="{}">{label}</a></div>"#, esc(back))
    };
    b.push_str(&format!(r#"<h1>Minesweeper</h1>{crumb}"#));
    if !enabled {
        b.push_str(
            r#"<div class="banner bad">The minesweeper minigame is disabled right now.</div></main></body></html>"#,
        );
        return Html(b);
    }
    // Intro + stat scope differ by mode; freeplay drops the "Bonus hints:" tally.
    let intro = if free {
        "Just for fun — results count toward the all-time leaderboards (no bonus hints here). Every board is solvable by logic alone, no guessing needed, on both difficulties. Left-click a cell to reveal, right-click to flag. On a revealed number: left-click chord-reveals its neighbours once you've flagged that many mines, right-click flags all remaining neighbours when they must all be mines."
    } else {
        "Clear the board without hitting a mine to earn a bonus hint — worth 1 either way. Every board is solvable by logic alone, no guessing needed, on both difficulties. Left-click a cell to reveal, right-click to flag. On a revealed number: left-click chord-reveals its neighbours once you've flagged that many mines, right-click flags all remaining neighbours when they must all be mines. Bonus hints are spent automatically when you hint without enough points."
    };
    let statline = if free {
        r#"<div class="statline"><span id="minesleft"></span> · ⏱ <b id="mstimer">0:00</b></div>"#.to_string()
    } else {
        format!(r#"<div class="statline">Bonus hints: <b id="credits">{credits}</b> · <span id="minesleft"></span> · ⏱ <b id="mstimer">0:00</b></div>"#)
    };
    let statnote = if free {
        "All-time, across every season. Times are for cleared boards."
    } else {
        "This season. Times are for cleared boards."
    };
    b.push_str(&format!(
        r#"<p class="empty mineintro">{intro}</p>
<div class="mstabs">
  <button class="mstab active" data-tab="standard">Standard <small>16×16 · 40</small></button>
  <button class="mstab" data-tab="expert">Expert <small>30×16 · 99</small></button>
  <span class="mstabgap"></span>
  <button class="mstab" data-lb="standard">🏆 Standard</button>
  <button class="mstab" data-lb="expert">🏆 Expert</button>
</div>
<div id="playview">
  {statline}
  <div id="msg" class="inlinemsg"></div>
  <div class="minewrap">
    <div class="mineboard">
      <div class="msscroll"><div id="grid" class="msgrid"></div></div>
      <div class="btnrow"><button id="newgame" class="ghost" style="display:none">New game</button></div>
    </div>
    <aside class="minestats-panel">
      <h3>Your stats</h3>
      <div id="minestats" class="minestats">{stats}</div>
      <p class="mstatnote">{statnote}</p>
    </aside>
  </div>
</div>
<div id="lbview" class="lbview" style="display:none"></div>
<script>var FREE={free};</script>"#,
        stats = mine_stats_html(standard, expert),
    ));
    b.push_str(MINE_STYLE);
    b.push_str(MINE_JS);
    b.push_str("</main></body></html>");
    Html(b)
}

const MINE_STYLE: &str = r#"<style>
/* Widen this page (and match the header) so the full expert board fits beside
   the stats instead of scrolling inside the 900px default column. */
main{max-width:1180px}
header .hwrap{max-width:1180px}
.mineintro{max-width:860px}
.minewrap{display:flex;gap:18px;align-items:flex-start;flex-wrap:wrap;margin-top:14px}
.mineboard{min-width:0}
.minestats-panel{flex:0 1 220px;min-width:190px;background:var(--panel);border:1px solid var(--bd);border-radius:12px;padding:14px 16px}
.minestats-panel h3{margin:0 0 12px;font-size:12px;text-transform:uppercase;letter-spacing:.09em;color:var(--muted)}
.minestats{display:grid;gap:16px}
.msblock{display:grid;gap:9px}
.msblock h4{margin:0;font-size:11px;text-transform:uppercase;letter-spacing:.08em;color:var(--accent)}
.mstat{display:flex;justify-content:space-between;align-items:baseline;gap:16px;font-size:13.5px}
.mstat>span{color:var(--muted)}
.mstat>b{font-family:var(--mono);font-size:14px}
.mstat.sep{border-top:1px solid var(--bd);padding-top:9px;margin-top:2px}
.mstatnote{color:var(--muted);font-size:11px;margin:14px 0 0;line-height:1.4}
.btnrow{display:flex;gap:10px;flex-wrap:wrap}
.mstabs{display:flex;gap:6px;margin:0 0 12px;flex-wrap:wrap;align-items:center}
.mstab{background:transparent;border:1px solid var(--bd);border-radius:8px;color:var(--muted);font-weight:600;padding:7px 13px;cursor:pointer;font-size:13px}
.mstab small{font-weight:400;opacity:.7;margin-left:6px;font-size:11px}
.mstab:hover{color:var(--fg);border-color:var(--accent)}
.mstab.active{color:#0d1117;background:var(--accent);border-color:var(--accent)}
.mstabgap{width:1px;align-self:stretch;background:var(--bd);margin:2px 4px}
.lbview{margin-top:14px;max-width:560px}
.lbhead{font-size:15px;margin:0 0 6px}
.lbsub{font-size:12px;text-transform:uppercase;letter-spacing:.08em;color:var(--muted);margin:18px 0 8px}
.lbsub small{text-transform:none;letter-spacing:0;font-weight:400}
table.lb{width:100%;border-collapse:collapse;font-size:13.5px}
table.lb th{text-align:left;font-size:11px;text-transform:uppercase;letter-spacing:.06em;color:var(--muted);font-weight:600;padding:4px 10px;border-bottom:1px solid var(--bd)}
table.lb td{padding:6px 10px;border-bottom:1px solid var(--bd)}
table.lb td.rank{color:var(--muted);font-family:var(--mono);width:1%;white-space:nowrap}
table.lb td.num{font-family:var(--mono);text-align:right}
table.lb th:last-child{text-align:right}
table.lb tr:first-child td.rank{color:var(--accent);font-weight:700}
.msscroll{overflow:auto;max-width:100%;margin:0 0 10px}
.msgrid{display:grid;gap:2px;width:max-content}
.cell{width:28px;height:28px;padding:0;background:var(--panel);border:1px solid var(--bd);border-radius:4px;color:var(--fg);font-family:var(--mono);font-size:14px;font-weight:700;cursor:pointer;line-height:1;text-align:center}
.msgrid.exp .cell{width:24px;height:24px;font-size:12px;border-radius:3px}
.cell:hover:not(.open){border-color:var(--accent)}
.cell.open{background:var(--bg);cursor:default}
.cell.mine{background:var(--bad);color:#fff;border-color:var(--bad)}
.cell.flag{color:var(--warn)}
.cell[data-n="1"]{color:#5aa9e6}.cell[data-n="2"]{color:#28a56d}.cell[data-n="3"]{color:#e24d5f}
.cell[data-n="4"]{color:#c58af9}.cell[data-n="5"]{color:#d98324}.cell[data-n="6"]{color:#3fbfbf}
.cell[data-n="7"]{color:#e6e9ef}.cell[data-n="8"]{color:#98a3b6}
</style>"#;

const MINE_JS: &str = r#"<script>
(function(){
var W,H,minesN,over=false,flags=0,cells=[],activeKind='standard',view='board',lbKind='standard';
var grid=document.getElementById('grid'),msg=document.getElementById('msg'),
    minesleft=document.getElementById('minesleft'),creditsEl=document.getElementById('credits'),
    statsEl=document.getElementById('minestats'),timerEl=document.getElementById('mstimer'),
    newBtn=document.getElementById('newgame'),tabs=document.querySelectorAll('.mstab'),
    playview=document.getElementById('playview'),lbview=document.getElementById('lbview');
// Each difficulty is its own persistent board (keyed server-side by player+kind),
// so switching tabs keeps each game + timer alive. The "New game" button only
// shows once the current board ends (to replay the same difficulty).
function showNewBtn(s){if(newBtn)newBtn.style.display=s?'':'none';}
function updateTabUI(){tabs.forEach(function(t){
 var on=view==='board'?(t.getAttribute('data-tab')===activeKind):(t.getAttribute('data-lb')===lbKind);
 t.classList.toggle('active',on);
});}
// Show a season leaderboard tab (pausing the live board we're leaving).
function showLb(kind){
 if(view==='board'&&!over)pauseKind(activeKind);
 view='lb';lbKind=kind;playview.style.display='none';lbview.style.display='';updateTabUI();
 lbview.innerHTML='<p class="empty">Loading…</p>';
 fetch('/minesweeper/leaderboard?kind='+kind+FB).then(function(r){return r.text()}).then(function(h){
  if(view==='lb'&&lbKind===kind)lbview.innerHTML=h;
 }).catch(function(){lbview.innerHTML='<p class="empty">Could not load leaderboard.</p>';});
}
// Timer: mirrors the server's active-play clock (which excludes time while the
// tab is hidden/closed). baseMs is the last value the server reported; while
// running we add the wall-clock time since that sync.
var baseMs=0,running=false,syncAt=0;
function fmtTime(ms){var s=Math.max(0,Math.floor(ms/1000));return Math.floor(s/60)+':'+('0'+(s%60)).slice(-2);}
function renderTimer(){if(timerEl)timerEl.textContent=fmtTime(baseMs+(running?(Date.now()-syncAt):0));}
function setTimer(ms,isRunning){if(ms!=null)baseMs=ms;running=!!isRunning;syncAt=Date.now();renderTimer();}
setInterval(renderTimer,250);
// Freeplay (minigame section) vs seasonal (board) — thread the mode through
// every request so the server records to the right scope and skips bonus hints.
// `FREE` is the page-level global set just above this script.
var IS_FREE=(typeof FREE!=='undefined')&&FREE===true;
var FB=IS_FREE?'&free=true':'';
function wf(o){if(IS_FREE)o.free='true';return o;}
function post(url,body){return fetch(url,{method:'POST',body:body?new URLSearchParams(body):undefined}).then(function(r){return r.json()});}
function setStats(h){if(h!=null&&statsEl)statsEl.innerHTML=h;}
function openCell(cell){var cb=cells[cell.r][cell.c];cb.classList.remove('flag');cb.classList.add('open');if(cell.n>0){cb.textContent=cell.n;cb.setAttribute('data-n',cell.n);}else{cb.textContent='';}}
function updateMines(){if(minesleft)minesleft.textContent=(minesN-flags)+' mines';}
function inb(r,c){return r>=0&&r<H&&c>=0&&c<W;}
function nbrs(r,c){var out=[];for(var dr=-1;dr<=1;dr++)for(var dc=-1;dc<=1;dc++){if((dr||dc)&&inb(r+dr,c+dc))out.push([r+dr,c+dc]);}return out;}
function numOf(btn){return btn.classList.contains('open')?(parseInt(btn.textContent,10)||0):-1;}
function surround(r,c){var flagged=0,hidden=[];nbrs(r,c).forEach(function(p){var b=cells[p[0]][p[1]];if(b.classList.contains('flag'))flagged++;else if(!b.classList.contains('open'))hidden.push(p);});return{flagged:flagged,hidden:hidden};}
function buildGrid(){
 var exp=activeKind==='expert',cs=exp?24:28;
 grid.className='msgrid'+(exp?' exp':'');
 grid.innerHTML='';grid.style.gridTemplateColumns='repeat('+W+','+cs+'px)';cells=[];
 for(var r=0;r<H;r++){var row=[];for(var c=0;c<W;c++){
  var btn=document.createElement('button');btn.className='cell';
  (function(r,c,btn){
   btn.addEventListener('click',function(){if(btn.classList.contains('open'))chordReveal(r,c);else reveal(r,c);});
   btn.addEventListener('contextmenu',function(e){e.preventDefault();if(btn.classList.contains('open'))chordFlag(r,c);else toggleFlag(btn);});
  })(r,c,btn);
  grid.appendChild(btn);row.push(btn);
 }cells.push(row);}
}
function toggleFlag(btn){
 if(over||btn.classList.contains('open'))return;
 if(btn.classList.contains('flag')){btn.classList.remove('flag');btn.textContent='';flags--;}
 else{btn.classList.add('flag');btn.textContent='⚑';flags++;}
 updateMines();
}
// Right-click a number: if the remaining hidden neighbours must all be mines, flag them.
function chordFlag(r,c){
 if(over)return;var n=numOf(cells[r][c]);if(n<=0)return;
 var s=surround(r,c);
 if(s.hidden.length>0&&s.hidden.length===n-s.flagged){
  s.hidden.forEach(function(p){var b=cells[p[0]][p[1]];if(!b.classList.contains('flag')){b.classList.add('flag');b.textContent='⚑';flags++;}});
  updateMines();
 }
}
// Left-click a number: once you've flagged exactly that many, reveal the rest.
function chordReveal(r,c){
 if(over)return;var n=numOf(cells[r][c]);if(n<=0)return;
 var s=surround(r,c);
 if(s.flagged!==n)return;
 var q=s.hidden.slice();
 (function step(){
  if(over||!q.length)return;
  var p=q.shift();
  if(cells[p[0]][p[1]].classList.contains('open')){step();return;}
  reveal(p[0],p[1]).then(step);
 })();
}
function reveal(r,c){
 if(over)return Promise.resolve();var btn=cells[r][c];
 if(btn.classList.contains('open')||btn.classList.contains('flag'))return Promise.resolve();
 return post('/minesweeper/reveal',wf({r:r,c:c,kind:activeKind})).then(function(j){
  if(over)return;
  if(!j.ok){msg.className='inlinemsg bad';msg.textContent=j.error||'Invalid.';return;}
  if(j.state==='lost'){over=true;(j.mines||[]).forEach(function(m){var mb=cells[m.r][m.c];mb.classList.add('open','mine');mb.textContent='✱';});msg.className='inlinemsg bad';msg.textContent='Boom — try again.';setStats(j.stats);setTimer(j.elapsed_ms,false);showNewBtn(true);return;}
  (j.cells||[]).forEach(openCell);
  setTimer(j.elapsed_ms,j.running);
  if(j.state==='won'){over=true;msg.className='inlinemsg ok';msg.textContent='Cleared! You earned a bonus hint.';if(j.credits!=null&&creditsEl)creditsEl.textContent=j.credits;setStats(j.stats);showNewBtn(true);}
 }).catch(function(){if(!over){msg.className='inlinemsg bad';msg.textContent='Request failed.';}});
}
function startBoard(w,h,mn){W=w;H=h;minesN=mn;over=false;flags=0;buildGrid();updateMines();}
// Start a fresh board for the active difficulty.
function newGame(){
 msg.textContent='';msg.className='inlinemsg';showNewBtn(false);
 post('/minesweeper/new',wf({kind:activeKind})).then(function(j){
  if(!j.ok){msg.className='inlinemsg bad';msg.textContent=j.error||'Could not start.';grid.innerHTML='';return;}
  startBoard(j.w,j.h,j.mines);setTimer(0,false);
 }).catch(function(){msg.className='inlinemsg bad';msg.textContent='Could not start.';});
}
// Show the active difficulty's board: resume the in-progress one (survives tab
// close/reopen and difficulty switches) or auto-start a fresh one. The timer
// comes back paused and resumes on the next reveal; an untouched board shows no
// "resumed" notice.
function loadGame(){
 fetch('/minesweeper/state?kind='+activeKind+FB).then(function(r){return r.json()}).then(function(j){
  if(!j||!j.active){newGame();return;}
  startBoard(j.w,j.h,j.mines);showNewBtn(false);
  var cs=j.cells||[];cs.forEach(openCell);setTimer(j.elapsed_ms,false);
  msg.className='inlinemsg';msg.textContent=cs.length?'Resumed your board — click to continue.':'';
 }).catch(function(){newGame();});
}
// Pause the active difficulty's clock (tab hidden/closed, or leaving for the
// other difficulty); it resumes on the next reveal of that board.
function pauseKind(kind){
 try{if(!navigator.sendBeacon('/minesweeper/pause?kind='+kind+FB))post('/minesweeper/pause?kind='+kind+FB);}catch(e){post('/minesweeper/pause?kind='+kind+FB);}
}
function pauseNow(){
 if(view!=='board'||over)return;
 setTimer(baseMs+(running?(Date.now()-syncAt):0),false);
 pauseKind(activeKind);
}
function selectTab(kind){
 if(view==='board'&&kind===activeKind){if(over)newGame();return;}
 if(view==='board'&&!over)pauseKind(activeKind); // bank the board we're leaving
 view='board';activeKind=kind;lbview.style.display='none';playview.style.display='';
 updateTabUI();showNewBtn(false);loadGame();
}
document.addEventListener('visibilitychange',function(){if(document.visibilityState==='hidden')pauseNow();});
window.addEventListener('pagehide',pauseNow);
if(newBtn)newBtn.addEventListener('click',newGame);
tabs.forEach(function(t){t.addEventListener('click',function(){
 var lk=t.getAttribute('data-lb');
 if(lk)showLb(lk);else selectTab(t.getAttribute('data-tab'));
});});
updateTabUI();
loadGame();
})();
</script>"#;

/// Page shell. The header bar (brand · "signed in as …" · home) lives in `HEAD`
/// and is filled client-side from `/api/me`, so the identity isn't needed here.
fn page_head(_me: Option<&str>) -> String {
    String::from(HEAD)
}

// ---- accounts (native sign-in, setup, invites, resets, users console) ----

const AUTH_STYLE: &str = r#"<style>
.authform{display:grid;gap:12px;max-width:380px;margin-bottom:10px}
.authform label{display:grid;gap:4px;font-size:13px;color:var(--muted)}
.authform input,.linkform input,.linkform select,.utable select{background:var(--bg);color:var(--fg);border:1px solid var(--bd);border-radius:6px;padding:7px 9px;font-size:13px}
.authform button,.linkform button{justify-self:start}
.utable{width:100%;border-collapse:collapse;font-size:13px;margin-bottom:8px}
.utable th{text-align:left;font-size:11px;letter-spacing:.06em;text-transform:uppercase;color:var(--muted);padding:6px 8px;border-bottom:1px solid var(--bd)}
.utable td{padding:7px 8px;border-bottom:1px solid var(--bd);vertical-align:middle}
.utable form{display:inline}.utable .mono{font-family:var(--mono);font-size:12px}
.utable tr.dead td{color:var(--muted)}
.linkform{display:flex;flex-wrap:wrap;gap:8px;align-items:end;margin:6px 0 14px}
.linkform label{display:grid;gap:4px;font-size:12px;color:var(--muted)}
.minted input{width:100%;font-family:var(--mono);font-size:12.5px;background:var(--bg);color:var(--fg);border:1px solid var(--ok);border-radius:6px;padding:8px 10px;margin-top:6px}
.tag{font-family:var(--mono);font-size:11px;color:var(--muted);border:1px solid var(--bd);border-radius:5px;padding:1px 6px}
</style>"#;

fn auth_page(title: &str, crumb: &str, err: Option<&str>, body: &str) -> Html<String> {
    let mut b = page_head(None);
    b.push_str(AUTH_STYLE);
    b.push_str(&format!(
        r#"<h1>{}</h1><div class="crumb">{crumb}</div>"#,
        esc(title)
    ));
    if let Some(e) = err {
        b.push_str(&format!(r#"<div class="banner bad">{}</div>"#, esc(e)));
    }
    b.push_str(body);
    b.push_str("</main></body></html>");
    Html(b)
}

const NEW_PASSWORD_FIELDS: &str = r#"<label>Password (at least 10 characters)<input type="password" name="password" autocomplete="new-password" minlength="10" required></label>
<label>Confirm password<input type="password" name="confirm" autocomplete="new-password" minlength="10" required></label>"#;

pub fn login_page(rd: &str, username: &str, err: Option<&str>) -> Html<String> {
    auth_page(
        "Sign in",
        r#"<a href="/">← home</a>"#,
        err,
        &format!(
            r#"<form class="authform" method="post" action="/login">
<input type="hidden" name="rd" value="{}">
<label>Username<input type="text" name="username" value="{}" autocomplete="username" required autofocus></label>
<label>Password<input type="password" name="password" autocomplete="current-password" required></label>
<button type="submit">Sign in</button></form>
<p class="statline">No account? Accounts are invite-only — ask an admin for an invite link. Forgot your password? An admin can give you a reset link.</p>"#,
            esc(rd),
            esc(username)
        ),
    )
}

pub fn setup_page(token: &str, username: &str, err: Option<&str>) -> Html<String> {
    auth_page(
        "First-run setup",
        "create the sysadmin account",
        err,
        &format!(
            r#"<p class="statline">This account administers the whole instance: users, invites, seasons.</p>
<form class="authform" method="post" action="/setup">
<input type="hidden" name="token" value="{}">
<label>Username<input type="text" name="username" value="{}" autocomplete="username" required autofocus></label>
{NEW_PASSWORD_FIELDS}
<button type="submit">Create sysadmin</button></form>"#,
            esc(token),
            esc(username)
        ),
    )
}

pub fn invite_page(token: &str, username: &str, err: Option<&str>) -> Html<String> {
    auth_page(
        "Create your account",
        "you've been invited to Bifrost",
        err,
        &format!(
            r#"<form class="authform" method="post" action="/invite/{}">
<label>Username (3–32 letters, digits, _ . -)<input type="text" name="username" value="{}" autocomplete="username" required autofocus></label>
{NEW_PASSWORD_FIELDS}
<button type="submit">Create account</button></form>"#,
            urlq(token),
            esc(username)
        ),
    )
}

pub fn reset_page(token: &str, username: &str, err: Option<&str>) -> Html<String> {
    auth_page(
        "Reset password",
        &format!("for <b>{}</b>", esc(username)),
        err,
        &format!(
            r#"<form class="authform" method="post" action="/reset/{}">
{NEW_PASSWORD_FIELDS}
<button type="submit">Set password</button></form>
<p class="statline">This signs you out everywhere else.</p>"#,
            urlq(token)
        ),
    )
}

/// "in 3 h" / "in 2 d" / "expired".
fn fmt_left(secs: i64) -> String {
    match secs {
        s if s <= 0 => "expired".into(),
        s if s < 3600 => format!("in {} min", (s + 59) / 60),
        s if s < 2 * 86400 => format!("in {} h", (s + 3599) / 3600),
        s => format!("in {} d", (s + 86399) / 86400),
    }
}

pub struct UsersView<'a> {
    pub accounts: &'a [crate::db::AccountRow],
    pub links: &'a [crate::db::LinkRow],
    pub me_id: i64,
    /// Native auth (links and passwords apply).
    pub native: bool,
    /// Roles are dictated by the IdP's groups (read-only here).
    pub idp_roles: bool,
    pub minted: Option<&'a crate::account::Minted>,
    pub err: Option<&'a str>,
    pub now: i64,
}

pub fn users_page(v: &UsersView) -> Html<String> {
    let mut body = String::new();
    if let Some(m) = v.minted {
        body.push_str(&format!(
            r#"<div class="banner ok minted"><b>{}</b> — copy it now; it won't be shown again.
<input readonly id="mintedlink" data-path="{}" value="{}" onclick="this.select()"></div>
<script>(function(){{var i=document.getElementById('mintedlink');i.value=location.origin+i.dataset.path;}})();</script>"#,
            esc(&m.what),
            esc(&m.path),
            esc(&m.path)
        ));
    }

    body.push_str("<h2>Accounts</h2><table class=\"utable\"><tr><th>User</th><th>Role</th><th>Status</th><th></th></tr>");
    for a in v.accounts {
        let me = a.id == v.me_id;
        let role_cell = if v.idp_roles {
            format!(r#"<span class="tag">{}</span>"#, esc(&a.role))
        } else {
            let opts: String = ["user", "moderator", "sysadmin"]
                .iter()
                .map(|r| {
                    let sel = if *r == a.role { " selected" } else { "" };
                    format!(r#"<option value="{r}"{sel}>{r}</option>"#)
                })
                .collect();
            format!(
                r#"<form method="post" action="/admin/users/role"><input type="hidden" name="account_id" value="{}">
<select name="role" onchange="this.form.submit()">{opts}</select></form>"#,
                a.id
            )
        };
        let kind = if a.idp { r#" <span class="tag">idp</span>"# } else { "" };
        let status = if a.disabled { "disabled" } else { "active" };
        let mut actions = String::new();
        if !me {
            actions.push_str(&format!(
                r#"<form method="post" action="/admin/users/disable"><input type="hidden" name="account_id" value="{}"><input type="hidden" name="disabled" value="{}"><button class="ghost" type="submit">{}</button></form> "#,
                a.id,
                !a.disabled,
                if a.disabled { "enable" } else { "disable" }
            ));
        }
        if v.native && a.has_password {
            actions.push_str(&format!(
                r#"<form method="post" action="/admin/links/reset"><input type="hidden" name="account_id" value="{}"><button class="ghost" type="submit">reset link</button></form>"#,
                a.id
            ));
        }
        body.push_str(&format!(
            r#"<tr class="{}"><td class="mono">{}{}{kind}</td><td>{role_cell}</td><td>{status}</td><td>{actions}</td></tr>"#,
            if a.disabled { "dead" } else { "" },
            esc(&a.username),
            if me { " (you)" } else { "" },
        ));
    }
    body.push_str("</table>");
    if v.idp_roles {
        body.push_str(r#"<p class="statline">Roles follow the identity provider's groups on this instance.</p>"#);
    }

    if v.native {
        body.push_str(&format!(
            r#"<h2>Invite links</h2>
<form class="linkform" method="post" action="/admin/links/invite">
<label>Label (optional)<input type="text" name="label" maxlength="80" placeholder="who it's for"></label>
<label>Valid for (days)<input type="number" name="days" value="7" min="1" max="90"></label>
<button type="submit">Mint single-use invite</button></form>
<form class="linkform" method="post" action="/admin/links/community">
<label>Label (optional)<input type="text" name="label" maxlength="80" placeholder="e.g. Discord, season 5"></label>
<label>Valid for (hours)<input type="number" name="hours" value="48" min="1" max="{}"></label>
<button type="submit">Mint community invite</button></form>
<p class="statline">A community invite works for anyone who has it until it expires or you revoke it — fine for a Discord post; single-use invites admit one account.</p>"#,
            90 * 24
        ));
    }

    if !v.links.is_empty() {
        body.push_str("<table class=\"utable\"><tr><th>Link</th><th>Label / for</th><th>Used</th><th>Expires</th><th></th></tr>");
        for l in v.links {
            let live = l.live(v.now);
            let used = match l.max_uses {
                Some(m) => format!("{}/{m}", l.uses),
                None => l.uses.to_string(),
            };
            let label = match (&l.username, l.label.is_empty()) {
                (Some(u), _) => format!("for {}", esc(u)),
                (None, false) => esc(&l.label),
                (None, true) => String::new(),
            };
            let state = if l.revoked {
                "revoked".to_string()
            } else if live {
                fmt_left(l.expires_at - v.now)
            } else if l.expires_at <= v.now {
                "expired".to_string()
            } else {
                "used up".to_string()
            };
            let revoke = if live {
                format!(
                    r#"<form method="post" action="/admin/links/revoke"><input type="hidden" name="id" value="{}"><button class="ghost" type="submit">revoke</button></form>"#,
                    l.id
                )
            } else {
                String::new()
            };
            body.push_str(&format!(
                r#"<tr class="{}"><td><span class="tag">{}</span></td><td>{label}</td><td class="mono">{used}</td><td class="mono" title="{}">{state}</td><td>{revoke}</td></tr>"#,
                if live { "" } else { "dead" },
                esc(&l.kind),
                fmt_date(l.expires_at),
            ));
        }
        body.push_str("</table>");
    }

    auth_page(
        "Users & links",
        r#"<a href="/admin">← admin</a>"#,
        v.err,
        &body,
    )
}
