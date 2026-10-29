//! Archipelago tracker client + live board state.
//!
//! Bifrost holds one persistent WebSocket connection to the MultiServer as a
//! `Tracker`-tagged client. From it we learn every slot (via `Connected`) and
//! subscribe to each slot's status + hints in the server's data storage
//! (`_read_client_status_*` / `_read_hints_*`). The persistent, timestamped
//! activity log is sourced separately from the server journal (see
//! `activity.rs`); the only live `PrintJSON` handling left here is flipping a
//! slot to "goaled" immediately so the board doesn't wait for the next poll.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

/// The key that marks a connection as Bifrost's own: Bifrost's Archipelago
/// fork doesn't announce clients that present it (`bifrost_key` in Connect)
/// joining, leaving or using `!admin` (`packaging/archipelago/patches/`).
/// Random per process and handed only to the AP servers Bifrost runs (env
/// `BIFROST_QUIET_KEY`), so it's never sent to an external server. Not the
/// admin password: the in-logic helper, which runs apworld code, gets it too.
pub fn quiet_key() -> Option<&'static str> {
    static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    crate::config::get()
        .ap_managed
        .then(|| KEY.get_or_init(crate::auth::new_token).as_str())
}

/// Add the quiet key to a `[Connect]` message, when there is one.
fn keyed(mut connect: Value) -> Value {
    if let (Some(key), Some(c)) = (
        quiet_key(),
        connect.get_mut(0).and_then(Value::as_object_mut),
    ) {
        c.insert("bifrost_key".into(), key.into());
    }
    connect
}

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
    /// Every item this slot has been granted, as (item, location, finder) —
    /// from the savefile (see the counts refresher). Rendered as the game
    /// page's "found items" list in the same format as hints.
    pub received_items: Vec<(i64, i64, i64)>,
    /// Location ids this slot has checked (savefile `location_checks`), fed to
    /// the PopTracker engine to grey out collected locations.
    pub checked_locations: Vec<i64>,
}

#[derive(Default)]
pub struct Board {
    pub connected: bool,
    pub team: i64,
    pub seed: Option<String>,
    /// Percentage of a world's locations that must be checked to afford a hint.
    pub hint_cost: i64,
    pub slots: BTreeMap<i64, SlotState>,
    pub meta: Option<std::sync::Arc<crate::meta::Meta>>,
}

impl Board {
    pub fn name_of(&self, slot: i64) -> String {
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
    /// This slot's items that no one has found yet (items_for − received);
    /// decreases toward 0 as the run completes.
    pub still_to_find: u32,
    /// Hints for items hinted to be in this slot's world (with identity).
    pub hint_views: Vec<HintView>,
    /// Every item this slot has received, as found HintViews (item @ location ·
    /// finder ✓) — the complete "found" log, a superset of the found hints.
    pub found_items: Vec<HintView>,
    /// Hints for items located in THIS slot's world that belong to someone else
    /// (finding == this slot), oriented "item → receiver @ location". Used by the
    /// board so a prioritized hint showcases on the FINDER's card — "what others
    /// want from you" — rather than the receiver's.
    pub finder_views: Vec<HintView>,
}

/// An owned copy of what the pages need to render, taken under one short lock.
pub struct BoardSnapshot {
    pub connected: bool,
    pub seed: Option<String>,
    pub hint_cost: i64,
    pub ap: BTreeMap<i64, ApLive>,
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

    /// Locations of this slot's items that are already "done" — hinted (in the
    /// slot's own hints) or found (received). Keyed by (finder, location) since a
    /// location id is only unique within its finder's world.
    fn accounted_locs(&self, ap_slot: i64) -> HashSet<(i64, i64)> {
        let mut set = HashSet::new();
        if let Some(s) = self.slots.get(&ap_slot) {
            for h in s.hint_list.iter().filter(|h| h.receiving == ap_slot) {
                set.insert((h.finding, h.location));
            }
            for (_item, loc, finder) in &s.received_items {
                set.insert((*finder, *loc));
            }
        }
        set
    }

