//! Integration test: the engine runs PopTracker's `ap-storage-example` pack
//! (real `init.lua` + `autotracking.lua`) under the sandboxed host, the
//! Archipelago handlers set item state from fed AP data, and location
//! accessibility reacts correctly — including the `$lua` rule escape hatch.

use poptracker::{Pack, Session};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

fn fixture_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ap-storage")
}

fn fixture() -> Pack {
    Pack::load(fixture_dir()).expect("load fixture pack")
}

/// Recursively add a directory's files to a zip under `prefix`.
fn zip_add(zw: &mut zip::ZipWriter<std::fs::File>, base: &Path, dir: &Path, prefix: &str) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        let rel = format!("{prefix}{}", p.strip_prefix(base).unwrap().to_string_lossy());
        if p.is_dir() {
            zip_add(zw, base, &p, prefix);
        } else {
            zw.start_file(&rel, zip::write::SimpleFileOptions::default()).unwrap();
            zw.write_all(&std::fs::read(&p).unwrap()).unwrap();
        }
    }
}

#[test]
fn summary_reports_ap_autotracking_and_counts() {
    let s = Session::new(&fixture()).unwrap();
    let sum = s.summary();
    assert!(sum.archipelago_autotracking, "fixture registers AP handlers");
    assert_eq!(sum.items, 3);
    assert_eq!(sum.maps, 1);
    assert_eq!(sum.locations, 4);
}

#[test]
fn instruction_cap_aborts_runaway_pack() {
    // A pack whose init.lua spins forever must be aborted by the instruction
    // cap, not hang the host.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/infinite-init");
    let pack = Pack::load(dir).unwrap();
    let r = Session::new(&pack);
    assert!(r.is_err(), "runaway init.lua must be aborted by the instruction cap");
}

#[test]
fn loads_from_zip_with_top_folder() {
    let src = fixture_dir();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    {
        let mut zw = zip::ZipWriter::new(std::fs::File::create(tmp.path()).unwrap());
        // nest under a top folder to exercise find_pack_root's one-level-down case
        zip_add(&mut zw, &src, &src, "ap-storage/");
        zw.finish().unwrap();
    }
    let pack = Pack::load(tmp.path()).expect("load pack from zip");
    let s = Session::new(&pack).unwrap();
    s.on_clear(&serde_json::json!({})).unwrap();
    s.on_retrieved("a", &serde_json::json!(1)).unwrap();
    assert!(accessibility(&s)["Tests/NeedsA"], "zip-loaded pack behaves like the dir");
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
    let a = st.items.iter().find(|i| i.codes.iter().any(|c| c == "a")).unwrap();
    assert_eq!(a.img.as_deref(), Some("images/a.png"), "item image surfaced for the grid");
    assert!(a.codes.iter().any(|c| c == "a"), "item codes surfaced for grid lookup");
    let b = st.items.iter().find(|i| i.codes.iter().any(|c| c == "b")).unwrap();
    assert_eq!(b.count, 5, "consumable count set by the pack's Lua");
    assert_eq!(b.overlay.as_deref(), Some("5"), "consumable count overlay");
    let c = st.items.iter().find(|i| i.codes.iter().any(|c| c == "c")).unwrap();
    assert!(c.active && c.stage == 1, "progressive item active at stage 1");

    let acc = accessibility(&s);
    assert!(acc["Tests/NeedsA"], "a present");
    assert!(acc["Tests/NeedsAandB2"], "a AND b>=2");
    assert!(acc["Tests/NeedsC2orA"], "c2 (stage 1) present");
    assert!(acc["Tests/NeedsLuaRule"], "$has_enough_b: b(5) >= 3");

    // renderer data: the map def and NeedsA's marker come through, marker
    // accessibility tracks the location.
    let st2 = s.state();
    let map = st2.maps.iter().find(|m| m.name == "example").expect("map 'example'");
    assert_eq!(map.img, "images/map.png");
    assert_eq!(map.location_size, Some(16.0));
    let marker = st2.markers.iter().find(|m| m.location == "Tests/NeedsA").expect("NeedsA marker");
    assert_eq!((marker.map.as_str(), marker.x, marker.y), ("example", 10.0, 20.0));
    assert!(marker.accessible, "NeedsA reachable -> marker reachable");
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
