//! JSON items as a pack declares them, and the image filters on their images.

use crate::defs::ItemStage;
use crate::layout::{img_path, num};
use serde::Serialize;

/// One of PopTracker's image filters, for the page to draw. Parsed here into a fixed
/// set of kinds and numbers, as what a pack writes shouldn't reach the page's CSS.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ImageFilter {
    Grey,
    Saturation {
        value: f64,
    },
    Brightness {
        value: f64,
    },
    /// Another of the pack's images drawn on top, with its own filters.
    Overlay {
        img: String,
        filters: Vec<ImageFilter>,
    },
}

/// How many filters an image may have; nothing real comes close.
const MAX_IMAGE_FILTERS: usize = 16;

/// PopTracker's image mods (`img_mods`): a comma-separated list of `name|arg|…`.
/// `@disabled` means the pack's chosen filter. Anything else unknown is no filter,
/// which is how packs write "none".
pub(crate) fn parse_filters(mods: &str, disabled: &str) -> Vec<ImageFilter> {
    mods.split(',')
        .map(|m| m.trim_matches(' '))
        .flat_map(|m| match m {
            // Itself a list. Empty here, so it can't refer to itself.
            "@disabled" | "@disable" => parse_filters(disabled, ""),
            _ => parse_filter(m, disabled).into_iter().collect(),
        })
        .take(MAX_IMAGE_FILTERS)
        .collect()
}

