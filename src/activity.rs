//! Persistent activity log, sourced from the AP server's systemd journal.
//!
//! The journal is the one source that is timestamped, name-resolved, survives
//! Bifrost restarts, and reaches back over the whole season — so a single
//! ingester covers persistence, catch-up after downtime, and the current
//! season's retroactive backfill. We read it with `-o json` (structured
//! `__CURSOR` / `__REALTIME_TIMESTAMP` / `MESSAGE`), track the cursor so ingest
//! is exactly-once, and keep only real gameplay — item sends, goals, releases,
//! collects — filtering out joins/leaves/connections and all server chatter.
//!
//! Rows are tagged with the current season (AP seed) and queried back by it.
//! Relative "x ago" and the archive's frozen reference are handled at render.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::process::Command;

use crate::ap::Board;
use crate::db::Db;

const UNIT: &str = "archipelago";
const POLL: Duration = Duration::from_secs(5);
const CURRENT_ZIP: &str = "/srv/archipelago/output/current.zip";

/// Spawn the journal ingester. Idles until the board knows a season (seed).
pub fn spawn(board: Arc<RwLock<Board>>, db: Db) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = ingest(&board, &db).await {
                tracing::warn!("activity ingest: {e}");
            }
            tokio::time::sleep(POLL).await;
        }
    });
}

enum Mode {
    /// Resume from the saved cursor (normal steady state + catch-up on restart).
    Incremental(String),
    /// First run for this season: pull history after the previous season's end.
    Backfill(i64),
    /// We have rows but lost the cursor: seek to the journal tail, insert nothing.
    SeekEnd,
}

