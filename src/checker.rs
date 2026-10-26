//! YAML checker — validates an Archipelago options file against the installed
//! worlds plus the season's custom worlds, via the AP adapter (AP's own
//! `roll_settings`, the same logic the official checker uses). Persists nothing.

use axum::response::Html;
use serde::Deserialize;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;

use crate::{ap_adapter, config};

/// Each check spawns a Python that loads every world (~300 MB). This box also
/// runs the game, so we serialize checks — a burst of requests queues instead of
/// storm-spawning and OOM-killing the MultiServer.
fn check_gate() -> &'static Semaphore {
    static SEM: OnceLock<Semaphore> = OnceLock::new();
    SEM.get_or_init(|| Semaphore::new(1))
}

/// Checks queued behind the gate (plus the running one). Beyond
/// [`MAX_QUEUED`] new checks are refused rather than piling up a backlog.
static QUEUED: AtomicUsize = AtomicUsize::new(0);
const MAX_QUEUED: usize = 8;

/// Decrements [`QUEUED`] however the check ends.
struct QueueSlot;
impl Drop for QueueSlot {
    fn drop(&mut self) {
        QUEUED.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Guard against someone pasting something enormous into the box.
const MAX_YAML: usize = 512 * 1024;

#[derive(Debug, Deserialize)]
pub struct CheckResult {
    pub ok: bool,
    #[serde(default)]
    pub players: Vec<PlayerResult>,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PlayerResult {
    pub name: String,
    #[serde(default)]
    pub game: Option<String>,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// Validate `yaml` (raw file contents) against the season's worlds.
pub async fn check_yaml(yaml: &str) -> Result<CheckResult, String> {
    if yaml.len() > MAX_YAML {
        return Err(format!(
            "file too large ({} bytes; max {MAX_YAML})",
            yaml.len()
        ));
    }
    if QUEUED.fetch_add(1, Ordering::SeqCst) >= MAX_QUEUED {
        QUEUED.fetch_sub(1, Ordering::SeqCst);
        return Err("the checker is busy — try again in a minute".into());
    }
    let _slot = QueueSlot;
    // Serialize checks (see check_gate). Held for the whole subprocess run.
    let _permit = check_gate()
        .acquire()
        .await
        .map_err(|e| format!("checker busy: {e}"))?;
    ap_adapter::check_yaml(&config::get().season(), yaml).await
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const HEAD: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>Bifrost · YAML checker</title>
<style>
:root{--bg:#0d1117;--panel:#161c26;--bd:#2a3542;--fg:#e6e9ef;--muted:#98a3b6;--accent:#5aa9e6;--ok:#28a56d;--bad:#e24d5f;
--font:"IBM Plex Sans",system-ui,sans-serif;--mono:"IBM Plex Mono",ui-monospace,Menlo,monospace}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font-family:var(--font);line-height:1.55;
display:flex;flex-direction:column;align-items:center}main{width:100%;max-width:760px;padding:16px;padding-block:6vh 40px}
a{color:var(--accent);text-decoration:none}a:hover{text-decoration:underline}
h1{font-size:30px;margin:0 0 4px}.crumb{font-family:var(--mono);font-size:12px;color:var(--muted);margin-bottom:22px}
.lede{color:var(--muted);margin:0 0 20px}
textarea{width:100%;min-height:280px;background:var(--panel);color:var(--fg);border:1px solid var(--bd);border-radius:10px;
padding:12px;font-family:var(--mono);font-size:13px;resize:vertical}
button{margin-top:12px;background:var(--accent);color:#0d1117;border:0;border-radius:8px;padding:10px 18px;
font-weight:600;font-size:14px;cursor:pointer}button:hover{filter:brightness(1.08)}
.banner{border-radius:10px;padding:12px 16px;margin:0 0 16px;font-weight:600}
.banner.ok{background:rgba(40,165,109,.15);border:1px solid var(--ok);color:#7fe3b4}
.banner.bad{background:rgba(226,77,95,.13);border:1px solid var(--bad);color:#f2a7b0}
.slot{background:var(--panel);border:1px solid var(--bd);border-left-width:4px;border-radius:10px;padding:12px 14px;margin:10px 0}
.slot.ok{border-left-color:var(--ok)}.slot.bad{border-left-color:var(--bad)}
.slot .who{font-family:var(--mono);font-size:13px}.slot .game{color:var(--muted);font-size:13px}
.slot .err{font-family:var(--mono);font-size:12.5px;color:#f2a7b0;margin-top:6px;white-space:pre-wrap;word-break:break-word}
</style></head><body><main>
<h1>YAML checker</h1><div class="crumb"><a href="/">← Bifrost</a> · validate against the installed worlds</div>"#;

const FOOT: &str = "</main></body></html>";

pub fn form_page() -> Html<String> {
    let mut b = String::from(HEAD);
    b.push_str(
        r#"<p class="lede">Paste your Archipelago options YAML and check it against the worlds installed on the server — before you submit it. Catches version drift and invalid option values.</p>
<form method="post" action="/check">
<textarea name="yaml" placeholder="name: MyName&#10;game: ...&#10;..." required></textarea>
<div><button type="submit">Check it</button></div>
</form>"#,
    );
    b.push_str(FOOT);
    Html(b)
}

pub fn result_page(res: &CheckResult) -> Html<String> {
    let mut body = String::from(HEAD);
    if res.ok {
        body.push_str(r#"<div class="banner ok">Looks good — every slot rolled cleanly.</div>"#);
    } else {
        body.push_str(&format!(
            r#"<div class="banner bad">{} problem(s) found — see below.</div>"#,
            res.errors.len().max(1)
        ));
    }
    for p in &res.players {
        let cls = if p.ok { "ok" } else { "bad" };
        body.push_str(&format!(
            r#"<div class="slot {cls}"><div class="who">{}</div>"#,
            esc(&p.name)
        ));
        if let Some(g) = &p.game {
            body.push_str(&format!(r#"<div class="game">{}</div>"#, esc(g)));
        }
        if let Some(e) = &p.error {
            body.push_str(&format!(r#"<div class="err">{}</div>"#, esc(e)));
        }
        body.push_str("</div>");
    }
    if res.players.is_empty() && !res.errors.is_empty() {
        for e in &res.errors {
            body.push_str(&format!(
                r#"<div class="slot bad"><div class="err">{}</div></div>"#,
                esc(e)
            ));
        }
    }
    body.push_str(r#"<div style="margin-top:18px"><a href="/check">← check another</a></div>"#);
    body.push_str(FOOT);
    Html(body)
}

pub fn error_page(msg: &str) -> Html<String> {
    let mut body = String::from(HEAD);
    body.push_str(&format!(
        r#"<div class="banner bad">Checker error: {}</div><div><a href="/check">← back</a></div>"#,
        esc(msg)
    ));
    body.push_str(FOOT);
    Html(body)
}
