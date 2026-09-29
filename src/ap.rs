//! Archipelago tracker client + live board state.
//!
//! Bifrost holds one persistent WebSocket connection to the MultiServer as a
//! `Tracker`-tagged client. From it we learn every slot (via `Connected`),
//! subscribe to each slot's status + hints in the server's data storage
//! (`_read_client_status_*` / `_read_hints_*`), and fold the global `PrintJSON`
//! event stream (item sends, goals, hints, joins) plus `Bounced` DeathLinks into
//! a rolling feed. Item/location *names* need the data package and are deferred;
//! events render at the slot level.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::response::Html;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

const MAX_EVENTS: usize = 60;

#[derive(Clone, Default)]
pub struct SlotState {
    pub name: String,
    pub game: String,
    pub status: i64, // ClientStatus: 0 unknown,5 connected,10 ready,20 playing,30 goal
    pub hints: usize,
    pub sent: u64,
    pub received: u64,
}

#[derive(Clone)]
pub struct Event {
    pub icon: &'static str,
    pub text: String,
}

#[derive(Default)]
pub struct Board {
    pub connected: bool,
    pub team: i64,
    pub seed: Option<String>,
    pub slots: BTreeMap<i64, SlotState>,
    pub events: VecDeque<Event>,
}

impl Board {
    fn push_event(&mut self, icon: &'static str, text: String) {
        self.events.push_front(Event { icon, text });
        while self.events.len() > MAX_EVENTS {
            self.events.pop_back();
        }
    }
    fn name_of(&self, slot: i64) -> String {
        if slot == 0 {
            return "Server".into();
        }
        self.slots
            .get(&slot)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| format!("slot {slot}"))
    }
}

#[derive(Clone)]
pub struct ApConfig {
    pub url: String,
    pub slot: String,
    pub password: String,
}

impl ApConfig {
    pub fn from_env() -> Option<Self> {
        let slot = std::env::var("BIFROST_SLOT")
            .ok()
            .filter(|s| !s.is_empty())?;
        Some(Self {
            url: std::env::var("BIFROST_AP_URL").unwrap_or_else(|_| "ws://127.0.0.1:38281".into()),
            slot,
            password: std::env::var("BIFROST_AP_PASSWORD").unwrap_or_default(),
        })
    }
}

