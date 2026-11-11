//! The pack's maps, locations and item stages as the engine keeps them.

use crate::item_json::ImageFilter;
use crate::rules::Rules;

#[derive(Clone)]
pub(crate) struct MapDef {
    pub(crate) name: String,
    pub(crate) img: String,
    pub(crate) location_size: Option<f64>,
    pub(crate) location_shape: Option<String>,
    pub(crate) location_orientation: Option<String>,
    pub(crate) location_border: Option<f64>,
}
#[derive(Clone)]
pub(crate) struct MapLoc {
    pub(crate) map: String,
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) size: Option<f64>,
    pub(crate) shape: Option<String>,
    pub(crate) orientation: Option<String>,
    pub(crate) border: Option<f64>,
    /// `restrict_visibility_rules`: on this map, also needs these.
    pub(crate) restrict: Rules,
    /// `force_invisibility_rules`: on this map, hidden if these are met.
    pub(crate) force_hidden: Rules,
}
#[derive(Clone)]
pub(crate) struct SectionDef {
    pub(crate) name: String,
    pub(crate) item_count: i64,
    /// `hosted_item`: the codes of items this section stands for.
    pub(crate) hosted: Vec<String>,
    /// `ref`: this section is that one (`location/section`), shown elsewhere.
    pub(crate) reference: Option<String>,
    pub(crate) rules: Rules, // effective access_rules: the location's joined to its own
    pub(crate) vis: Rules,   // effective visibility_rules; empty = always visible
    pub(crate) scoutable_glitched: bool,
}
#[derive(Clone)]
pub(crate) struct LocNode {
    pub(crate) name: String, // its id: the full path, e.g. "Area/Sub/Spot"
    pub(crate) display_name: String, // the same, without what makes a duplicate's id unique
    pub(crate) short_name: String, // its last part
    pub(crate) rules: Rules, // effective access_rules: the parent chain's joined to its own
    pub(crate) vis: Rules,   // effective visibility_rules, likewise; empty = always visible
    pub(crate) scoutable_glitched: bool,
    pub(crate) map_locations: Vec<MapLoc>,
    pub(crate) sections: Vec<SectionDef>,
}

/// One stage of a JSON item.
#[derive(Clone, Default)]
pub(crate) struct ItemStage {
    pub(crate) codes: Vec<String>,
    pub(crate) img: String,
    pub(crate) disabled_img: String,
    pub(crate) mods: Vec<ImageFilter>,
    pub(crate) disabled_mods: Vec<ImageFilter>,
    pub(crate) inherit_codes: bool,
}

impl LocNode {
    /// A real, drawable/checkable location: it has sections or a map placement.
    pub(crate) fn drawn(&self) -> bool {
        !self.sections.is_empty() || !self.map_locations.is_empty()
    }
}
