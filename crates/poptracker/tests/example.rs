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
        let rel = format!(
            "{prefix}{}",
            p.strip_prefix(base).unwrap().to_string_lossy()
        );
        if p.is_dir() {
            zip_add(zw, base, &p, prefix);
        } else {
            zw.start_file(&rel, zip::write::SimpleFileOptions::default())
                .unwrap();
            zw.write_all(&std::fs::read(&p).unwrap()).unwrap();
        }
    }
}

#[test]
fn summary_reports_ap_autotracking_and_counts() {
    let s = Session::new(&fixture()).unwrap();
    let sum = s.summary();
    assert!(
        sum.archipelago_autotracking,
        "fixture registers AP handlers"
    );
    assert_eq!(sum.items, 3);
    assert_eq!(sum.maps, 1);
    assert_eq!(sum.locations, 9);
}

#[test]
fn instruction_cap_aborts_runaway_pack() {
    // A pack whose init.lua spins forever must be aborted by the instruction
    // cap, not hang the host.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/infinite-init");
    let pack = Pack::load(dir).unwrap();
    let r = Session::new(&pack);
    assert!(
        r.is_err(),
        "runaway init.lua must be aborted by the instruction cap"
    );
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
    assert!(
        accessibility(&s)["Tests/NeedsA"],
        "zip-loaded pack behaves like the dir"
    );
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
    let a = st
        .items
        .iter()
        .find(|i| i.codes.iter().any(|c| c == "a"))
        .unwrap();
    assert_eq!(
        a.img.as_deref(),
        Some("images/a.png"),
        "item image surfaced for the grid"
    );
    assert!(
        a.codes.iter().any(|c| c == "a"),
        "item codes surfaced for grid lookup"
    );
    let b = st
        .items
        .iter()
        .find(|i| i.codes.iter().any(|c| c == "b"))
        .unwrap();
    assert_eq!(b.count, 5, "consumable count set by the pack's Lua");
    assert_eq!(b.overlay.as_deref(), Some("5"), "consumable count overlay");
    let c = st
        .items
        .iter()
        .find(|i| i.codes.iter().any(|c| c == "c"))
        .unwrap();
    assert!(
        c.active && c.stage == 1,
        "progressive item active at stage 1"
    );

    let acc = accessibility(&s);
    assert!(acc["Tests/NeedsA"], "a present");
    assert!(acc["Tests/NeedsAandB2"], "a AND b>=2");
    assert!(acc["Tests/NeedsC2orA"], "c2 (stage 1) present");
    assert!(acc["Tests/NeedsLuaRule"], "$has_enough_b: b(5) >= 3");

    // renderer data: the map def and NeedsA's marker come through, marker
    // accessibility tracks the location.
    let st2 = s.state();
    let map = st2
        .maps
        .iter()
        .find(|m| m.name == "example")
        .expect("map 'example'");
    assert_eq!(map.img, "images/map.png");
    assert_eq!(map.location_size, Some(16.0));
    let marker = st2
        .markers
        .iter()
        .find(|m| m.location == "Tests/NeedsA")
        .expect("NeedsA marker");
    assert_eq!(
        (marker.map.as_str(), marker.x, marker.y),
        ("example", 10.0, 20.0)
    );
    assert!(marker.accessible, "NeedsA reachable -> marker reachable");
}

#[test]
fn checked_location_marks_section_and_marker_done() {
    let s = Session::new(&fixture()).unwrap();
    s.on_clear(&serde_json::json!({})).unwrap();

    // Before any check, NeedsA is not collected and nothing is cleared. The
    // marker carries the per-section state the map UI needs for split colors.
    let before = s.state();
    let m0 = before
        .markers
        .iter()
        .find(|m| m.location == "Tests/NeedsA")
        .unwrap();
    assert!(!m0.checked, "unchecked location marker is not Done");
    assert_eq!(
        m0.sections.len(),
        1,
        "marker carries its location's sections"
    );
    assert_eq!(m0.sections[0].cleared, 0, "nothing cleared before a check");
    let l0 = before
        .locations
        .iter()
        .find(|l| l.name == "Tests/NeedsA")
        .unwrap();
    assert!(!l0.checked && !l0.sections[0].checked);

    // Feed the checked location id (maps to @Tests/NeedsA in the fixture).
    s.on_location(1, "Tests/NeedsA").unwrap();

    let after = s.state();
    let l = after
        .locations
        .iter()
        .find(|l| l.name == "Tests/NeedsA")
        .unwrap();
    assert!(
        l.sections[0].checked,
        "section chest fully decremented -> checked"
    );
    assert_eq!(
        l.sections[0].cleared, l.sections[0].item_count,
        "cleared count reaches the section total once Done"
    );
    assert!(l.checked, "all sections checked -> location Done");
    let m = after
        .markers
        .iter()
        .find(|m| m.location == "Tests/NeedsA")
        .unwrap();
    assert!(m.checked, "location Done -> marker renders grey");
    assert!(m.sections[0].checked, "marker's section state is Done too");
}

