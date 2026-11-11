//! Location, section and item objects, and custom items.

use crate::Result;
use crate::engine::{DIRTY_CUSTOM, E, ITEM_INDEX, ITEM_METHODS, ITEM_OBJECTS, LOCATION_METHODS};
use crate::items::{item_get, item_set};
use crate::rules::locations::{hosted_collected, mark_indirect};
use crate::rules::settle::settle_accessibility;
use crate::rules::{LEVEL_CLEARED, LEVEL_NONE};
use mlua::{Lua, MultiValue, Table, Value};

/// Location and item objects' methods, and `ScriptHost:CreateLuaItem` (so after `ScriptHost`).
pub(super) fn install(lua: &Lua, eng: &E) -> Result<()> {
    // A location or section object's `AccessibilityLevel`, read from the rules.
    {
        let eng2 = eng.clone();
        let methods = lua.create_table()?;
        methods.set(
            "__index",
            lua.create_function(move |lua, (t, key): (Table, Value)| {
                if key
                    .as_string()
                    .and_then(|k| k.to_str().ok().map(|k| k.to_string()))
                    .as_deref()
                    != Some("AccessibilityLevel")
                {
                    return Ok(Value::Nil);
                }
                let id: String = t.raw_get("FullID")?;
                mark_indirect(&eng2);
                // A collected section reads as cleared, as in PopTracker, once it has
                // its hosted items too.
                if t.raw_get::<Option<i64>>("AvailableChestCount")?
                    .is_some_and(|left| left <= 0)
                    && hosted_collected(lua, &eng2, &id)
                {
                    return Ok(Value::Integer(LEVEL_CLEARED));
                }
                settle_accessibility(lua, &eng2);
                let level = eng2.borrow().accessibility.get(&id).copied();
                Ok(Value::Integer(level.unwrap_or(LEVEL_NONE)))
            })?,
        )?;
        lua.set_named_registry_value(LOCATION_METHODS, methods)?;
    }
    // Custom items: a table the pack fills in, which `FindObjectForCode` can
    // find again. `:Set`/`:Get` keep values in its `ItemState`; overlays,
    // clicks and icons aren't drawn here.
    // JSON items' objects: fields go to the item, and methods are shared. The
    // overlay text shows; its styling isn't drawn here.
    {
        let methods = lua.create_table()?;
        for noop in [
            "SetOverlayBackground",
            "SetOverlayColor",
            "SetOverlayFontSize",
            "SetOverlayAlign",
        ] {
            methods.set(noop, lua.create_function(|_, _: MultiValue| Ok(()))?)?;
        }
        let meta = lua.create_table()?;
        let index_of = |t: &Table| t.raw_get::<i64>(ITEM_INDEX).map(|i| i as usize);
        let eng2 = eng.clone();
        methods.set(
            "SetOverlay",
            lua.create_function(move |lua, (t, text): (Table, Value)| {
                item_set(lua, &eng2, index_of(&t)?, "BadgeText", text).map(|_| ())
            })?,
        )?;
        let eng2 = eng.clone();
        meta.set(
            "__index",
            lua.create_function(move |lua, (t, key): (Table, Value)| {
                let Some(name) = key
                    .as_string()
                    .and_then(|k| k.to_str().ok().map(|k| k.to_string()))
                else {
                    return Ok(Value::Nil);
                };
                let method: Value = methods.raw_get(name.as_str())?;
                if !method.is_nil() {
                    return Ok(method);
                }
                item_get(lua, &eng2, index_of(&t)?, &name)
            })?,
        )?;
        let eng2 = eng.clone();
        meta.set(
            "__newindex",
            lua.create_function(move |lua, (t, key, value): (Table, Value, Value)| {
                if let Some(name) = key
                    .as_string()
                    .and_then(|k| k.to_str().ok().map(|k| k.to_string()))
                    && item_set(lua, &eng2, index_of(&t)?, &name, value.clone())?
                {
                    return Ok(());
                }
                t.raw_set(key, value)
            })?,
        )?;
        lua.set_named_registry_value(ITEM_METHODS, meta)?;
        lua.set_named_registry_value(ITEM_OBJECTS, lua.create_table()?)?;
    }
    let eng2 = eng.clone();
    lua.set_named_registry_value(DIRTY_CUSTOM, lua.create_table()?)?;
    let changed = lua.create_function(move |lua, item: Table| {
        let dirty: Table = lua.named_registry_value(DIRTY_CUSTOM)?;
        dirty.raw_set(item, true)?;
        let mut e = eng2.borrow_mut();
        e.accessibility_stale = true;
        e.visibility_stale = true;
        e.provider_cache.clear();
        e.dirty_custom = true;
        Ok(())
    })?;
    lua.load(
        r##"
        local changed = ...
        local items = __LUAITEMS
        local function noop() end
        ScriptHost.CreateLuaItem = function()
          local item = { Type = "custom", ItemState = {} }
          item.Set = function(self, k, v)
            local old = self.ItemState[k]
            self.ItemState[k] = v
            if old ~= v then
              changed(self)
              if type(self.PropertyChangedFunc) == "function" then
                self:PropertyChangedFunc(k, v, old)
              end
            end
          end
          item.Get = function(self, k) return self.ItemState[k] end
          item.SetOverlay = noop
          item.SetOverlayBackground = noop
          item.SetOverlayColor = noop
          item.SetOverlayFontSize = noop
          item.SetOverlayAlign = noop
          items[#items + 1] = item
          return item
        end
        "##,
    )
    .set_name("host")
    .set_mode(mlua::chunk::ChunkMode::Text)
    .call::<()>(changed)?;

    Ok(())
}
