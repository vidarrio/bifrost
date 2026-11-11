//! `require`, finding modules only in the pack.

use crate::Result;
use crate::pack::{find_in_pack, module_path};
use crate::sandbox::to_lua;
use crate::session::engine::E;
use crate::session::read_pack_file;
use mlua::{Lua, Table, Value};

/// The pack-only `require`.
pub(super) fn install(lua: &Lua, eng: &E, g: &Table) -> Result<()> {
    // `require`, as PopTracker has it (0.25.6+): modules come only from the pack.
    // "foo.baz" tries scripts/foo/baz.lua, scripts/foo/baz/init.lua, foo/baz.lua
    // and foo/baz/init.lua; each file runs once, with the module name as `...`,
    // and its result is cached.
    {
        let eng2 = eng.clone();
        let loaded = lua.create_table()?;
        let loading = lua.create_table()?;
        let std_libs = lua.create_table()?;
        for lib in ["os", "string", "table", "math", "coroutine", "utf8"] {
            std_libs.set(lib, g.get::<Value>(lib)?)?;
        }
        g.set(
            "require",
            lua.create_function(move |lua, name: String| {
                // The libraries packs have, as they are here: `os` is the small one.
                let lib: Value = std_libs.get(name.as_str())?;
                if !lib.is_nil() {
                    return Ok(lib);
                }
                let rel = module_path(&name).ok_or_else(|| {
                    mlua::Error::runtime(format!("require: bad module name '{name}'"))
                })?;
                let (dir, variant) = {
                    let e = eng2.borrow();
                    (e.dir.clone(), e.variant.clone())
                };
                // PopTracker tries `.LUA` before `.lua` for each.
                let file = [
                    format!("scripts/{rel}.LUA"),
                    format!("scripts/{rel}.lua"),
                    format!("scripts/{rel}/init.LUA"),
                    format!("scripts/{rel}/init.lua"),
                    format!("{rel}.LUA"),
                    format!("{rel}.lua"),
                    format!("{rel}/init.LUA"),
                    format!("{rel}/init.lua"),
                ]
                .into_iter()
                .find(|f| find_in_pack(&dir, &variant, f).is_some_and(|p| p.is_file()))
                .ok_or_else(|| {
                    mlua::Error::runtime(format!("require: module '{name}' not found in pack"))
                })?;
                // Cached by file, so "a.b" and "a/b" are one module.
                let cached: Value = loaded.get(file.as_str())?;
                if !cached.is_nil() {
                    return Ok(cached);
                }
                // A module that requires itself, directly or not, is an error.
                if loading.get::<bool>(file.as_str())? {
                    return Err(mlua::Error::runtime(format!(
                        "require: module '{name}' requires itself"
                    )));
                }
                let txt = read_pack_file(&eng2, &file).map_err(to_lua)?;
                loading.set(file.as_str(), true)?;
                let v = lua
                    .load(txt)
                    .set_name(file.as_str())
                    .set_mode(mlua::chunk::ChunkMode::Text)
                    .call::<Value>(name.as_str());
                loading.set(file.as_str(), Value::Nil)?;
                let v = v?;
                let v = if v.is_nil() { Value::Boolean(true) } else { v };
                loaded.set(file.as_str(), &v)?;
                Ok(v)
            })?,
        )?;
    }

    Ok(())
}