    /// Item names to offer in the slot's hint search: the slot's own items that
    /// still have at least one copy neither hinted nor found. A name whose every
    /// copy is accounted for drops off the list.
    pub fn hint_suggestions(&self, ap_slot: i64, game: &str) -> Vec<String> {
        let Some(m) = self.meta.as_ref() else {
            return Vec::new();
        };
        let Some(placements) = m.placements(ap_slot) else {
            return Vec::new();
        };
        let acc = self.accounted_locs(ap_slot);
        let mut names: Vec<String> = placements
            .iter()
            .filter(|(_, locs)| locs.iter().any(|a| !acc.contains(&(a[0], a[1]))))
            .filter_map(|(item_id, _)| m.item_name(game, *item_id).map(str::to_string))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// For an admin `/send <player> <item>` of an item that has an active
    /// (unfound) hint, the `/send_location <finder> <location>` that delivers it
    /// from its real spot — which also checks that location, so the hint resolves
    /// to "found". Returns the (finder connect-name, location name). Plain `/send`
    /// instead grants a duplicate and leaves the location (and hint) untouched.
    pub fn send_to_location(&self, player_arg: &str, item_arg: &str) -> Option<(String, String)> {
        let m = self.meta.as_ref()?;
        // Resolve the player to a slot by connect name (exact, case-insensitive).
        let (&pslot, pstate) = self
            .slots
            .iter()
            .find(|(_, s)| s.name.eq_ignore_ascii_case(player_arg))?;
        // Resolve the item id within that player's game (exact, case-insensitive).
        let item_id = m.item_id_by_name(&pstate.game, item_arg).or_else(|| {
            m.item_names
                .get(&pstate.game)?
                .iter()
                .find(|(_, n)| n.eq_ignore_ascii_case(item_arg))
                .map(|(id, _)| *id)
        })?;
        // First active (unfound) hint for this item going to this player.
        let h = pstate
            .hint_list
            .iter()
            .find(|h| h.receiving == pslot && h.item == item_id && !h.found)?;
        let finder_name = self.slots.get(&h.finding)?.name.clone();
        let finder_game = &self.slots.get(&h.finding)?.game;
        let loc_name = m.location_name(finder_game, h.location)?.to_string();
        Some((finder_name, loc_name))
    }

    /// The first copy of `item_id` (in placement order) that hasn't been hinted
    /// or found — as (finder_slot, location_id). Used to hint exactly one copy.
    pub fn first_unhinted_location(&self, ap_slot: i64, item_id: i64) -> Option<(i64, i64)> {
        let m = self.meta.as_ref()?;
        let locs = m.placements(ap_slot)?.get(&item_id)?;
        let acc = self.accounted_locs(ap_slot);
        locs.iter().map(|a| (a[0], a[1])).find(|p| !acc.contains(p))
    }

    pub fn snapshot(&self) -> BoardSnapshot {
        let ap = self
            .slots
            .iter()
            .map(|(n, s)| {
                let (world_size, items_for) = self
                    .meta
                    .as_ref()
                    .and_then(|m| m.totals(*n))
                    .map(|t| (t.world_size, t.items_for))
                    .unwrap_or((0, 0));
                // Items of this slot that nobody has found yet.
                let still_to_find = items_for.saturating_sub(s.received as u32);
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
                // Every item this slot has received, rendered like a found hint
                // ("item @ location · finder ✓"): item named in this slot's game,
                // location in the finder's game.
                let mut found_items = Vec::new();
                if let Some(m) = self.meta.as_ref() {
                    for (item, location, finder) in &s.received_items {
                        let item_name = m.item_name(&s.game, *item).unwrap_or("?");
                        let find_game = self
                            .slots
                            .get(finder)
                            .map(|x| x.game.as_str())
                            .unwrap_or("");
                        let loc = m.location_name(find_game, *location).unwrap_or("?");
                        let finder_name = self.name_of(*finder);
                        found_items.push(HintView {
                            finding: *finder,
                            location: *location,
                            item: *item,
                            receiving: *n,
                            found: true,
                            text: format!("{item_name} @ {loc} · {finder_name} ✓"),
                        });
                    }
                }
                // Finder-oriented: items located in THIS slot's world that belong
                // to someone else (finding == this slot). Item is named in the
                // RECEIVER's game, the location in this (the finder's) own game.
                // Rendered "item → receiver @ location" so the board can showcase
                // a prioritized hint on the finder's card.
                let mut finder_views = Vec::new();
                if let Some(m) = self.meta.as_ref() {
                    for h in s.hint_list.iter().filter(|h| h.finding == *n) {
                        let recv_game = self
                            .slots
                            .get(&h.receiving)
                            .map(|x| x.game.as_str())
                            .unwrap_or("");
                        let item = m.item_name(recv_game, h.item).unwrap_or("?");
                        let loc = m.location_name(&s.game, h.location).unwrap_or("?");
                        let receiver = self.name_of(h.receiving);
                        let mark = if h.found { " ✓" } else { "" };
                        finder_views.push(HintView {
                            finding: h.finding,
                            location: h.location,
                            item: h.item,
                            receiving: h.receiving,
                            found: h.found,
                            text: format!("{item} → {receiver} @ {loc}{mark}"),
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
                        still_to_find,
                        hint_views,
                        found_items,
                        finder_views,
                    },
                )
            })
            .collect();
        BoardSnapshot {
            connected: self.connected,
            seed: self.seed.clone(),
            hint_cost: self.hint_cost,
            ap,
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
        use crate::config::var;
        let slot = var("BIFROST_SLOT")?;
        Some(Self {
            url: var("BIFROST_AP_URL").unwrap_or_else(|| "ws://127.0.0.1:38281".into()),
            slot,
            password: var("BIFROST_AP_PASSWORD").unwrap_or_default(),
            admin_password: var("BIFROST_AP_ADMIN_PASSWORD").unwrap_or_default(),
        })
    }
}

/// Spawn the reconnecting tracker task (and the savefile counts refresher)
/// for `season`; returns their handles so the season can stop them. `coop`
/// maps an AP slot name to an extra Bifrost-slot name (co-op seats), used when
/// seeding the season's slots.
pub fn spawn(
    season: i64,
    board: Arc<RwLock<Board>>,
    cfg: ApConfig,
    db: crate::db::Db,
    coop: Vec<(String, String)>,
) -> Vec<tokio::task::JoinHandle<()>> {
    // Refresh authoritative sent/received totals from the server savefile. An
    // external tracker only sees live item events, so these counts would reset
    // on every reconnect and undercount a mid-season board; the savefile holds
    // the true tallies (location_checks / received_items). Runs independently of
    // the tracker connection so it survives reconnects.
    let counts = {
        let board = board.clone();
        tokio::spawn(async move {
            loop {
                match crate::meta::load_counts(season).await {
                    Ok(counts) if !counts.is_empty() => {
                        if let Ok(mut b) = board.write() {
                            for (slot, c) in counts {
                                if let Some(s) = b.slots.get_mut(&slot) {
                                    s.sent = c.sent;
                                    s.received = c.items.len() as u64;
                                    s.received_items =
                                        c.items.iter().map(|a| (a[0], a[1], a[2])).collect();
                                    s.checked_locations = c.checked;
                                }
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("counts refresh failed: {e}"),
                }
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
        })
    };

    let tracker = tokio::spawn(async move {
        loop {
            if let Err(e) = run_once(season, &board, &cfg, &db, &coop).await {
                tracing::warn!("AP tracker disconnected: {e}");
            }
            if let Ok(mut b) = board.write() {
                b.connected = false;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
    vec![counts, tracker]
}

async fn run_once(
    season: i64,
    board: &Arc<RwLock<Board>>,
    cfg: &ApConfig,
    db: &crate::db::Db,
    coop: &[(String, String)],
) -> Result<(), String> {
    // The tracker connects as one of the room's slots: the configured one,
    // else the season's first (the roster exists once it's generated).
    let slot = if cfg.slot.is_empty() {
        db.slots(season)
            .await
            .ok()
            .and_then(|s| s.into_iter().min_by_key(|r| r.ap_slot).map(|r| r.name))
            .ok_or("no slots yet to connect the tracker as")?
    } else {
        cfg.slot.clone()
    };
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
                        "name": slot,
                        "uuid": "bifrost-tracker",
                        "version": {"class": "Version", "major": 0, "minor": 6, "build": 8},
                        "items_handling": 0,
                        "tags": ["Tracker"],
                        "slot_data": false,
                        "password": cfg.password,
                    }]);
                    let connect = keyed(connect);
                    if let Ok(mut b) = board.write() {
                        if let Some(seed) = pkt.get("seed_name").and_then(Value::as_str) {
                            b.seed = Some(seed.to_string());
                        }
                        if let Some(hc) = pkt.get("hint_cost").and_then(Value::as_i64) {
                            b.hint_cost = hc;
                        }
                    }
                    tx.send(Message::Text(connect.to_string().into()))
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
                    tx.send(Message::Text(sub.to_string().into()))
                        .await
                        .map_err(|e| format!("send Get/SetNotify: {e}"))?;
                    // Seed this season's Bifrost slots (1:1 + co-op extras) —
                    // but only from the seed the season deployed: an AP server
                    // running some other world must not be attributed to it.
                    let (room_seed, ap_slots) = {
                        let b = board.read().map_err(|_| "lock")?;
                        (
                            b.seed.clone(),
                            b.slots
                                .iter()
                                .map(|(n, s)| (*n, s.name.clone(), s.game.clone()))
                                .collect::<Vec<_>>(),
                        )
                    };
                    let expected = db.season(season).await.and_then(|s| s.seed);
                    match (&room_seed, &expected) {
                        (Some(room), Some(want)) if room != want => {
                            return Err(format!(
                                "the AP server is running seed {room}, but this season deployed {want}"
                            ));
                        }
                        (Some(room), None) => {
                            // A world generated outside Bifrost: adopt its seed.
                            let _ = db.set_season_seed(season, room).await;
                        }
                        _ => {}
                    }
                    if room_seed.is_some()
                        && let Err(e) = db.seed_season(season, &ap_slots, coop).await
                    {
                        tracing::warn!("season seed failed: {e}");
                    }
                    // Load season metadata (item names + per-slot totals) in the
                    // background — it decodes the seed via a Python subprocess.
                    let bclone = board.clone();
                    tokio::spawn(async move {
                        match crate::meta::load(season).await {
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
        s.status = status;
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

/// The persistent activity log is sourced from the AP server journal (see
/// `activity.rs`). The live tracker's only remaining job for `PrintJSON` is to
/// flip a slot's status to "goaled" immediately, so the board updates without
/// waiting for the next periodic client-status poll. Everything else (item
/// sends, releases, collects, join/part) is handled by the journal ingester.
fn on_printjson(board: &Arc<RwLock<Board>>, pkt: &Value) {
    if pkt.get("type").and_then(Value::as_str) != Some("Goal") {
        return;
    }
    let Some(slot) = pkt.get("slot").and_then(Value::as_i64) else {
        return;
    };
    if let Ok(mut b) = board.write()
        && let Some(s) = b.slots.get_mut(&slot)
    {
        s.status = 30;
    }
}

/// Register a slot's goal (mark it "finished") the way its game client would,
/// over a short-lived connection AS that slot. Must connect with **empty tags**
/// (a Tracker/TextOnly connection is refused "Trackers can't register Goal
/// Complete") and the slot's real game. Sends `StatusUpdate{status:30}`; the
/// server then announces the goal and runs auto-collect/release per its modes.
/// Returns once the goal broadcast is seen (best-effort).
pub async fn mark_goal(url: &str, password: &str, name: &str, game: &str) -> Result<(), String> {
    const CLIENT_GOAL: i64 = 30;
    let fut = async {
        let (ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        let (mut tx, mut rx) = ws.split();
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
                            "cmd": "Connect", "game": game, "name": name, "uuid": "bifrost-goal",
                            "version": {"class": "Version", "major": 0, "minor": 6, "build": 8},
                            "items_handling": 7, "tags": [], "slot_data": false,
                            "password": password,
                        }]);
                        let connect = keyed(connect);
                        tx.send(Message::Text(connect.to_string().into()))
                            .await
                            .map_err(|e| format!("send Connect: {e}"))?;
                    }
                    "ConnectionRefused" => {
                        let errs = pkt.get("errors").map(|e| e.to_string()).unwrap_or_default();
                        return Err(format!("connection refused: {errs}"));
                    }
                    "Connected" => {
                        let upd = json!([{ "cmd": "StatusUpdate", "status": CLIENT_GOAL }]);
                        tx.send(Message::Text(upd.to_string().into()))
                            .await
                            .map_err(|e| format!("send StatusUpdate: {e}"))?;
                    }
                    "InvalidPacket" => {
                        let t = pkt.get("text").and_then(Value::as_str).unwrap_or("");
                        return Err(format!("rejected: {t}"));
                    }
                    "PrintJSON" if pkt.get("type").and_then(Value::as_str) == Some("Goal") => {
                        return Ok(()); // goal registered + announced
                    }
                    _ => {}
                }
            }
        }
        Err("no goal confirmation".to_string())
    };
    tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .map_err(|_| "mark-finished timed out".to_string())?
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
    // (finding_player, location) of a hint, from a stored-hint value (object or
    // NamedTuple array) or a Hint PrintJSON's NetworkItem `item` field.
    fn hint_fl(v: &Value) -> Option<(i64, i64)> {
        if let Some(o) = v.as_object() {
            Some((
                o.get("finding_player")?.as_i64()?,
                o.get("location")?.as_i64()?,
            ))
        } else if let Some(a) = v.as_array() {
            Some((a.get(1)?.as_i64()?, a.get(2)?.as_i64()?))
        } else {
            None
        }
    }
    fn packet_fl(pkt: &Value) -> Option<(i64, i64)> {
        let it = pkt.get("item")?;
        if let Some(o) = it.as_object() {
            Some((o.get("player")?.as_i64()?, o.get("location")?.as_i64()?))
        } else if let Some(a) = it.as_array() {
            Some((a.get(2)?.as_i64()?, a.get(1)?.as_i64()?))
        } else {
            None
        }
    }

    let fut = async {
        let (ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        let (mut tx, mut rx) = ws.split();
        let mut said = false;
        // Before hinting, snapshot which (finder, location) hints already exist so
        // we can tell AP's newly-placed copy from the ones it re-broadcasts. AP's
        // `!hint` re-notifies the existing hints *and* the already-found copies
        // alongside the one new copy it buys, so without this a repeat of the same
        // item name would report an old/found copy instead of the fresh one.
        let mut before: Option<HashSet<(i64, i64)>> = None;
        let mut hints_key = String::new();
        // A newly-placed *found* copy, kept only as a last resort.
        let mut new_found_fallback: Option<Vec<Value>> = None;
        let mut collecting = false;
        loop {
            let next = if collecting {
                match tokio::time::timeout(Duration::from_millis(1200), rx.next()).await {
                    Ok(x) => x,
                    Err(_) => break, // nothing more coming soon
                }
            } else {
                rx.next().await
            };
            let Some(msg) = next else { break };
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
                            "version": {"class": "Version", "major": 0, "minor": 6, "build": 8},
                            "items_handling": 0, "tags": ["TextOnly"], "slot_data": false,
                            "password": password,
                        }]);
                        let connect = keyed(connect);
                        tx.send(Message::Text(connect.to_string().into()))
                            .await
                            .map_err(|e| format!("send Connect: {e}"))?;
                    }
                    "ConnectionRefused" => {
                        let errs = pkt.get("errors").map(|e| e.to_string()).unwrap_or_default();
                        return Err(format!("connection refused: {errs}"));
                    }
                    "Connected" if !said => {
                        // Fetch our current hints first; hint only after the reply.
                        let slot = pkt.get("slot").and_then(Value::as_i64).unwrap_or(0);
                        hints_key = format!("_read_hints_0_{slot}");
                        let get = json!([{ "cmd": "Get", "keys": [hints_key] }]);
                        tx.send(Message::Text(get.to_string().into()))
                            .await
                            .map_err(|e| format!("send Get: {e}"))?;
                    }
                    "Retrieved" if !said => {
                        let set: HashSet<(i64, i64)> = pkt
                            .get("keys")
                            .and_then(|k| k.get(&hints_key))
                            .and_then(Value::as_array)
                            .map(|hs| hs.iter().filter_map(hint_fl).collect())
                            .unwrap_or_default();
                        before = Some(set);
                        let say = json!([{ "cmd": "Say", "text": format!("!hint {item}") }]);
                        tx.send(Message::Text(say.to_string().into()))
                            .await
                            .map_err(|e| format!("send Say: {e}"))?;
                        said = true;
                        collecting = true;
                    }
                    "PrintJSON" if pkt.get("type").and_then(Value::as_str) == Some("Hint") => {
                        let Some(data) = pkt.get("data").and_then(Value::as_array) else {
                            continue;
                        };
                        // Skip hints AP merely re-broadcast (already placed before).
                        let is_new = match (before.as_ref(), packet_fl(&pkt)) {
                            (Some(b), Some(fl)) => !b.contains(&fl),
                            _ => true,
                        };
                        if !is_new {
                            continue;
                        }
                        if pkt.get("found").and_then(Value::as_bool) == Some(true) {
                            new_found_fallback.get_or_insert_with(|| data.clone());
                        } else {
                            return Ok(data.clone()); // the freshly revealed copy
                        }
                    }
                    // CommandResult = end-of-hint info ("there may be more"),
                    // "previously used" (nothing new — already fully hinted), or a
                    // rejection. Surface a points shortfall verbatim so the
                    // bonus-hint fallback can run; otherwise prefer a new found copy
                    // if that's all we got, else the message itself.
                    "PrintJSON"
                        if pkt.get("type").and_then(Value::as_str) == Some("CommandResult") =>
                    {
                        let Some(data) = pkt.get("data").and_then(Value::as_array) else {
                            continue;
                        };
                        let raw: String = data
                            .iter()
                            .filter_map(|p| p.get("text").and_then(Value::as_str))
                            .collect::<String>()
                            .to_lowercase();
                        let shortfall = raw.contains("afford")
                            || raw.contains("not enough")
                            || raw.contains("do not have");
                        if shortfall || new_found_fallback.is_none() {
                            return Ok(data.clone());
                        }
                        return Ok(new_found_fallback.take().unwrap());
                    }
                    _ => {}
                }
            }
        }
        new_found_fallback.ok_or_else(|| "no response".to_string())
    };
    tokio::time::timeout(Duration::from_secs(8), fut)
        .await
        .map_err(|_| "hint timed out".to_string())?
}

/// Quote one argument for an AP server command, which the server splits with
/// Python's `shlex` (so a name with spaces or quotes stays one argument).
pub fn quote_arg(s: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c);
    if !s.is_empty() && s.chars().all(safe) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    }
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
    admin_exchange(url, connect_name, password, admin_password, command)
        .await
        .map(|out| {
            out.into_iter()
                .filter(|(ty, _)| ty != "Chat")
                .map(|(_, data)| data)
                .collect()
        })
        .map_err(|e| e.message)
}

/// Why an admin command failed, and whether it had been sent by then (after
/// which it may well have taken effect).
#[derive(Debug)]
pub struct AdminFail {
    pub message: String,
    pub sent: bool,
}

/// The server's replies to an admin command: the texts it sent back to us
/// alone — `AdminCommandResult` (e.g. "No hints found.") or, if the command
/// never ran, `CommandResult` (e.g. another admin session took over). Empty
/// when the command produced only broadcasts (as a placed hint does).
pub async fn send_admin_results(
    url: &str,
    connect_name: &str,
    password: &str,
    admin_password: &str,
    command: &str,
) -> Result<Vec<String>, AdminFail> {
    let out = admin_exchange(url, connect_name, password, admin_password, command).await?;
    Ok(out
        .into_iter()
        .filter(|(ty, _)| ty == "AdminCommandResult" || ty == "CommandResult")
        .map(|(_, data)| {
            data.iter()
                .filter_map(|d| d.get("text").and_then(Value::as_str))
                .collect()
        })
        .collect())
}

/// A lock per key (e.g. one per AP server), held across an async exchange.
pub async fn keyed_lock(key: &str) -> tokio::sync::OwnedMutexGuard<()> {
    static LOCKS: std::sync::LazyLock<
        std::sync::Mutex<HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::LazyLock::new(Default::default);
    let lock = {
        let mut map = LOCKS.lock().unwrap();
        // Drop locks nobody holds or waits for, so the map stays small.
        map.retain(|_, l| std::sync::Arc::strong_count(l) > 1);
        map.entry(key.to_string()).or_default().clone()
    };
    lock.lock_owned().await
}

/// Every PrintJSON (type, parts) the server sent after the command. One
/// exchange per server at a time: the server keeps a single admin session, so
/// a second login in between would leave the first command refused.
async fn admin_exchange(
    url: &str,
    connect_name: &str,
    password: &str,
    admin_password: &str,
    command: &str,
) -> Result<Vec<(String, Vec<Value>)>, AdminFail> {
    if admin_password.is_empty() {
        return Err(AdminFail {
            message: "no server admin password configured (BIFROST_AP_ADMIN_PASSWORD)".into(),
            sent: false,
        });
    }
    let _one_at_a_time = keyed_lock(&format!("admin:{url}")).await;
    let sent = std::sync::atomic::AtomicBool::new(false);
    let fut = async {
        let (ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        let (mut tx, mut rx) = ws.split();
        let mut login_sent = false;
        let mut collecting = false;
        let mut out: Vec<(String, Vec<Value>)> = Vec::new();
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
                            "version": {"class": "Version", "major": 0, "minor": 6, "build": 8},
                            "items_handling": 0, "tags": ["TextOnly"], "slot_data": false,
                            "password": password,
                        }]);
                        let connect = keyed(connect);
                        tx.send(Message::Text(connect.to_string().into()))
                            .await
                            .map_err(|e| format!("send Connect: {e}"))?;
                    }
                    "ConnectionRefused" => {
                        let errs = pkt.get("errors").map(|e| e.to_string()).unwrap_or_default();
                        return Err(format!("connection refused: {errs}"));
                    }
                    "Connected" if !login_sent => {
                        let say = json!([{ "cmd": "Say", "text": format!("!admin login {admin_password}") }]);
                        tx.send(Message::Text(say.to_string().into()))
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
                            // Command output comes back as AdminCommandResult, but
                            // many commands instead *broadcast* their effect: /send a
                            // plain text PrintJSON ("Cheat console: sending …"),
                            // /send_location an ItemSend, /hint a Hint, etc. Capture
                            // every broadcast except plain player Chat so the console
                            // shows what happened. (`!admin …` isn't echoed to chat.)
                            out.push((ty.to_string(), data.clone()));
                        } else if ty == "CommandResult" {
                            // The login reply (from the client command processor).
                            let txt: String = data
                                .iter()
                                .filter_map(|d| d.get("text").and_then(Value::as_str))
                                .collect();
                            if txt.to_lowercase().contains("successful") {
                                let say =
                                    json!([{ "cmd": "Say", "text": format!("!admin {command}") }]);
                                // From here on the command may have run.
                                sent.store(true, std::sync::atomic::Ordering::SeqCst);
                                tx.send(Message::Text(say.to_string().into()))
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
        Ok::<_, String>(out)
    };
    let fail = |message: String| AdminFail {
        message,
        sent: sent.load(std::sync::atomic::Ordering::SeqCst),
    };
    match tokio::time::timeout(Duration::from_secs(14), fut).await {
        Ok(r) => r.map_err(fail),
        Err(_) => Err(fail("admin timed out".to_string())),
    }
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
                            "version": {"class": "Version", "major": 0, "minor": 6, "build": 8},
                            "items_handling": 0, "tags": ["TextOnly"], "slot_data": false,
                            "password": password,
                        }]);
                        let connect = keyed(connect);
                        tx.send(Message::Text(connect.to_string().into()))
                            .await
                            .ok()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_command_args_stay_whole() {
        // AP splits commands with shlex: each stays one argument.
        assert_eq!(quote_arg("Ann"), "Ann");
        assert_eq!(quote_arg("Ann B"), "'Ann B'");
        assert_eq!(quote_arg("Link's House"), r#"'Link'"'"'s House'"#);
        assert_eq!(quote_arg(""), "''");
    }
}