/// Spawn the reconnecting tracker task.
pub fn spawn(board: Arc<RwLock<Board>>, cfg: ApConfig) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = run_once(&board, &cfg).await {
                tracing::warn!("AP tracker disconnected: {e}");
            }
            if let Ok(mut b) = board.write() {
                b.connected = false;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

async fn run_once(board: &Arc<RwLock<Board>>, cfg: &ApConfig) -> Result<(), String> {
    let (ws, _) = tokio_tungstenite::connect_async(&cfg.url)
        .await
        .map_err(|e| format!("connect {}: {e}", cfg.url))?;
    let (mut tx, mut rx) = ws.split();
    tracing::info!("AP tracker connected to {}", cfg.url);

    while let Some(msg) = rx.next().await {
        let msg = msg.map_err(|e| format!("recv: {e}"))?;
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => return Err("server closed".into()),
            Message::Ping(p) => {
                let _ = tx.send(Message::Pong(p)).await;
                continue;
            }
            _ => continue,
        };
        let packets: Vec<Value> = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for pkt in packets {
            let cmd = pkt.get("cmd").and_then(Value::as_str).unwrap_or("");
            match cmd {
                "RoomInfo" => {
                    // Answer the handshake as a read-only tracker.
                    let connect = json!([{
                        "cmd": "Connect",
                        "game": "",
                        "name": cfg.slot,
                        "uuid": "bifrost-tracker",
                        "version": {"class": "Version", "major": 0, "minor": 6, "build": 7},
                        "items_handling": 0,
                        "tags": ["Tracker"],
                        "slot_data": false,
                        "password": cfg.password,
                    }]);
                    if let (Some(seed), Ok(mut b)) =
                        (pkt.get("seed_name").and_then(Value::as_str), board.write())
                    {
                        b.seed = Some(seed.to_string());
                    }
                    tx.send(Message::Text(connect.to_string()))
                        .await
                        .map_err(|e| format!("send Connect: {e}"))?;
                }
                "ConnectionRefused" => {
                    let errs = pkt.get("errors").map(|e| e.to_string()).unwrap_or_default();
                    return Err(format!("connection refused: {errs}"));
                }
                "Connected" => {
                    let keys = on_connected(board, &pkt);
                    // Subscribe to + fetch every slot's status and hints.
                    let sub = json!([
                        {"cmd": "Get", "keys": keys},
                        {"cmd": "SetNotify", "keys": keys},
                    ]);
                    tx.send(Message::Text(sub.to_string()))
                        .await
                        .map_err(|e| format!("send Get/SetNotify: {e}"))?;
                }
                "PrintJSON" => on_printjson(board, &pkt),
                "Retrieved" => {
                    if let Some(map) = pkt.get("keys").and_then(Value::as_object) {
                        let mut b = board.write().map_err(|_| "lock")?;
                        for (k, v) in map {
                            apply_storage(&mut b, k, v);
                        }
                    }
                }
                "SetReply" => {
                    if let Some(k) = pkt.get("key").and_then(Value::as_str) {
                        let mut b = board.write().map_err(|_| "lock")?;
                        let v = pkt.get("value").cloned().unwrap_or(Value::Null);
                        apply_storage(&mut b, k, &v);
                    }
                }
                "Bounced" => on_bounced(board, &pkt),
                _ => {}
            }
        }
    }
    Err("stream ended".into())
}

/// Populate slots from `Connected.slot_info`; return the data-storage keys to
/// subscribe to (client_status + hints for every slot on our team).
fn on_connected(board: &Arc<RwLock<Board>>, pkt: &Value) -> Vec<String> {
    let team = pkt.get("team").and_then(Value::as_i64).unwrap_or(0);
    let mut keys = Vec::new();
    let mut b = match board.write() {
        Ok(b) => b,
        Err(_) => return keys,
    };
    b.connected = true;
    b.team = team;
    if let Some(info) = pkt.get("slot_info").and_then(Value::as_object) {
        for (slot_str, ns) in info {
            let slot: i64 = match slot_str.parse() {
                Ok(n) => n,
                Err(_) => continue,
            };
            // Skip group/spectator pseudo-slots (type != player == 1).
            let stype = ns.get("type").and_then(Value::as_i64).unwrap_or(1);
            if stype != 1 {
                continue;
            }
            let name = ns
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string();
            let game = ns
                .get("game")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string();
            let entry = b.slots.entry(slot).or_default();
            entry.name = name;
            entry.game = game;
            keys.push(format!("_read_client_status_{team}_{slot}"));
            keys.push(format!("_read_hints_{team}_{slot}"));
        }
    }
    b.push_event("*", "Bifrost connected to the multiworld.".into());
    keys
}

fn slot_from_key(rest: &str) -> Option<i64> {
    rest.rsplit('_').next().and_then(|s| s.parse::<i64>().ok())
}

fn apply_storage(b: &mut Board, key: &str, value: &Value) {
    if let Some(rest) = key.strip_prefix("_read_client_status_") {
        let Some(slot) = slot_from_key(rest) else {
            return;
        };
        let status = value.as_i64().unwrap_or(0);
        let Some(s) = b.slots.get_mut(&slot) else {
            return;
        };
        let was_goal = s.status == 30;
        s.status = status;
        if status == 30 && !was_goal {
            let name = s.name.clone();
            b.push_event("goal", format!("{name} reached their goal!"));
        }
    } else if let Some(rest) = key.strip_prefix("_read_hints_") {
        let Some(slot) = slot_from_key(rest) else {
            return;
        };
        let n = value.as_array().map(|a| a.len()).unwrap_or(0);
        if let Some(s) = b.slots.get_mut(&slot) {
            s.hints = n;
        }
    }
}

