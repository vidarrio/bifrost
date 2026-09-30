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

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

const MAX_EVENTS: usize = 60;

/// A hint as stored in `_read_hints_*` (ids; names resolved at render via Meta).
#[derive(Clone)]
pub struct HintRaw {
    pub receiving: i64,
    pub finding: i64,
    pub item: i64,
    pub location: i64,
    pub found: bool,
}

#[derive(Clone, Default)]
pub struct SlotState {
    pub name: String,
    pub game: String,
    pub status: i64, // ClientStatus: 0 unknown,5 connected,10 ready,20 playing,30 goal
    pub hint_list: Vec<HintRaw>,
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
    /// Percentage of a world's locations that must be checked to afford a hint.
    pub hint_cost: i64,
    pub slots: BTreeMap<i64, SlotState>,
    pub events: VecDeque<Event>,
    pub meta: Option<std::sync::Arc<crate::meta::Meta>>,
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

/// A resolved hint for one of a slot's items, carrying the display text, the
/// (finding, location, item) identity used to prioritize it, and the receiving
/// slot (the item's owner) used to authorize who may prioritize it.
#[derive(Clone)]
pub struct HintView {
    pub finding: i64,
    pub location: i64,
    pub item: i64,
    pub receiving: i64,
    /// The item has already been found at its location — such hints sort to the
    /// bottom, can't be prioritized, and drop off the board.
    pub found: bool,
    pub text: String,
}

/// Live per-AP-slot figures, snapshotted for rendering without holding the lock.
#[derive(Clone, Default)]
pub struct ApLive {
    pub status: i64,
    pub sent: u64,
    pub received: u64,
    pub world_size: u32,
    pub items_elsewhere: u32,
    /// Hints for items hinted to be in this slot's world (with identity).
    pub hint_views: Vec<HintView>,
}

/// An owned copy of what the pages need to render, taken under one short lock.
pub struct BoardSnapshot {
    pub connected: bool,
    pub seed: Option<String>,
    pub hint_cost: i64,
    pub ap: BTreeMap<i64, ApLive>,
    pub events: Vec<Event>,
}

impl Board {
    /// Data for the admin console's command autocomplete: the connected player
    /// names, each player's game, and the item/location name pools per game.
    pub fn autocomplete_data(&self) -> Value {
        let mut players: Vec<String> = Vec::new();
        let mut game = serde_json::Map::new();
        for s in self.slots.values() {
            if s.name.is_empty() {
                continue;
            }
            players.push(s.name.clone());
            game.insert(s.name.clone(), Value::String(s.game.clone()));
        }
        let mut items = serde_json::Map::new();
        let mut locations = serde_json::Map::new();
        if let Some(m) = self.meta.as_ref() {
            for (g, names) in &m.item_names {
                let mut v: Vec<String> = names.values().cloned().collect();
                v.sort();
                v.dedup();
                items.insert(g.clone(), json!(v));
            }
            for (g, names) in &m.location_names {
                let mut v: Vec<String> = names.values().cloned().collect();
                v.sort();
                v.dedup();
                locations.insert(g.clone(), json!(v));
            }
        }
        json!({"players": players, "game": game, "items": items, "locations": locations})
    }

