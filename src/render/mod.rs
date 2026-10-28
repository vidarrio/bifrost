//! All of Bifrost's HTML. Shared helpers and the page shell live here; each
//! area of the site has its own submodule, re-exported so callers use
//! `render::…` throughout.

use axum::response::Html;

use crate::ap::BoardSnapshot;
use crate::db::{SlotRow, SubmissionRow};

mod account;
mod admin;
mod board;
mod console;
mod game;
mod landing;
mod load;
mod log;
mod mine;
mod season;
mod signup;

pub use account::*;
pub use admin::*;
pub use board::*;
pub use console::*;
pub use game::*;
pub use landing::*;
pub use load::*;
pub use log::*;
pub use mine::*;
pub use season::*;
pub use signup::*;

/// HTML-escape (for callers outside this module).
pub fn escape(s: &str) -> String {
    esc(s)
}

/// HTML-escape text for an element or a quoted attribute value.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// A JavaScript string literal for an inline handler attribute (`onclick`):
/// JSON-quoted, then HTML-escaped — the browser decodes the attribute back
/// to the literal, so a quote in `s` can't end the string.
fn js_attr(s: &str) -> String {
    esc(&serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()))
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
.banner{border-radius:10px;padding:12px 16px;margin:0 0 14px}.banner.ok{background:rgba(40,165,109,.15);border:1px solid var(--ok)}.banner.bad{background:rgba(226,77,95,.13);border:1px solid var(--bad)}.muted{color:var(--muted)}.banner.warn{background:rgba(217,131,36,.13);border:1px solid var(--warn)}.loadcharts{display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:12px;margin:8px 0 12px}.loadchart{background:var(--panel);border:1px solid var(--bd);border-radius:10px;padding:10px 12px}.loadchart h3{margin:0 0 4px;font-size:12px;font-weight:600;color:var(--fg)}.loadchart .now{font-family:var(--mono);font-size:20px;color:var(--fg)}.loadchart svg{display:block;width:100%;height:80px;overflow:visible}.loadchart .axis{display:flex;justify-content:space-between;font-family:var(--mono);font-size:10.5px;color:var(--muted)}.loadchart .grid{stroke:var(--bd);stroke-width:1}.loadchart .plot{position:relative}.loadchart .ylab{position:absolute;right:0;top:calc(50% - 15px);font-family:var(--mono);font-size:10.5px;color:var(--muted);pointer-events:none}.loadchart .line{fill:none;stroke:var(--accent);stroke-width:2;stroke-linejoin:round;stroke-linecap:round}.loadchart .hit{fill:transparent}.loadchart .hit:hover{fill:rgba(90,169,230,.18)}
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
<header><div class="hwrap"><a class="brand" href="/">Bifrost</a><span class="spacer"></span><span class="who" id="who"></span><span id="adminslot"></span><span id="homeslot"></span></div></header>
<script>
// Fill the header from a cached identity synchronously (before paint) so it
// doesn't flicker empty→filled on reload, then confirm/update from /api/me.
function bifApplyMe(m){
 var w=document.getElementById('who');
 if(w){if(m&&m.user){w.innerHTML='signed in as <b></b>'+(m.sign_out?' · <a href="'+m.sign_out+'" id="signout">sign out</a>':'');w.querySelector('b').textContent=m.user;var so=document.getElementById('signout');if(so)so.onclick=function(e){e.preventDefault();if(m.sign_out==='/logout'){var f=document.createElement('form');f.method='post';f.action='/logout';document.body.appendChild(f);f.submit()}else location.href=m.sign_out}}else{w.innerHTML='<a href="'+((m&&m.sign_in)||'/login')+'">sign in</a>'}}
 var a=document.getElementById('adminslot');
 if(a)a.innerHTML=(m&&m.admin)?'<a class="pill" href="/admin/seasons">admin</a>':'';
 var h=document.getElementById('homeslot');
 if(h){h.innerHTML='';if(m&&m.home){var l=document.createElement('a');l.className='pill';l.href=m.home;l.textContent='home';h.appendChild(l)}}
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
  fetch('board/fragment').then(function(r){return r.json()}).then(function(j){
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
  fetch('log/fragment'+window.location.search).then(function(r){return r.text()}).then(function(h){
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
    groups.sort_by(|a, b| {
        a.game
            .to_ascii_lowercase()
            .cmp(&b.game.to_ascii_lowercase())
    });
    groups
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

/// Toggles the game page's tab panels (delegated click on the tab bar).
/// Tab bar styling shared by the game page and the season console.
const TABS_CSS: &str = r#"<style>
.gtabs{display:flex;flex-wrap:wrap;gap:2px;border-bottom:1px solid var(--bd);margin:22px 0 16px}
.gtab{background:none;color:var(--muted);border:0;border-bottom:2px solid transparent;border-radius:0;margin-bottom:-1px;padding:8px 15px;font-family:var(--font);font-size:14px;font-weight:600;cursor:pointer}
.gtab:hover{color:var(--fg)}
.gtab.on{color:var(--accent);border-bottom-color:var(--accent)}
a.gtab{display:inline-block;text-decoration:none}
</style>"#;

/// The instance console's tab bar: one page per tab, so these
/// are links. Moderators see only Seasons.
fn console_nav(active: &str, sysadmin: bool) -> String {
    console_nav_with(active, sysadmin, crate::release_check::newer().as_ref())
}

/// The console's tab bar, plus the newer-release banner for sysadmins.
fn console_nav_with(
    active: &str,
    sysadmin: bool,
    newer: Option<&crate::release_check::Newer>,
) -> String {
    let mut out = format!(r#"{TABS_CSS}<div class="gtabs">"#);
    for (href, label) in [
        ("/admin/seasons", "Seasons"),
        ("/admin/users", "Users &amp; links"),
        ("/admin/catalog", "Apworlds"),
        ("/admin/settings", "Settings"),
    ] {
        if !sysadmin && href != "/admin/seasons" {
            continue;
        }
        let on = if href == active { " on" } else { "" };
        out.push_str(&format!(r#"<a class="gtab{on}" href="{href}">{label}</a>"#));
    }
    out.push_str("</div>");
    if sysadmin && let Some(n) = newer {
        out.push_str(&update_banner(n));
    }
    out
}

fn update_banner(n: &crate::release_check::Newer) -> String {
    format!(
        r#"<div class="banner warn">Bifrost <b>{}</b> is out (this is {}): <a href="{}">what's new</a> · <a href="https://github.com/vidarrio/bifrost/blob/main/docs/USER_GUIDE.md#updating-bifrost">how to update</a></div>"#,
        esc(&n.version),
        esc(crate::release_check::current()),
        esc(&n.url),
    )
}

/// Toggles `.gtabpanel`s from the `.gtabs` bar. The open tab lives in the URL
/// hash, and every form's action carries it too: a post's redirect inherits
/// the fragment, so you land back on the tab you acted from.
const TABS_JS: &str = r#"<script>
(function(){var bar=document.querySelector('.gtabs');if(!bar)return;
function activate(id,push){var btn=bar.querySelector('.gtab[data-tab="'+id+'"]');if(!btn)return;
bar.querySelectorAll('.gtab').forEach(function(x){x.classList.toggle('on',x===btn);});
document.querySelectorAll('.gtabpanel').forEach(function(p){p.hidden=(p.id!==id);});
if(push)history.replaceState(null,'','#'+id);
document.querySelectorAll('form').forEach(function(f){var a=f.getAttribute('action');if(a)f.setAttribute('action',a.split('#')[0]+'#'+id);});}
bar.addEventListener('click',function(e){var b=e.target.closest('.gtab');if(!b)return;activate(b.getAttribute('data-tab'),true);});
var h=(location.hash||'').slice(1),el=h&&document.getElementById(h);
if(el&&el.classList.contains('gtabpanel'))activate(h,false);
})();
</script>"#;

/// Tab bar + the opening tag of the (default-visible) Hints panel. The caller
/// closes the panel, then appends `inlogic_panel(...)` and `GAME_TAB_JS`.
fn tabs_open() -> &'static str {
    r#"<div class="gtabs"><button type="button" class="gtab on" data-tab="tab-hints">Hints</button><button type="button" class="gtab" data-tab="tab-ut">Universal Tracker</button><button type="button" class="gtab" data-tab="tab-pt">PopTracker</button><button type="button" class="gtab" data-tab="tab-log">Log</button></div><div class="gtabpanel" id="tab-hints">"#
}

/// Page shell. The header bar (brand · "signed in as …" · home) lives in `HEAD`
/// and is filled client-side from `/api/me`, so the identity isn't needed here.
fn page_head(_me: Option<&str>) -> String {
    with_base(HEAD)
}

/// A page head with `<base>` set: season links are base-relative, and the base
/// pins them to the season the page was rendered for (see `scope`).
pub fn with_base(head: &str) -> String {
    head.replacen(
        "<meta charset=\"utf-8\">",
        &format!(
            "<meta charset=\"utf-8\"><base href=\"{}/\">",
            esc(&crate::scope::link_base())
        ),
        1,
    )
}

// ---- accounts (native sign-in, setup, invites, resets, users console) ----

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_releases_show_to_sysadmins() {
        let n = crate::release_check::Newer {
            version: "0.2.0<x>".into(),
            url: "https://github.com/vidarrio/bifrost/releases/tag/v0.2.0".into(),
        };
        let b = update_banner(&n);
        assert!(console_nav_with("/admin/settings", true, Some(&n)).contains(&b));
        assert!(!console_nav_with("/admin/seasons", false, Some(&n)).contains("is out"));
        assert!(!console_nav_with("/admin/settings", true, None).contains("is out"));
        assert!(b.contains("Bifrost <b>0.2.0&lt;x&gt;</b> is out"), "{b}");
        assert!(b.contains(r#"href="https://github.com/vidarrio/bifrost/releases/tag/v0.2.0""#));
        use console::release_status as st;
        assert!(st(false, Some(&(0, Ok(Some(n.clone()))))).contains("checks off"));
        assert!(st(true, None).contains("not checked"));
        assert!(st(true, Some(&(0, Ok(None)))).contains("newest"));
        assert!(st(true, Some(&(0, Ok(Some(n))))).contains("0.2.0&lt;x&gt;</a> is out"));
        assert!(
            st(true, Some(&(0, Err("<rate limited>".into())))).contains("&lt;rate limited&gt;")
        );
    }

    #[test]
    fn attribute_values_cant_be_broken_out_of() {
        assert_eq!(
            esc(r#"x" onmouseover="a()"#),
            "x&quot; onmouseover=&quot;a()"
        );
        assert_eq!(esc("it's <b>"), "it&#39;s &lt;b&gt;");
        // In a handler the attribute decodes back to a JSON string literal.
        assert_eq!(
            js_attr("Delete O'Brien\"s?"),
            r#"&quot;Delete O&#39;Brien\&quot;s?&quot;"#
        );
        let btn = action_btn("x", &[], "go", true, "Remove ');alert(1);('?");
        assert!(
            btn.contains("confirm(&quot;Remove &#39;);alert(1);(&#39;?&quot;)"),
            "{btn}"
        );
    }

    #[test]
    fn imported_summary_fields_are_escaped() {
        let s = crate::season::SeasonSummary {
            name: "<s>".into(),
            games: vec![crate::season::SeasonGame {
                status: "x\" onmouseover=\"a()".into(),
                ..Default::default()
            }],
            minesweeper: vec![crate::season::MineBoard {
                label: "<script>bad()</script>".into(),
                time: vec![("p".into(), 1)],
                ..Default::default()
            }],
            ..Default::default()
        };
        let f = LogFilter {
            kinds_selected: [true; 4],
            classes_selected: [true; 4],
            sender: "",
            receiver: "",
            within: 0,
            since: None,
            until: None,
            participants: &[],
            qs: "",
        };
        let html = season_page(&s, &[], 1, 1, 50, &f).0;
        assert!(
            !html.contains("<script>bad()") && !html.contains("x\" onmouseover"),
            "unescaped field"
        );
    }
}