fn on_printjson(board: &Arc<RwLock<Board>>, pkt: &Value) {
    let ty = pkt.get("type").and_then(Value::as_str).unwrap_or("");
    let mut b = match board.write() {
        Ok(b) => b,
        Err(_) => return,
    };
    match ty {
        "ItemSend" => {
            let src = pkt
                .get("item")
                .and_then(|i| i.get("player"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let dst = pkt.get("receiving").and_then(Value::as_i64).unwrap_or(0);
            let flags = pkt
                .get("item")
                .and_then(|i| i.get("flags"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if let Some(s) = b.slots.get_mut(&src) {
                s.sent += 1;
            }
            if let Some(s) = b.slots.get_mut(&dst) {
                s.received += 1;
            }
            // Only surface progression/useful/trap items in the feed to keep it
            // meaningful (filler floods otherwise).
            if flags != 0 {
                let (sn, dn) = (b.name_of(src), b.name_of(dst));
                let icon = if flags & 0b100 != 0 { "trap" } else { "item" };
                let text = if src == dst {
                    format!("{sn} found their own item")
                } else {
                    format!("{sn} → {dn}")
                };
                b.push_event(icon, text);
            }
        }
        "Goal" => {
            if let Some(slot) = pkt.get("slot").and_then(Value::as_i64) {
                let name = b.name_of(slot);
                if let Some(s) = b.slots.get_mut(&slot) {
                    s.status = 30;
                }
                b.push_event("goal", format!("{name} reached their goal!"));
            }
        }
        "Release" => {
            if let Some(slot) = pkt.get("slot").and_then(Value::as_i64) {
                let name = b.name_of(slot);
                b.push_event("*", format!("{name} released their remaining items"));
            }
        }
        "Collect" => {
            if let Some(slot) = pkt.get("slot").and_then(Value::as_i64) {
                let name = b.name_of(slot);
                b.push_event("*", format!("{name} collected their remaining items"));
            }
        }
        "Join" => {
            if let Some(slot) = pkt.get("slot").and_then(Value::as_i64) {
                let name = b.name_of(slot);
                b.push_event("join", format!("{name} connected"));
            }
        }
        "Part" => {
            if let Some(slot) = pkt.get("slot").and_then(Value::as_i64) {
                let name = b.name_of(slot);
                b.push_event("part", format!("{name} disconnected"));
            }
        }
        _ => {}
    }
}

fn on_bounced(board: &Arc<RwLock<Board>>, pkt: &Value) {
    let tags = pkt.get("tags").and_then(Value::as_array);
    let is_deathlink = tags
        .map(|t| t.iter().any(|v| v.as_str() == Some("DeathLink")))
        .unwrap_or(false);
    if !is_deathlink {
        return;
    }
    let data = pkt.get("data");
    let source = data
        .and_then(|d| d.get("source"))
        .and_then(Value::as_str)
        .unwrap_or("someone");
    let cause = data.and_then(|d| d.get("cause")).and_then(Value::as_str);
    if let Ok(mut b) = board.write() {
        let text = match cause {
            Some(c) if !c.is_empty() => format!("{source} died — {c}"),
            _ => format!("{source} died"),
        };
        b.push_event("death", text);
    }
}

// ---- rendering ----

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

pub fn board_page(board: &Arc<RwLock<Board>>) -> Html<String> {
    let b = match board.read() {
        Ok(b) => b,
        Err(_) => return Html("<p>board unavailable</p>".into()),
    };
    let mut out = String::from(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><meta http-equiv="refresh" content="12">
<title>Bifrost · live board</title><style>
:root{--bg:#0d1117;--panel:#161c26;--bd:#2a3542;--fg:#e6e9ef;--muted:#98a3b6;--accent:#5aa9e6;--ok:#28a56d;--bad:#e24d5f;--warn:#d98324;
--font:"IBM Plex Sans",system-ui,sans-serif;--mono:"IBM Plex Mono",ui-monospace,Menlo,monospace}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font-family:var(--font);line-height:1.5;
display:flex;flex-direction:column;align-items:center}main{width:100%;max-width:900px;padding:16px;padding-block:5vh 40px}
a{color:var(--accent);text-decoration:none}a:hover{text-decoration:underline}
h1{font-size:30px;margin:0 0 2px}.crumb{font-family:var(--mono);font-size:12px;color:var(--muted);margin-bottom:20px}
.dot{display:inline-block;width:8px;height:8px;border-radius:50%;margin-right:6px;vertical-align:middle}
.on{background:var(--ok)}.off{background:var(--bad)}
h2{font-size:12px;letter-spacing:.09em;text-transform:uppercase;color:var(--muted);margin:26px 0 10px}
.grid{display:grid;grid-template-columns:repeat(2,1fr);gap:10px}@media(max-width:560px){.grid{grid-template-columns:1fr}}
.slot{background:var(--panel);border:1px solid var(--bd);border-left-width:4px;border-radius:10px;padding:10px 12px}
.slot.goal{border-left-color:var(--ok)}.slot.playing{border-left-color:var(--accent)}.slot.idle{border-left-color:var(--bd)}
.slot .top{display:flex;justify-content:space-between;gap:8px}
.slot .who{font-family:var(--mono);font-size:13px}.slot .game{color:var(--muted);font-size:12px}
.slot .st{font-family:var(--mono);font-size:11px;color:var(--muted)}.slot.goal .st{color:var(--ok)}
.slot .meta{font-family:var(--mono);font-size:11px;color:var(--muted);margin-top:6px}
.feed{display:grid;gap:6px}.ev{display:flex;gap:8px;font-size:13px;background:var(--panel);border:1px solid var(--bd);border-radius:8px;padding:7px 10px}
.ev .i{font-family:var(--mono);font-size:10px;color:var(--muted);min-width:44px}
.ev.goal .i{color:var(--ok)}.ev.death .i{color:var(--bad)}.ev.trap .i{color:var(--warn)}
.empty{color:var(--muted);font-size:13px}
</style></head><body><main>
<h1>Live board</h1><div class="crumb"><a href="/">← Bifrost</a> · "#,
    );
    if b.connected {
        out.push_str(r#"<span class="dot on"></span>connected"#);
    } else {
        out.push_str(r#"<span class="dot off"></span>waiting for the server…"#);
    }
    if let Some(seed) = &b.seed {
        out.push_str(&format!(" · seed {}", esc(seed)));
    }
    out.push_str("</div>");

    // slots
    out.push_str(r#"<h2>Slots</h2>"#);
    if b.slots.is_empty() {
        out.push_str(r#"<div class="empty">No slots yet — the board fills once Bifrost connects to a running game.</div>"#);
    } else {
        out.push_str(r#"<div class="grid">"#);
        for s in b.slots.values() {
            let (cls, label) = status_label(s.status);
            out.push_str(&format!(
                r#"<div class="slot {cls}"><div class="top"><span class="who">{}</span><span class="st">{}</span></div>
<div class="game">{}</div><div class="meta">sent {} · recv {} · hints {}</div></div>"#,
                esc(&s.name),
                label,
                esc(&s.game),
                s.sent,
                s.received,
                s.hints,
            ));
        }
        out.push_str("</div>");
    }

    // feed
    out.push_str(r#"<h2>Activity</h2>"#);
    if b.events.is_empty() {
        out.push_str(r#"<div class="empty">Quiet so far. Item finds, hints, goals and DeathLinks show up here.</div>"#);
    } else {
        out.push_str(r#"<div class="feed">"#);
        for e in &b.events {
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