async fn ingest(board: &Arc<RwLock<Board>>, db: &Db) -> Result<(), String> {
    let season = match board.read().ok().and_then(|b| b.seed.clone()) {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(()), // no season yet — nothing to scope events to
    };

    // Season metadata (decoded seed) classifies a sent item's importance from
    // its (sender, location). Loaded async on tracker connect.
    let meta = board.read().ok().and_then(|b| b.meta.clone());

    let mode = match db.journal_cursor().await {
        Some(c) => Mode::Incremental(c),
        None if db.activity_any(&season).await => Mode::SeekEnd,
        // One-time retroactive backfill of the current season. The current
        // season began no earlier than the previous one ended; when no season
        // has been archived yet, fall back to the current seed's deploy time
        // (a new season always writes a fresh current.zip) so we don't sweep in
        // earlier seasons still present in the journal.
        None => Mode::Backfill(db.last_finalized_at().await.max(current_zip_mtime())),
    };

    // The one-time backfill waits for meta so historical sends get classified;
    // steady-state incremental proceeds regardless (meta is loaded post-connect,
    // and an unclassifiable send just gets no colour rather than blocking the log).
    if matches!(mode, Mode::Backfill(_)) && meta.is_none() {
        return Ok(());
    }

    let mut cmd = Command::new("journalctl");
    cmd.args(["-u", UNIT, "-o", "json", "--no-pager"]);
    match &mode {
        Mode::Incremental(c) => {
            cmd.arg(format!("--after-cursor={c}"));
        }
        Mode::Backfill(boundary) => {
            cmd.arg(format!("--since=@{boundary}"));
        }
        Mode::SeekEnd => {
            cmd.args(["-n", "1"]);
        }
    }
    cmd.stdin(std::process::Stdio::null());

    let out = cmd.output().await.map_err(|e| format!("journalctl spawn: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "journalctl exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let boundary = match &mode {
        Mode::Backfill(b) => *b,
        _ => 0,
    };
    let insert = !matches!(mode, Mode::SeekEnd);

    let mut last_cursor: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(c) = v.get("__CURSOR").and_then(|c| c.as_str()) {
            last_cursor = Some(c.to_string());
        }
        if !insert {
            continue;
        }
        let msg = match v.get("MESSAGE").and_then(|m| m.as_str()) {
            Some(m) => m,
            None => continue, // non-string MESSAGE (binary) — never a gameplay line
        };
        let ts = v
            .get("__REALTIME_TIMESTAMP")
            .and_then(|t| t.as_str())
            .and_then(|t| t.parse::<i64>().ok())
            .map(|us| us / 1_000_000)
            .unwrap_or(0);
        if ts <= boundary {
            continue; // belt-and-suspenders with --since on backfill
        }
        if let Some((kind, body, sender, receiver, location)) = classify(msg) {
            // Classify a sent item's importance from (sender, location) via meta.
            let class = match (kind, &sender, &location, &meta) {
                ("send", Some(s), Some(loc), Some(m)) => m.item_flags(s, loc).map(item_class),
                _ => None,
            };
            db.activity_add(
                &season,
                ts,
                kind,
                &body,
                sender.as_deref(),
                receiver.as_deref(),
                class,
            )
            .await;
        }
    }

    if let Some(c) = last_cursor {
        db.set_journal_cursor(&c).await;
    }
    Ok(())
}

/// Deploy time of the current season's seed, as epoch seconds (0 if unknown).
fn current_zip_mtime() -> i64 {
    std::fs::metadata(CURRENT_ZIP)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Item classification from AP flag bits: trap (0b100) > progression (0b001) >
/// useful (0b010) > filler (0).
fn item_class(flags: i64) -> &'static str {
    if flags & 0b100 != 0 {
        "trap"
    } else if flags & 0b001 != 0 {
        "progression"
    } else if flags & 0b010 != 0 {
        "useful"
    } else {
        "filler"
    }
}

/// Classify one AP server console line into a kept activity entry
/// `(kind, display text, sender, receiver, location)`, or `None` to drop it
/// (joins/leaves/connections/hints/command echoes/server chatter). `sender` is
/// the actor; `receiver` is the item recipient and `location` the found spot
/// (sends only, else `None` — the location feeds item-importance classification).
type Classified = (
    &'static str,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);
fn classify(msg: &str) -> Option<Classified> {
    // Item send: "(Team #N) SENDER sent ITEM to RECEIVER (LOCATION)".
    if let Some(rest) = msg.strip_prefix("(Team #") {
        let after = rest.split_once(") ")?.1; // drop "N) "
        let (sender, tail) = after.split_once(" sent ")?; // tail = "ITEM to RECEIVER (LOC)"
        let (item_recv, loc) = split_trailing_paren(tail)?; // strip "(LOCATION)"
        let (item, receiver) = item_recv.rsplit_once(" to ")?;
        let (sender, receiver, item) = (sender.trim(), receiver.trim(), item.trim());
        return Some((
            "send",
            format!("{sender} → {receiver}: {item}"),
            Some(sender.to_string()),
            Some(receiver.to_string()),
            Some(loc.trim().to_string()),
        ));
    }
    // Goal / release / collect: "Notice (all): NAME (Team #N) has <...>."
    if let Some(rest) = msg.strip_prefix("Notice (all): ") {
        let (name, tail) = rest.split_once(" (Team #")?; // tail = "N) has ..."
        let what = tail.split_once(") ")?.1;
        let name = name.trim();
        let actor = || Some(name.to_string());
        if what.starts_with("has completed their goal") {
            return Some(("goal", format!("{name} reached their goal!"), actor(), None, None));
        }
        if what.starts_with("has released all remaining items") {
            return Some((
                "release",
                format!("{name} released their remaining items"),
                actor(),
                None,
                None,
            ));
        }
        if what.starts_with("has collected their items") {
            return Some((
                "collect",
                format!("{name} collected their remaining items"),
                actor(),
                None,
                None,
            ));
        }
    }
    None
}

/// Split `s` into (before, inside) where `inside` is the content of the final
/// balanced `(...)` group and `before` is everything ahead of it (trimmed).
/// Handles nested parens in the group, e.g. a location named "Science (Durian)".
fn split_trailing_paren(s: &str) -> Option<(&str, &str)> {
    let b = s.as_bytes();
    if b.last() != Some(&b')') {
        return None;
    }
    let mut depth = 0i32;
    let mut i = b.len();
    while i > 0 {
        i -= 1;
        match b[i] {
            b')' => depth += 1,
            b'(' => {
                depth -= 1;
                if depth == 0 {
                    return Some((s[..i].trim_end(), &s[i + 1..b.len() - 1]));
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_item_send() {
        let (k, t, s, r, loc) =
            classify("(Team #1) HunkOfJunk sent Floorings to Cam (Trade Location 8)").unwrap();
        assert_eq!(k, "send");
        assert_eq!(t, "HunkOfJunk → Cam: Floorings");
        assert_eq!(s.as_deref(), Some("HunkOfJunk"));
        assert_eq!(r.as_deref(), Some("Cam"));
        assert_eq!(loc.as_deref(), Some("Trade Location 8"));
    }

    #[test]
    fn parses_send_with_nested_paren_location() {
        let (k, t, s, r, loc) =
            classify("(Team #1) Cam sent Sniper Covert to Deh (Science (Durian))").unwrap();
        assert_eq!(k, "send");
        assert_eq!(t, "Cam → Deh: Sniper Covert");
        assert_eq!((s.as_deref(), r.as_deref()), (Some("Cam"), Some("Deh")));
        assert_eq!(loc.as_deref(), Some("Science (Durian)"));
    }

    #[test]
    fn parses_goal_release_collect() {
        let (k, t, s, r, _) = classify("Notice (all): Cam (Team #1) has completed their goal.").unwrap();
        assert_eq!((k, t.as_str(), s.as_deref(), r), ("goal", "Cam reached their goal!", Some("Cam"), None));
        let g = classify("Notice (all): Elle (Team #1) has released all remaining items from their world.").unwrap();
        assert_eq!((g.0, g.2.as_deref(), g.3), ("release", Some("Elle"), None));
        let c = classify("Notice (all): Dustbiter (Team #1) has collected their items from other worlds.").unwrap();
        assert_eq!((c.0, c.2.as_deref()), ("collect", Some("Dustbiter")));
    }

    #[test]
    fn item_classes() {
        assert_eq!(item_class(0b001), "progression");
        assert_eq!(item_class(0b010), "useful");
        assert_eq!(item_class(0b100), "trap");
        assert_eq!(item_class(0), "filler");
        assert_eq!(item_class(0b101), "trap"); // trap bit wins
        assert_eq!(item_class(0b1001), "progression"); // progression_skip_balancing
    }

    #[test]
    fn drops_noise() {
        assert!(classify("Notice (all): Deh (Team #1) viewing HITMAN has joined. Client(0.6.8), ['TextOnly'].").is_none());
        assert!(classify("Notice (all): HunkOfJunk (Team #1) has left the game. Client(0.6.0), [].").is_none());
        assert!(classify("Notice (Team #1): [Hint]: Deh's Level is at X in Deh's World. (priority)").is_none());
        assert!(classify("Notice (all): Deh: !hint Level - New York").is_none());
        assert!(classify("connection closed").is_none());
    }
}
