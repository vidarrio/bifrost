//! Plain globals and enums: `PopVersion`, `AutoTracker`, `Highlight`,
//! `ImageReference` and `AccessibilityLevel`.

use crate::Result;
use crate::rules::{
    LEVEL_CLEARED, LEVEL_INSPECT, LEVEL_NONE, LEVEL_NORMAL, LEVEL_PARTIAL, LEVEL_SEQUENCE_BREAK,
};
use mlua::{Lua, MultiValue, Table, Value};

/// The plain globals, the inert `AutoTracker`, `Highlight` and `ImageReference`.
pub(super) fn install(lua: &Lua, g: &Table) -> Result<()> {
    // Plain globals the API declares (packs gate on PopVersion; a nil breaks
    // version comparisons like `PopVersion < "x"`).
    g.set("PopVersion", "0.34.1")?;
    g.set("DEBUG", false)?;

    // AutoTracker (memory interface): present but inert — AP packs still probe
    // it (e.g. GetConnectionState). Reads return 0; only Archipelago reads as
    // connected.
    let at = lua.create_table()?;
    for m in ["ReadU8", "ReadU16", "ReadU24", "ReadU32"] {
        at.set(m, lua.create_function(|_, _: MultiValue| Ok(0i64))?)?;
    }
    at.set(
        "ReadVariable",
        lua.create_function(|_, _: MultiValue| Ok(Value::Nil))?,
    )?;
    at.set(
        "GetConnectionState",
        // Only Archipelago is here, connected. A pack told a console is connected
        // reads its memory, which is all zeroes, over the items it just received.
        lua.create_function(|_, (_t, backend): (Value, Option<String>)| {
            Ok(match backend.as_deref() {
                Some("AP") => 3i64,
                Some("SNES" | "UAT") => 1,
                _ => -1,
            })
        })?,
    )?;
    at.set("SelectedConnectorType", lua.create_table()?)?;
    g.set("AutoTracker", &at)?;

    // Highlight enum (display only).
    let hl = lua.create_table()?;
    for (k, v) in [
        ("None", 0),
        ("Avoid", -1),
        ("NoPriority", 1),
        ("Unspecified", 2),
        ("Priority", 3),
    ] {
        hl.set(k, v)?;
    }
    g.set("Highlight", &hl)?;

    // ImageReference: images are only drawn from item and location JSON here,
    // so a reference is just its pack path.
    let img = lua.create_table()?;
    img.set(
        "FromPackRelativePath",
        lua.create_function(|_, (_t, path): (Table, String)| Ok(path))?,
    )?;
    img.set(
        "FromImageReference",
        lua.create_function(|lua, (_t, r, m): (Table, Value, Option<String>)| {
            // A mod is added to the reference: `path:mod,mod`.
            let (Value::String(r), Some(m)) = (&r, m.filter(|m| !m.is_empty())) else {
                return Ok(r);
            };
            let r = r.to_string_lossy();
            let joined = if r.contains(':') {
                format!("{r},{m}")
            } else {
                format!("{r}:{m}")
            };
            lua.create_string(&joined).map(Value::String)
        })?,
    )?;
    g.set("ImageReference", &img)?;

    Ok(())
}

/// The `AccessibilityLevel` enum.
pub(super) fn install_levels(lua: &Lua, g: &Table) -> Result<()> {
    // AccessibilityLevel enum
    let al = lua.create_table()?;
    for (k, v) in [
        ("None", LEVEL_NONE),
        ("Partial", LEVEL_PARTIAL),
        ("Inspect", LEVEL_INSPECT),
        ("SequenceBreak", LEVEL_SEQUENCE_BREAK),
        ("Normal", LEVEL_NORMAL),
        ("Cleared", LEVEL_CLEARED),
    ] {
        al.set(k, v)?;
    }
    g.set("AccessibilityLevel", &al)?;

    Ok(())
}