    pub fn snapshot(&self) -> BoardSnapshot {
        let ap = self
            .slots
            .iter()
            .map(|(n, s)| {
                let (world_size, items_elsewhere) = self
                    .meta
                    .as_ref()
                    .and_then(|m| m.totals(*n))
                    .map(|t| (t.world_size, t.items_for_elsewhere))
                    .unwrap_or((0, 0));
                // Hints for THIS slot's own items — "where is my item", the way
                // hints are normally read. Filter by receiving == this slot; the
                // item is ours, the location is in whichever world is finding it.
                let mut hint_views = Vec::new();
                if let Some(m) = self.meta.as_ref() {
                    for h in s.hint_list.iter().filter(|h| h.receiving == *n) {
                        let item = m.item_name(&s.game, h.item).unwrap_or("?");
                        let find_game = self
                            .slots
                            .get(&h.finding)
                            .map(|x| x.game.as_str())
                            .unwrap_or("");
                        let loc = m.location_name(find_game, h.location).unwrap_or("?");
                        let finder = self.name_of(h.finding);
                        let mark = if h.found { " ✓" } else { "" };
                        hint_views.push(HintView {
                            finding: h.finding,
                            location: h.location,
                            item: h.item,
                            receiving: h.receiving,
                            found: h.found,
                            text: format!("{item} @ {loc} · {finder}{mark}"),
                        });
                    }
                }
                (
                    *n,
                    ApLive {
                        status: s.status,
                        sent: s.sent,
                        received: s.received,
                        world_size,
                        items_elsewhere,
                        hint_views,
                    },
                )
            })
            .collect();
        BoardSnapshot {
            connected: self.connected,
            seed: self.seed.clone(),
            hint_cost: self.hint_cost,
            ap,
            events: self.events.iter().cloned().collect(),
        }
    }
}

#[derive(Clone)]
pub struct ApConfig {
    pub url: String,
    pub slot: String,
    pub password: String,
    /// Server admin password (`server_password` in host.yaml) for `!admin`.
    pub admin_password: String,
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
            admin_password: std::env::var("BIFROST_AP_ADMIN_PASSWORD").unwrap_or_default(),
        })
    }
}