#[test]
fn progressive_stage_gates_secondary_codes() {
    // Regression: a progressive item's higher-stage code (here `cstage2`, the
    // second code of the upper stage) must not count as held while the item
    // sits below that stage — the bug that let a Big Barn satisfy a Deluxe
    // Barn gate because only each stage's *first* code was indexed.
    let s = Session::new(&fixture()).unwrap();
    s.on_clear(&serde_json::json!({})).unwrap();
    s.on_retrieved("c", &serde_json::json!([true, 0])).unwrap(); // active, stage 0
    assert!(
        !accessibility(&s)["Tests/NeedsCStage2"],
        "stage-2 code not provided at stage 0"
    );
    s.on_retrieved("c", &serde_json::json!([true, 1])).unwrap(); // bump to stage 1 (== c2/cstage2 stage)
    assert!(
        accessibility(&s)["Tests/NeedsCStage2"],
        "stage-2 code provided once the stage is reached"
    );
}

#[test]
fn string_form_access_rules_still_gate() {
    // A parent `"access_rules": "a"` (a bare string, not an array) must still
    // gate its children — regression for Desert Festival showing all-green.
    let s = Session::new(&fixture()).unwrap();
    s.on_clear(&serde_json::json!({})).unwrap();
    s.on_retrieved("a", &serde_json::json!(0)).unwrap(); // a NOT held
    assert!(
        !accessibility(&s)["Tests/StrParent/Child"],
        "string parent rule 'a' unmet -> child blocked"
    );
    s.on_retrieved("a", &serde_json::json!(1)).unwrap(); // a held
    assert!(
        accessibility(&s)["Tests/StrParent/Child"],
        "string parent rule 'a' met -> child reachable"
    );
}

#[test]
fn visibility_rules_hide_locations_the_slot_lacks() {
    let has_vis = |s: &Session| {
        s.state()
            .locations
            .iter()
            .any(|l| l.name == "Tests/VisGated")
    };
    let marker_shown = |s: &Session| {
        s.state()
            .markers
            .iter()
            .any(|m| m.location == "Tests/VisGated")
    };

    // No valid-location set fed => visibility enforcement is off, marker shows.
    let s = Session::new(&fixture()).unwrap();
    s.on_clear(&serde_json::json!({})).unwrap();
    assert!(
        has_vis(&s) && marker_shown(&s),
        "unfiltered: VisGated is visible"
    );

    // Fed a set WITHOUT the gate id (424242) => hidden from locations and markers.
    let s = Session::new(&fixture()).unwrap();
    s.set_valid_locations(&[1, 2, 3]);
    s.on_clear(&serde_json::json!({})).unwrap();
    assert!(
        !has_vis(&s) && !marker_shown(&s),
        "gate id absent -> hidden"
    );

    // Fed a set WITH the gate id => visible again.
    let s = Session::new(&fixture()).unwrap();
    s.set_valid_locations(&[1, 424242]);
    s.on_clear(&serde_json::json!({})).unwrap();
    assert!(
        has_vis(&s) && marker_shown(&s),
        "gate id present -> visible"
    );
}

#[test]
fn bracket_rules_yield_glitch_and_inspect_levels() {
    // `[b]` is a glitch requirement, `{b}` an inspect requirement. With `a` held
    // but `b` missing, the normal part is met so each rule only downgrades:
    // GlitchB -> sequence-break (yellow), InspectB -> viewable (blue).
    let s = Session::new(&fixture()).unwrap();
    s.on_clear(&serde_json::json!({})).unwrap();
    s.on_retrieved("a", &serde_json::json!(1)).unwrap();
    s.on_retrieved("b", &serde_json::json!(0)).unwrap();

    let st = s.state();
    let sec = |name: &str| {
        st.locations
            .iter()
            .find(|l| l.name == name)
            .unwrap_or_else(|| panic!("location {name}"))
            .sections[0]
            .clone()
    };
    assert_eq!(
        sec("Tests/GlitchB").level,
        "glitch",
        "missing [b] -> sequence break"
    );
    assert_eq!(
        sec("Tests/InspectB").level,
        "inspect",
        "missing {{b}} -> viewable only"
    );
    assert_eq!(
        sec("Tests/NeedsA").level,
        "reachable",
        "plain met rule -> reachable"
    );
    // `accessible` (the in-logic tally) stays true only for a fully reachable rule.
    assert!(sec("Tests/NeedsA").accessible);
    assert!(!sec("Tests/GlitchB").accessible && !sec("Tests/InspectB").accessible);

    // Holding `b` promotes both to fully reachable.
    s.on_retrieved("b", &serde_json::json!(1)).unwrap();
    let st2 = s.state();
    let lvl2 = |name: &str| {
        st2.locations
            .iter()
            .find(|l| l.name == name)
            .unwrap()
            .sections[0]
            .level
            .clone()
    };
    assert_eq!(
        lvl2("Tests/GlitchB"),
        "reachable",
        "[b] now held -> reachable"
    );
    assert_eq!(
        lvl2("Tests/InspectB"),
        "reachable",
        "{{b}} now held -> reachable"
    );
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
