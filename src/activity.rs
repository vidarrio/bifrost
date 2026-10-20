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
        if let Some((kind, body)) = classify(msg) {
            db.activity_add(&season, ts, kind, &body).await;
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

/// Classify one AP server console line into a kept activity entry, or `None` to
/// drop it (joins/leaves/connections/hints/command echoes/server chatter).
fn classify(msg: &str) -> Option<(&'static str, String)> {
    // Item send: "(Team #N) SENDER sent ITEM to RECEIVER (LOCATION)".
    if let Some(rest) = msg.strip_prefix("(Team #") {
        let after = rest.split_once(") ")?.1; // drop "N) "
        let (sender, tail) = after.split_once(" sent ")?; // tail = "ITEM to RECEIVER (LOC)"
        let (item_recv, _loc) = split_trailing_paren(tail)?; // strip "(LOCATION)"
        let (item, receiver) = item_recv.rsplit_once(" to ")?;
        return Some((
            "send",
            format!("{} → {}: {}", sender.trim(), receiver.trim(), item.trim()),
        ));
    }
    // Goal / release / collect: "Notice (all): NAME (Team #N) has <...>."
    if let Some(rest) = msg.strip_prefix("Notice (all): ") {
        let (name, tail) = rest.split_once(" (Team #")?; // tail = "N) has ..."
        let what = tail.split_once(") ")?.1;
        let name = name.trim();
        if what.starts_with("has completed their goal") {
            return Some(("goal", format!("{name} reached their goal!")));
        }
        if what.starts_with("has released all remaining items") {
            return Some(("release", format!("{name} released their remaining items")));
        }
        if what.starts_with("has collected their items") {
            return Some(("collect", format!("{name} collected their remaining items")));
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
        let (k, t) = classify("(Team #1) HunkOfJunk sent Floorings to Cam (Trade Location 8)").unwrap();
        assert_eq!(k, "send");
        assert_eq!(t, "HunkOfJunk → Cam: Floorings");
    }

    #[test]
    fn parses_send_with_nested_paren_location() {
        let (k, t) =
            classify("(Team #1) Cam sent Sniper Covert to Deh (Science (Durian))").unwrap();
        assert_eq!(k, "send");
        assert_eq!(t, "Cam → Deh: Sniper Covert");
    }

    #[test]
    fn parses_goal_release_collect() {
        assert_eq!(
            classify("Notice (all): Cam (Team #1) has completed their goal.").unwrap(),
            ("goal", "Cam reached their goal!".to_string())
        );
        assert_eq!(
            classify("Notice (all): Elle (Team #1) has released all remaining items from their world.")
                .unwrap()
                .0,
            "release"
        );
        assert_eq!(
            classify("Notice (all): Dustbiter (Team #1) has collected their items from other worlds.")
                .unwrap()
                .0,
            "collect"
        );
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
