//! Layouts, rule lists and location trees read from a pack's JSON.

use crate::defs::{LocNode, MapLoc, SectionDef};
use crate::rules::{MAX_ALTERNATIVES, Rules};
use crate::sandbox::note_error;
use crate::session::engine::E;
use std::collections::HashMap;
use std::rc::Rc;

/// Resolve a layout node for the UI: inline `{type:"layout", key:K}` references
/// to the named layout `K`, recursing through `content`/`tabs`. Other fields
/// (type, header, title, maps, orientation, dock…) are kept so the front-end can
/// reproduce the pack's tab hierarchy.
pub(crate) fn resolve_layout(
    layouts: &HashMap<String, serde_json::Value>,
    node: &serde_json::Value,
    depth: u32,
) -> serde_json::Value {
    if depth > 64 {
        return serde_json::Value::Null;
    }
    match node {
        serde_json::Value::Object(o) => {
            if o.get("type").and_then(|v| v.as_str()) == Some("layout") {
                if let Some(target) = o
                    .get("key")
                    .and_then(|v| v.as_str())
                    .and_then(|k| layouts.get(k))
                {
                    return resolve_layout(layouts, target, depth + 1);
                }
                return serde_json::Value::Null;
            }
            let mut m = serde_json::Map::new();
            for (k, v) in o {
                let nv = if k == "content" || k == "tabs" {
                    resolve_layout(layouts, v, depth + 1)
                } else {
                    v.clone()
                };
                m.insert(k.clone(), nv);
            }
            serde_json::Value::Object(m)
        }
        serde_json::Value::Array(a) => serde_json::Value::Array(
            a.iter()
                .map(|x| resolve_layout(layouts, x, depth + 1))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// An item image spec may carry comma-separated image mods (e.g.
/// `images/x.png,@disabled`); the file path is the part before the first comma.
pub(crate) fn img_path(s: &str) -> String {
    s.split(',').next().unwrap_or("").trim().to_string()
}

/// A number that PopTracker may encode as a JSON number or a numeric string.
pub(crate) fn num(v: &serde_json::Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// A location's or section's rule list, each alternative joined to each of its
/// parent's (PopTracker's way of inheriting). A rule is a comma-separated string or
/// an array of codes; a whole list may also be one string, which is one code.
fn parse_rules(eng: &E, node: &serde_json::Value, field: &str, parent: &Rules) -> Rules {
    let split = |v: &str| -> Vec<String> {
        v.split(',')
            .map(|c| c.trim_matches(' ').to_string())
            .collect()
    };
    let own: Rules = match node.get(field) {
        Some(serde_json::Value::String(s)) if !s.is_empty() => vec![vec![s.clone()]],
        Some(serde_json::Value::Array(a)) if !a.is_empty() => {
            let own: Rules = a
                .iter()
                .filter_map(|v| match v {
                    serde_json::Value::String(s) => Some(split(s)),
                    serde_json::Value::Array(parts) => Some(
                        parts
                            .iter()
                            .filter_map(|p| p.as_str().map(String::from))
                            .collect(),
                    ),
                    _ => None,
                })
                .collect();
            // Present but with nothing usable: no rules at all, not the parent's.
            if own.is_empty() {
                return own;
            }
            own
        }
        _ => return parent.clone(),
    };
    if parent.is_empty() {
        return own;
    }
    let mut out = Vec::with_capacity((own.len() * parent.len()).min(MAX_ALTERNATIVES));
    'join: for old in parent {
        for new in &own {
            if out.len() == MAX_ALTERNATIVES {
                note_error(
                    eng,
                    "a location's rules have too many alternatives, so some are ignored",
                );
                break 'join;
            }
            let mut rule = old.clone();
            rule.extend(new.iter().cloned());
            out.push(rule);
        }
    }
    out
}

/// A map location's own rules. Unlike a location's, an empty string is a rule, one
/// that's always met.
fn map_location_rules(eng: &E, m: &serde_json::Value, field: &str) -> Rules {
    match m.get(field) {
        Some(serde_json::Value::String(s)) => vec![vec![s.clone()]],
        _ => parse_rules(eng, m, field, &Vec::new()),
    }
}

/// Flatten the location tree into [`LocNode`]s, accumulating the parent chain's
/// access-rule groups so a node's effective accessibility includes its ancestors.
pub(crate) fn flatten_locations(
    eng: &E,
    node: &serde_json::Value,
    prefix: &str,
    parent_rules: &Rules,
    parent_vis: &Rules,
    parent_glitched: bool,
) {
    if let Some(arr) = node.as_array() {
        for n in arr {
            flatten_locations(eng, n, prefix, parent_rules, parent_vis, parent_glitched);
        }
        return;
    }
    let name = node.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let full = if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    };

    // `parent` inherits from a location added by an earlier file, in place of the
    // one this is nested in: by id, else the end of an id, as in PopTracker.
    let named_parent = node
        .get("parent")
        .and_then(|v| v.as_str())
        .filter(|p| !p.is_empty())
        .and_then(|p| {
            let p = p.strip_prefix('@').unwrap_or(p);
            let e = eng.borrow();
            let earlier = &e.nodes[..e.locations_before.min(e.nodes.len())];
            earlier
                .iter()
                .find(|n| n.name == p)
                .or_else(|| {
                    earlier
                        .iter()
                        .find(|n| n.name.len() > p.len() && n.name.ends_with(p))
                })
                .map(|n| (n.rules.clone(), n.vis.clone()))
        });
    let (parent_rules, parent_vis) = match &named_parent {
        Some((r, v)) => (r, v),
        None => (parent_rules, parent_vis),
    };
    let rules = parse_rules(eng, node, "access_rules", parent_rules);
    let vis = parse_rules(eng, node, "visibility_rules", parent_vis);
    let scoutable_glitched = node
        .get("inspectable_sequence_break")
        .and_then(|v| v.as_bool())
        .unwrap_or(parent_glitched);

    let map_locations: Vec<MapLoc> = node
        .get("map_locations")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    Some(MapLoc {
                        map: m.get("map")?.as_str()?.to_string(),
                        x: num(m.get("x")?)?,
                        y: num(m.get("y")?)?,
                        size: m.get("size").and_then(num),
                        shape: m.get("shape").and_then(|v| v.as_str().map(String::from)),
                        orientation: m
                            .get("orientation")
                            .and_then(|v| v.as_str().map(String::from)),
                        border: m.get("border_thickness").and_then(num),
                        restrict: map_location_rules(eng, m, "restrict_visibility_rules"),
                        force_hidden: map_location_rules(eng, m, "force_invisibility_rules"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let sections: Vec<SectionDef> = node
        .get("sections")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .map(|s| {
                    // As written: a custom item may compare them exactly.
                    let hosted: Vec<String> = s
                        .get("hosted_item")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .split(',')
                        .map(|c| c.trim_matches(' ').to_string())
                        .filter(|c| !c.is_empty())
                        .collect();
                    SectionDef {
                        name: s
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        // A `ref` or a hosted item holds no chests of its own.
                        item_count: s.get("item_count").and_then(|v| v.as_i64()).map_or(
                            if s.get("ref").is_some() || !hosted.is_empty() {
                                0
                            } else {
                                1
                            },
                            |n| n as i32 as i64,
                        ),
                        rules: parse_rules(eng, s, "access_rules", &rules),
                        vis: parse_rules(eng, s, "visibility_rules", &vis),
                        reference: s.get("ref").and_then(|v| v.as_str()).map(String::from),
                        hosted,
                        scoutable_glitched: s
                            .get("inspectable_sequence_break")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(scoutable_glitched),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    // Every location is kept, as rules may refer to one that isn't drawn.
    {
        let mut e = eng.borrow_mut();
        // A second location with an id gets `id[1]` and so on, as in PopTracker, to
        // keep its own level; it still shows under its name.
        let mut number = e.duplicate_ids.get(&full).copied().unwrap_or(0);
        let mut id = full.clone();
        while e.taken_ids.contains(&id) {
            number += 1;
            id = format!("{full}[{number}]");
        }
        e.duplicate_ids.insert(full.clone(), number);
        e.taken_ids.insert(id.clone());
        let nodes = Rc::make_mut(&mut e.nodes);
        nodes.push(LocNode {
            name: id,
            display_name: full.clone(),
            short_name: name.to_string(),
            rules: rules.clone(),
            vis: vis.clone(),
            scoutable_glitched,
            map_locations,
            sections,
        });
        e.location_lookup.clear();
        e.accessibility_stale = true;
        e.visibility_stale = true;
    }
    if let Some(children) = node.get("children") {
        flatten_locations(eng, children, &full, &rules, &vis, scoutable_glitched);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_inlines_refs_inside_tabs() {
        let mut layouts = HashMap::new();
        layouts.insert(
            "inner".to_string(),
            serde_json::json!({"type":"map","maps":["m1"]}),
        );
        layouts.insert(
            "tracker_default".to_string(),
            serde_json::json!({"type":"tabbed","tabs":[{"title":"A","content":{"type":"layout","key":"inner"}}]}),
        );
        let r = resolve_layout(&layouts, layouts.get("tracker_default").unwrap(), 0);
        assert_eq!(r["tabs"][0]["content"]["type"], "map");
        assert_eq!(r["tabs"][0]["content"]["maps"][0], "m1");
    }
}