fn parse_filter(text: &str, disabled: &str) -> Option<ImageFilter> {
    let mut parts = text.split('|');
    let name = parts.next()?;
    let args: Vec<&str> = parts.collect();
    let fraction = |arg: Option<&&str>| {
        arg.and_then(|a| a.parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .map_or(0.5, |v| v.clamp(0.0, 255.0))
    };
    match name {
        "grey" | "gray" | "disable" => Some(ImageFilter::Grey),
        "saturation" => Some(ImageFilter::Saturation {
            value: fraction(args.first()),
        }),
        "brightness" => Some(ImageFilter::Brightness {
            value: fraction(args.first()),
        }),
        "greyscale" | "grayscale" => Some(ImageFilter::Saturation { value: 0.0 }),
        "dim" => Some(ImageFilter::Brightness { value: 0.5 }),
        "overlay" if !args.is_empty() => Some(ImageFilter::Overlay {
            img: img_path(args[0]),
            filters: args[1..]
                .iter()
                .flat_map(|f| parse_filters(f, disabled))
                .filter(|f| !matches!(f, ImageFilter::Overlay { .. }))
                .take(MAX_IMAGE_FILTERS)
                .collect(),
        }),
        _ => None,
    }
}

/// A JSON item, as PopTracker's `JsonItem` holds it. `active` and `stage` are its
/// `_stage1` and `_stage2`. Packs read and write it through its object's metatable
/// (see `item_get`, `item_set`).
#[derive(Clone, Default)]
pub(crate) struct JsonItem {
    pub(crate) kind: String,
    pub(crate) name: String,
    pub(crate) codes: Vec<String>,
    /// Its codes as the pack wrote them, as a "*" watch is told.
    pub(crate) codes_text: String,
    pub(crate) stages: Vec<ItemStage>,
    pub(crate) img: String,
    pub(crate) disabled_img: String,
    pub(crate) mods: Vec<ImageFilter>,
    pub(crate) disabled_mods: Vec<ImageFilter>,
    /// The filters of a pack's `Icon`, which is `path:mods`.
    pub(crate) icon_mods: Vec<ImageFilter>,
    /// Its `Icon`, as the pack wrote it.
    pub(crate) icon_text: Option<String>,
    pub(crate) allow_disabled: bool,
    pub(crate) active: bool,
    pub(crate) stage: i64,
    pub(crate) count: i64,
    pub(crate) min: i64,
    pub(crate) max: i64,
    pub(crate) increment: i64,
    pub(crate) decrement: i64,
    pub(crate) overlay: String,
    pub(crate) overlay_color: String,
    pub(crate) icon: Option<String>,
    pub(crate) ignore_user_input: bool,
    /// For a composite toggle: the codes of its left and right parts.
    pub(crate) parts: [String; 2],
}

impl JsonItem {
    /// Every code it may provide, lowercase. A composite's stages are made up, so
    /// its parts' codes don't count.
    pub(crate) fn all_codes(&self) -> Vec<String> {
        let mut all = self.codes.clone();
        if self.kind != "composite_toggle" {
            for st in &self.stages {
                all.extend(st.codes.iter().cloned());
            }
        }
        all
    }

    /// PopTracker's `getState`: on, which `allow_disabled: false` always is.
    pub(crate) fn state(&self) -> bool {
        !self.allow_disabled || self.active
    }

    /// PopTracker's `providesCodeImpl`, for a code it can provide.
    pub(crate) fn provides(&self, code: &str) -> i64 {
        if self.kind == "composite_toggle" {
            if !self.codes.iter().any(|c| c == code) {
                return 0;
            }
            return (self.stage & 1) + ((self.stage >> 1) & 1);
        }
        if (self.stages.len() as i64) > self.stage {
            if self.allow_disabled && !self.active {
                return 0;
            }
            for st in self.stages[..=self.stage as usize].iter().rev() {
                if st.codes.iter().any(|c| c == code) {
                    return 1;
                }
                if !st.inherit_codes {
                    break;
                }
            }
            return 0;
        }
        if self.count != 0 {
            return self.count;
        }
        self.active as i64
    }

    /// Its image now, with its filters: a pack's override, else as PopTracker's
    /// `getImage` and `getImageMods` choose by whether it's on. A toggle ignores its
    /// stages. A blank falls back to any image it has, so the cell isn't empty.
    pub(crate) fn image(&self) -> (Option<String>, Vec<ImageFilter>) {
        if let Some(icon) = &self.icon {
            return (
                (!icon.is_empty()).then(|| img_path(icon)),
                self.icon_mods.clone(),
            );
        }
        let on = self.state();
        let stage = (self.kind != "toggle")
            .then(|| self.stages.get(self.stage.max(0) as usize))
            .flatten();
        let (img, mods) = match stage {
            Some(st) if on => (&st.img, &st.mods),
            Some(st) => (&st.disabled_img, &st.disabled_mods),
            None if on => (&self.img, &self.mods),
            None => (&self.disabled_img, &self.disabled_mods),
        };
        let img = Some(img.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| Some(self.img.as_str()).filter(|s| !s.is_empty()))
            .or_else(|| {
                self.stages
                    .iter()
                    .map(|s| s.img.as_str())
                    .find(|s| !s.is_empty())
            })
            .map(img_path);
        (img, mods.clone())
    }
}

fn split_codes(v: &serde_json::Value) -> Vec<String> {
    v.as_str()
        .unwrap_or("")
        .split(',')
        .map(|c| c.trim_matches(' ').to_lowercase())
        .filter(|c| !c.is_empty())
        .collect()
}

fn read_stage(v: &serde_json::Value, disabled: &str) -> ItemStage {
    let text = |k: &str| v.get(k).and_then(|i| i.as_str()).map(String::from);
    let img = text("img").unwrap_or_default();
    let mods = text("img_mods").unwrap_or_default();
    let off_mods = text("disabled_img_mods").unwrap_or_else(|| format!("{mods},@disabled"));
    ItemStage {
        codes: split_codes(v.get("codes").unwrap_or(&serde_json::Value::Null)),
        disabled_img: text("disabled_img").unwrap_or_else(|| img.clone()),
        img,
        mods: parse_filters(&mods, disabled),
        disabled_mods: parse_filters(&off_mods, disabled),
        inherit_codes: v
            .get("inherit_codes")
            .and_then(|b| b.as_bool())
            .unwrap_or(true),
    }
}

/// PopTracker's `JsonItem::FromJSON`.
pub(crate) fn read_item(v: &serde_json::Value, disabled: &str) -> JsonItem {
    // As PopTracker's `int`, so sums and steps can't overflow.
    let int = |k: &str, d: i64| v.get(k).and_then(num).map_or(d, |n| n as i32 as i64);
    let flag = |k: &str, d: bool| v.get(k).and_then(|b| b.as_bool()).unwrap_or(d);
    let text = |k: &str| v.get(k).and_then(|t| t.as_str()).unwrap_or("").to_string();
    let kind = text("type");
    let min = int("min_quantity", 0);
    let increment = int("increment", 1);
    let mods = text("img_mods");
    let off_mods = v
        .get("disabled_img_mods")
        .and_then(|t| t.as_str())
        .map(String::from)
        .unwrap_or_else(|| {
            if mods.is_empty() {
                "@disabled".to_string()
            } else {
                format!("{mods},@disabled")
            }
        });
    let mut item = JsonItem {
        name: text("name"),
        img: text("img"),
        disabled_img: v
            .get("disabled_img")
            .and_then(|t| t.as_str())
            .map_or_else(|| text("img"), String::from),
        mods: parse_filters(&mods, disabled),
        disabled_mods: parse_filters(&off_mods, disabled),
        allow_disabled: flag("allow_disabled", true),
        codes: split_codes(v.get("codes").unwrap_or(&serde_json::Value::Null)),
        min,
        max: int("max_quantity", (-1).min(min - 1)),
        increment,
        decrement: int("decrement", increment),
        ..Default::default()
    };
    match kind.as_str() {
        "toggle" | "toggle_badged" | "progressive_toggle" => item.allow_disabled = true,
        "static" => {
            item.allow_disabled = false;
            item.active = true;
        }
        _ => {}
    }
    if let Some(stages) = v.get("stages").and_then(|s| s.as_array()) {
        item.stages = stages.iter().map(|st| read_stage(st, disabled)).collect();
    }
    if kind == "composite_toggle"
        && let Some(images) = v.get("images").and_then(|i| i.as_array())
    {
        let side =
            |im: &serde_json::Value, k: &str| im.get(k).and_then(|b| b.as_bool()).unwrap_or(false);
        for n in 0..4u8 {
            if let Some(im) = images
                .iter()
                .find(|im| side(im, "left") == (n & 1 != 0) && side(im, "right") == (n & 2 != 0))
            {
                let mut stage = read_stage(im, disabled);
                stage.codes = Vec::new();
                stage.inherit_codes = false;
                item.stages.push(stage);
            }
        }
        item.active = true;
        item.parts = [
            text("item_left").to_lowercase(),
            text("item_right").to_lowercase(),
        ];
    }
    if matches!(
        kind.as_str(),
        "toggle" | "toggle_badged" | "progressive_toggle"
    ) && flag("initial_active_state", false)
    {
        item.active = true;
    }
    let mut written: Vec<String> = vec![text("codes")];
    if let Some(stages) = v.get("stages").and_then(|s| s.as_array()) {
        written.extend(
            stages
                .iter()
                .filter_map(|st| st.get("codes")?.as_str().map(String::from)),
        );
    }
    if kind == "composite_toggle" {
        written.push(text("item_left"));
        written.push(text("item_right"));
    }
    written.retain(|c| !c.is_empty());
    item.codes_text = written.join(", ");
    item.stage = int("initial_stage_idx", 0)
        .min(item.stages.len() as i64 - 1)
        .max(0);
    item.count = int("initial_quantity", 0).max(item.min);
    if item.max >= item.min && item.count > item.max {
        item.count = item.max;
    }
    if kind == "consumable" && item.count > 0 {
        item.active = true;
    }
    item.kind = kind;
    item
}