/// Spawn the reconnecting tracker task. `coop` maps an AP slot name to an extra
/// Bifrost-slot name (co-op seats), used when seeding a new season.
pub fn spawn(
    board: Arc<RwLock<Board>>,
    cfg: ApConfig,
    db: crate::db::Db,
    coop: Vec<(String, String)>,
) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = run_once(&board, &cfg, &db, &coop).await {
                tracing::warn!("AP tracker disconnected: {e}");
            }
            if let Ok(mut b) = board.write() {
                b.connected = false;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

async fn run_once(
    board: &Arc<RwLock<Board>>,
    cfg: &ApConfig,
    db: &crate::db::Db,
    coop: &[(String, String)],
) -> Result<(), String> {
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
                    if let Ok(mut b) = board.write() {
                        if let Some(seed) = pkt.get("seed_name").and_then(Value::as_str) {
                            b.seed = Some(seed.to_string());
                        }
                        if let Some(hc) = pkt.get("hint_cost").and_then(Value::as_i64) {
                            b.hint_cost = hc;
                        }
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
                    // Seed this season's Bifrost slots (1:1 + co-op extras).
                    let (season, ap_slots) = {
                        let b = board.read().map_err(|_| "lock")?;
                        (
                            b.seed.clone(),
                            b.slots
                                .iter()
                                .map(|(n, s)| (*n, s.name.clone(), s.game.clone()))
                                .collect::<Vec<_>>(),
                        )
                    };
                    if let Some(season) = season {
                        match db.seed_season(&season, &ap_slots, coop).await {
                            Ok(()) => {}
                            Err(e) => tracing::warn!("season seed failed: {e}"),
                        }
                    }
                    // Load season metadata (item names + per-slot totals) in the
                    // background — it decodes the seed via a Python subprocess.
                    let bclone = board.clone();
                    tokio::spawn(async move {
                        match crate::meta::load().await {
                            Ok(m) => {
                                if let Ok(mut b) = bclone.write() {
                                    b.meta = Some(std::sync::Arc::new(m));
                                }
                                tracing::info!("season metadata loaded");
                            }
                            Err(e) => tracing::warn!("meta load failed: {e}"),
                        }
                    });
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
        let list: Vec<HintRaw> = value
            .as_array()
            .map(|a| a.iter().filter_map(parse_hint).collect())
            .unwrap_or_default();
        if let Some(s) = b.slots.get_mut(&slot) {
            s.hint_list = list;
        }
    }
}

/// Parse a stored Hint, tolerating either the object or NamedTuple-as-array form.
/// Array order is (receiving, finding, location, item, found, …).
fn parse_hint(v: &Value) -> Option<HintRaw> {
    if let Some(o) = v.as_object() {
        Some(HintRaw {
            receiving: o.get("receiving_player").and_then(Value::as_i64)?,
            finding: o.get("finding_player").and_then(Value::as_i64)?,
            location: o.get("location").and_then(Value::as_i64).unwrap_or(0),
            item: o.get("item").and_then(Value::as_i64).unwrap_or(0),
            found: o.get("found").and_then(Value::as_bool).unwrap_or(false),
        })
    } else if let Some(a) = v.as_array() {
        Some(HintRaw {
            receiving: a.first().and_then(Value::as_i64)?,
            finding: a.get(1).and_then(Value::as_i64)?,
            location: a.get(2).and_then(Value::as_i64).unwrap_or(0),
            item: a.get(3).and_then(Value::as_i64).unwrap_or(0),
            found: a.get(4).and_then(Value::as_bool).unwrap_or(false),
        })
    } else {
        None
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
            let item = pkt.get("item");
            let src = item
                .and_then(|i| i.get("player"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let dst = pkt.get("receiving").and_then(Value::as_i64).unwrap_or(0);
            let flags = item
                .and_then(|i| i.get("flags"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let item_id = item
                .and_then(|i| i.get("item"))
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
                // Resolve the item name if season metadata is loaded.
                let named = b.meta.as_ref().and_then(|m| {
                    let recv_game = b.slots.get(&dst)?.game.as_str();
                    m.item_name(recv_game, item_id).map(str::to_string)
                });
                let text = match named {
                    Some(it) if src == dst => format!("{sn} found their own {it}"),
                    Some(it) => format!("{sn} found {dn}'s {it}"),
                    None if src == dst => format!("{sn} found their own item"),
                    None => format!("{sn} → {dn}"),
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
        // Join/Part (connect/disconnect) are deliberately not fed into the
        // activity feed — clients flap on unstable networks and would drown it.
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

// ---- hinting (a short-lived connection AS the target AP slot) ----

/// Connect as `name` (the AP slot's connect name), run `!hint <item>`, and
/// return the reply's message parts — a `Hint` PrintJSON on success, a
/// `CommandResult` on rejection. The hint spends *that* slot's points, which is
/// why we connect as it rather than acting as admin.
pub async fn send_hint(
    url: &str,
    password: &str,
    name: &str,
    item: &str,
) -> Result<Vec<Value>, String> {
    let fut = async {
        let (ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        let (mut tx, mut rx) = ws.split();
        let mut said = false;
        while let Some(msg) = rx.next().await {
            let msg = msg.map_err(|e| format!("recv: {e}"))?;
            let text = match msg {
                Message::Text(t) => t.to_string(),
                Message::Ping(p) => {
                    let _ = tx.send(Message::Pong(p)).await;
                    continue;
                }
                Message::Close(_) => return Err("server closed".to_string()),
                _ => continue,
            };
            let packets: Vec<Value> = serde_json::from_str(&text).map_err(|_| "bad json")?;
            for pkt in packets {
                match pkt.get("cmd").and_then(Value::as_str).unwrap_or("") {
                    "RoomInfo" => {
                        let connect = json!([{
                            "cmd": "Connect", "game": "", "name": name, "uuid": "bifrost-hint",
                            "version": {"class": "Version", "major": 0, "minor": 6, "build": 7},
                            "items_handling": 0, "tags": ["TextOnly"], "slot_data": false,
                            "password": password,
                        }]);
                        tx.send(Message::Text(connect.to_string()))
                            .await
                            .map_err(|e| format!("send Connect: {e}"))?;
                    }
                    "ConnectionRefused" => {
                        let errs = pkt.get("errors").map(|e| e.to_string()).unwrap_or_default();
                        return Err(format!("connection refused: {errs}"));
                    }
                    "Connected" if !said => {
                        let say = json!([{ "cmd": "Say", "text": format!("!hint {item}") }]);
                        tx.send(Message::Text(say.to_string()))
                            .await
                            .map_err(|e| format!("send Say: {e}"))?;
                        said = true;
                    }
                    // A rejection comes back as CommandResult ("can't afford",
                    // "did you mean"); a *successful* hint comes back as a Hint
                    // PrintJSON (no CommandResult) — accept either, or we hang
                    // until the timeout on every hint that actually lands.
                    "PrintJSON"
                        if matches!(
                            pkt.get("type").and_then(Value::as_str),
                            Some("CommandResult") | Some("Hint")
                        ) =>
                    {
                        if let Some(data) = pkt.get("data").and_then(Value::as_array) {
                            return Ok(data.clone());
                        }
                    }
                    _ => {}
                }
            }
        }
        Err("no response".to_string())
    };
    tokio::time::timeout(Duration::from_secs(8), fut)
        .await
        .map_err(|_| "hint timed out".to_string())?
}

/// Run a server command as an admin, over a short-lived connection: connect as
/// `connect_name` (any valid slot), `!admin login <admin_password>`, then
/// `!admin <command>` (e.g. `/send Player Item`, `/hint Player Item`). Returns
/// each `CommandResult` line's message parts for the caller to render.
pub async fn send_admin(
    url: &str,
    connect_name: &str,
    password: &str,
    admin_password: &str,
    command: &str,
) -> Result<Vec<Vec<Value>>, String> {
    if admin_password.is_empty() {
        return Err("no server admin password configured (BIFROST_AP_ADMIN_PASSWORD)".into());
    }
    let fut = async {
        let (ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        let (mut tx, mut rx) = ws.split();
        let mut login_sent = false;
        let mut collecting = false;
        let mut out: Vec<Vec<Value>> = Vec::new();
        loop {
            // Once the command is issued, gather its output until a short idle gap.
            let next = if collecting {
                match tokio::time::timeout(Duration::from_millis(1500), rx.next()).await {
                    Ok(Some(m)) => m,
                    _ => break,
                }
            } else {
                match tokio::time::timeout(Duration::from_secs(8), rx.next()).await {
                    Ok(Some(m)) => m,
                    _ => return Err("admin timed out".to_string()),
                }
            };
            let msg = next.map_err(|e| format!("recv: {e}"))?;
            let text = match msg {
                Message::Text(t) => t.to_string(),
                Message::Ping(p) => {
                    let _ = tx.send(Message::Pong(p)).await;
                    continue;
                }
                Message::Close(_) => break,
                _ => continue,
            };
            let packets: Vec<Value> = serde_json::from_str(&text).map_err(|_| "bad json")?;
            for pkt in packets {
                match pkt.get("cmd").and_then(Value::as_str).unwrap_or("") {
                    "RoomInfo" => {
                        let connect = json!([{
                            "cmd": "Connect", "game": "", "name": connect_name, "uuid": "bifrost-admin",
                            "version": {"class": "Version", "major": 0, "minor": 6, "build": 7},
                            "items_handling": 0, "tags": ["TextOnly"], "slot_data": false,
                            "password": password,
                        }]);
                        tx.send(Message::Text(connect.to_string()))
                            .await
                            .map_err(|e| format!("send Connect: {e}"))?;
                    }
                    "ConnectionRefused" => {
                        let errs = pkt.get("errors").map(|e| e.to_string()).unwrap_or_default();
                        return Err(format!("connection refused: {errs}"));
                    }
                    "Connected" if !login_sent => {
                        let say = json!([{ "cmd": "Say", "text": format!("!admin login {admin_password}") }]);
                        tx.send(Message::Text(say.to_string()))
                            .await
                            .map_err(|e| format!("send login: {e}"))?;
                        login_sent = true;
                    }
                    "PrintJSON" => {
                        let ty = pkt.get("type").and_then(Value::as_str).unwrap_or("");
                        let Some(data) = pkt.get("data").and_then(Value::as_array) else {
                            continue;
                        };
                        if collecting {
                            // Server command output comes back as AdminCommandResult.
                            if ty == "AdminCommandResult" || ty == "CommandResult" {
                                out.push(data.clone());
                            }
                        } else if ty == "CommandResult" {
                            // The login reply (from the client command processor).
                            let txt: String = data
                                .iter()
                                .filter_map(|d| d.get("text").and_then(Value::as_str))
                                .collect();
                            if txt.to_lowercase().contains("successful") {
                                let say =
                                    json!([{ "cmd": "Say", "text": format!("!admin {command}") }]);
                                tx.send(Message::Text(say.to_string()))
                                    .await
                                    .map_err(|e| format!("send command: {e}"))?;
                                collecting = true;
                            } else {
                                return Err(format!("admin login failed: {txt}"));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(out)
    };
    tokio::time::timeout(Duration::from_secs(14), fut)
        .await
        .map_err(|_| "admin timed out".to_string())?
}

/// Resolve a PrintJSON `data` list into readable text, using the board's slot
/// names + the season metadata for item/location names.
pub fn resolve_parts(b: &Board, parts: &[Value]) -> String {
    let mut out = String::new();
    for p in parts {
        let ty = p.get("type").and_then(Value::as_str).unwrap_or("text");
        let text = p.get("text").and_then(Value::as_str).unwrap_or("");
        match ty {
            "player_id" => out.push_str(&b.name_of(text.parse().unwrap_or(0))),
            "item_id" | "location_id" => {
                let owner = p.get("player").and_then(Value::as_i64).unwrap_or(0);
                let id: i64 = text.parse().unwrap_or(0);
                let game = b.slots.get(&owner).map(|x| x.game.as_str()).unwrap_or("");
                let resolved = b.meta.as_ref().and_then(|m| {
                    if ty == "item_id" {
                        m.item_name(game, id)
                    } else {
                        m.location_name(game, id)
                    }
                });
                out.push_str(resolved.unwrap_or(text));
            }
            _ => out.push_str(text),
        }
    }
    out
}

/// Read a slot's current hint points by briefly connecting as it (the points
/// arrive in `Connected`). Returns None on any failure/timeout.
pub async fn slot_points(url: &str, password: &str, name: &str) -> Option<i64> {
    let fut = async {
        let (ws, _) = tokio_tungstenite::connect_async(url).await.ok()?;
        let (mut tx, mut rx) = ws.split();
        while let Some(msg) = rx.next().await {
            let text = match msg.ok()? {
                Message::Text(t) => t.to_string(),
                Message::Ping(p) => {
                    let _ = tx.send(Message::Pong(p)).await;
                    continue;
                }
                Message::Close(_) => return None,
                _ => continue,
            };
            let packets: Vec<Value> = serde_json::from_str(&text).ok()?;
            for pkt in packets {
                match pkt.get("cmd").and_then(Value::as_str).unwrap_or("") {
                    "RoomInfo" => {
                        let connect = json!([{
                            "cmd": "Connect", "game": "", "name": name, "uuid": "bifrost-points",
                            "version": {"class": "Version", "major": 0, "minor": 6, "build": 7},
                            "items_handling": 0, "tags": ["TextOnly"], "slot_data": false,
                            "password": password,
                        }]);
                        tx.send(Message::Text(connect.to_string())).await.ok()?;
                    }
                    "ConnectionRefused" => return None,
                    "Connected" => {
                        return pkt.get("hint_points").and_then(Value::as_i64);
                    }
                    _ => {}
                }
            }
        }
        None
    };
    tokio::time::timeout(Duration::from_secs(6), fut)
        .await
        .ok()
        .flatten()
}
