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
    let err = Session::new(&pack)
        .err()
        .expect("runaway init.lua must be aborted");
    assert!(
        err.to_string().contains("instruction limit exceeded"),
        "aborted by the instruction cap, not something else: {err}"
    );
}

#[test]
fn instruction_cap_covers_coroutines() {
    // A coroutine runs on its own Lua thread; the cap must apply there too.
    // Run it on a thread, so a regression fails within seconds instead of
    // hanging the test run.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/infinite-coroutine");
        let pack = Pack::load(dir).unwrap();
        let _ = tx.send(Session::new(&pack).err().map(|e| e.to_string()));
    });
    let err = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("a runaway coroutine wasn't stopped within 30 s")
        .expect("runaway coroutine must be aborted");
    assert!(
        err.contains("instruction limit exceeded"),
        "aborted by the instruction cap: {err}"
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

#[test]
fn handlers_may_register_handlers() {
    // Every event kind fires a handler that registers another one, and the
    // Clear handler loads more items; both borrow the engine mutably, and none
    // may panic on a held borrow (#2).
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/reentrant-handlers");
    let s = Session::new(&Pack::load(dir).unwrap()).unwrap();
    s.on_clear(&serde_json::json!({})).unwrap();
    s.on_item(1, 100, "Thing", 1).unwrap();
    s.on_location(200, "Place").unwrap();
    s.on_retrieved("key", &serde_json::json!(1)).unwrap();
    let active = |code: &str| {
        s.state()
            .items
            .iter()
            .find(|i| i.codes.iter().any(|c| c == code))
            .unwrap_or_else(|| panic!("no item {code}"))
            .active
    };
    // A handler list is a snapshot taken when its event fires: the late
    // Retrieved and SetReply handlers were added during the one on_retrieved
    // call that fires both lists, and Clear hasn't fired since late_clear was
    // added. The late Item and Location handlers already ran (their events
    // came after the ones that added them).
    for code in ["late_clear", "late_retrieved", "late_set_reply"] {
        assert!(!active(code), "{code} ran too early");
    }
    assert!(active("late_item") && active("late_location"));
    // …but they do on the next one; the Clear handler's items loaded too.
    s.on_clear(&serde_json::json!({})).unwrap();
    s.on_item(2, 100, "Thing", 1).unwrap();
    s.on_location(201, "Place").unwrap();
    s.on_retrieved("key", &serde_json::json!(2)).unwrap();
    for code in [
        "late_clear",
        "late_item",
        "late_location",
        "late_retrieved",
        "late_set_reply",
    ] {
        assert!(active(code), "{code} didn't run");
    }
    assert!(
        s.state()
            .items
            .iter()
            .any(|i| i.codes.iter().any(|c| c == "loaded")),
        "items loaded by a handler"
    );
}

#[test]
fn host_api_covers_require_custom_items_and_callbacks() {
    // The fixture's init.lua asserts most of it (require, custom items,
    // player lookups); here, what has to be seen from outside.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/host-api");
    let s = Session::new(&Pack::load(dir).unwrap()).unwrap();
    let item = |s: &Session, code: &str| {
        s.state()
            .items
            .into_iter()
            .find(|i| i.codes.iter().any(|c| c == code))
            .unwrap()
    };
    // The clear handler saw one frame (after init) and the self-removing
    // handler's work; each state runs the "*" watch once.
    s.on_clear(&serde_json::json!({})).unwrap();
    assert!(item(&s, "key").active, "one frame before the clear");
    assert_eq!(item(&s, "watched").count, 2);
    assert_eq!(item(&s, "watched").count, 3);
    // Frames: init, the clear, and the three states so far; "once" ran once.
    // (Each `item` call is a state, which runs the frame handlers first.)
    assert_eq!(item(&s, "frames").count, 6);
    assert_eq!(item(&s, "once").count, 1);
    assert_eq!(item(&s, "ordered").count, 8, "in the order added");
    let acc = accessibility(&s);
    assert!(acc["Args"], "$has_args|one|two");
    assert!(!acc["WrongArgs"], "$has_args|one");
}

#[test]
fn clear_handlers_get_slot_data_and_the_players() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/connect");
    let s = Session::new(&Pack::load(dir).unwrap()).unwrap();
    s.set_multiworld(Some("seed1"), &[(1, "Ann", "Game A"), (2, "Bo", "Game B")])
        .unwrap();
    s.on_clear(&serde_json::json!({"Data": {"Options": {"Flag": true}}}))
        .unwrap();
    let active = |code: &str| {
        s.state()
            .items
            .into_iter()
            .find(|i| i.codes.iter().any(|c| c == code))
            .unwrap()
            .active
    };
    assert!(active("from_slot_data"));
    assert!(active("players_known"));
}

fn zip_of(dir: &Path, names: impl Iterator<Item = String>) -> std::path::PathBuf {
    let path = dir.join("p.zip");
    let mut z = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    for n in names {
        z.start_file(&n, zip::write::SimpleFileOptions::default())
            .unwrap();
    }
    z.finish().unwrap();
    path
}

#[test]
fn extraction_caps_depth_and_counts_folders() {
    let tmp = tempfile::TempDir::new().unwrap();
    let deep = zip_of(tmp.path(), std::iter::once(format!("{}f", "d/".repeat(40))));
    let err = poptracker::extract_zip(&deep, &tmp.path().join("a")).expect_err("too deep");
    assert!(err.to_string().contains("too deep"), "{err}");
    // Few entries, each bringing many folders of its own.
    let many = zip_of(
        tmp.path(),
        (0..4000).map(|i| format!("{}f", format!("{i}x/").repeat(30))),
    );
    let err = poptracker::extract_zip(&many, &tmp.path().join("b")).expect_err("too many");
    assert!(err.to_string().contains("too many"), "{err}");
    // Shared folders count once.
    let fine = zip_of(tmp.path(), (0..2000).map(|i| format!("a/b/c/{i}")));
    poptracker::extract_zip(&fine, &tmp.path().join("c")).unwrap();
}

#[test]
fn packs_load_with_variants_and_survive_their_errors() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/survive");
    let pack = Pack::load(dir).unwrap();
    // The Archipelago map variant wins, and its files come first.
    assert_eq!(pack.asset("/images/x.png").unwrap(), b"variant\n");
    assert_eq!(pack.asset("images/root.png").unwrap(), b"root\n");
    assert!(pack.asset("../manifest.json").is_none());
    let s = Session::new(&pack).unwrap();
    s.on_clear(&serde_json::json!({})).unwrap();
    s.on_item(1, 100, "Thing", 1).unwrap();
    let st = s.state();
    let item = |code: &str| {
        st.items
            .iter()
            .find(|i| i.codes.iter().any(|c| c == code))
            .unwrap()
    };
    assert!(item("after_error").active, "the next handler still ran");
    assert!(item("got_item").active, "and later events too");
    assert_eq!(item("after_error").overlay.as_deref(), Some("hello"));
    assert_eq!(st.pack_error_count, 2);
    assert!(
        st.pack_errors[0].contains("broken handler"),
        "{:?}",
        st.pack_errors
    );
}

#[test]
fn running_out_of_budget_in_a_handler_still_ends_it() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/survive");
    let s = Session::new(&Pack::load(dir).unwrap()).unwrap();
    let err = s
        .on_location(1, "spin")
        .expect_err("a handler that never ends must be stopped");
    assert!(err.to_string().contains("instruction limit"), "{err}");
}
