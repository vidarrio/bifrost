//! Integration test: the engine runs PopTracker's `ap-storage-example` pack
//! (real `init.lua` + `autotracking.lua`) under the sandboxed host, the
//! Archipelago handlers set item state from fed AP data, and location
//! accessibility reacts correctly — including the `$lua` rule escape hatch.

use poptracker::{Pack, Session};
use std::collections::HashMap;
use std::path::Path;

fn fixture() -> Pack {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ap-storage");
    Pack::load(dir).expect("load fixture pack")
}

fn accessibility(s: &Session) -> HashMap<String, bool> {
    s.state()
        .locations
        .into_iter()
        .map(|l| (l.name, l.accessible))
        .collect()
}

#[test]
fn full_items_make_locations_reachable() {
    let s = Session::new(&fixture()).unwrap();
    s.on_clear(&serde_json::json!({})).unwrap();
    s.on_retrieved("a", &serde_json::json!(1)).unwrap(); // toggle on
    s.on_retrieved("b", &serde_json::json!(5)).unwrap(); // consumable = 5
    s.on_retrieved("c", &serde_json::json!([true, 1])).unwrap(); // progressive stage 1 -> c2

    let st = s.state();
    let b = st.items.iter().find(|i| i.key == "b").unwrap();
    assert_eq!(b.count, 5, "consumable count set by the pack's Lua");
    let c = st.items.iter().find(|i| i.key == "c").unwrap();
    assert!(c.active && c.stage == 1, "progressive item active at stage 1");

    let acc = accessibility(&s);
    assert!(acc["Tests/NeedsA"], "a present");
    assert!(acc["Tests/NeedsAandB2"], "a AND b>=2");
    assert!(acc["Tests/NeedsC2orA"], "c2 (stage 1) present");
    assert!(acc["Tests/NeedsLuaRule"], "$has_enough_b: b(5) >= 3");
}

#[test]
fn no_items_make_locations_unreachable() {
    let s = Session::new(&fixture()).unwrap();
    s.on_clear(&serde_json::json!({})).unwrap();
    s.on_retrieved("a", &serde_json::json!(0)).unwrap();
    s.on_retrieved("b", &serde_json::json!(1)).unwrap();
    s.on_retrieved("c", &serde_json::json!([false, 0])).unwrap();

    let acc = accessibility(&s);
    assert!(!acc["Tests/NeedsA"]);
    assert!(!acc["Tests/NeedsAandB2"]);
    assert!(!acc["Tests/NeedsC2orA"]);
    assert!(!acc["Tests/NeedsLuaRule"], "$has_enough_b: b(1) < 3");
}
