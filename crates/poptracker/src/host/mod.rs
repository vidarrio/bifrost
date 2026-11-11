//! The host API: the globals a pack's Lua sees.

mod archipelago;
mod globals;
mod objects;
mod require;
mod script_host;
mod tracker;

use crate::Result;
use crate::engine::E;
use mlua::Lua;

// ---- host API ----------------------------------------------------------------

pub(crate) fn install_host(lua: &Lua, eng: &E) -> Result<()> {
    let g = lua.globals();

    globals::install(lua, &g)?;
    tracker::install(lua, eng, &g)?;
    script_host::install(lua, eng, &g)?;
    objects::install(lua, eng)?;
    require::install(lua, eng, &g)?;
    archipelago::install(lua, eng, &g)?;
    globals::install_levels(lua, &g)?;

    Ok(())
}
