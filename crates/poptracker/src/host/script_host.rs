//! The `ScriptHost` global.

use crate::Result;
use crate::engine::{CODE_WATCHES, E, FRAME_HANDLERS};
use crate::sandbox::{is_memory_error, note_error, to_lua};
use crate::session::read_pack_file;
use mlua::{Function, Lua, MultiValue, Table, Value};

/// The `ScriptHost` global (and `NotificationType`): scripts, frame handlers and code watches.
pub(super) fn install(lua: &Lua, eng: &E, g: &Table) -> Result<()> {
    // ScriptHost
    let sh = lua.create_table()?;
    {
        let eng2 = eng.clone();
        sh.set(
            "LoadScript",
            // Some packs load a script they can only name once connected.
            lua.create_function(move |lua, (_t, file): (Table, Option<String>)| {
                let Some(file) = file else { return Ok(false) };
                // As in PopTracker, a script that won't load is logged and the
                // caller carries on.
                let ran = read_pack_file(&eng2, &file)
                    .map_err(to_lua)
                    .and_then(|txt| {
                        lua.load(txt)
                            .set_name(file.as_str())
                            .set_mode(mlua::chunk::ChunkMode::Text)
                            .call::<()>(load_script_module(&file))
                    });
                match ran {
                    Ok(()) => Ok(true),
                    Err(e) if is_memory_error(&e) => Err(e),
                    Err(e) => {
                        note_error(&eng2, &e.to_string());
                        Ok(false)
                    }
                }
            })?,
        )?;
    }
    for noop in [
        "AddMemoryWatch",
        "RemoveMemoryWatch",
        "AddVariableWatch",
        "RemoveVariableWatch",
        "AddOnLocationSectionChangedHandler",
        "RemoveOnLocationSectionChangedHandler",
        "RemoveOnLocationSectionHandler",
    ] {
        sh.set(noop, lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    }
    // Frame handlers and code watches, by name, in the order they were added
    // (see `FRAME_HANDLERS`). Each list holds `{name, fn, code}` entries; a
    // removed or replaced one is marked dead, so both stay O(1).
    let make: Function = lua
        .load(
            r##"
            return function(list)
              local by_name = {}
              local function remove(_, name)
                local e = by_name[name]
                if not e then return false end
                e.dead = true
                by_name[name] = nil
                return true
              end
              local function add(_, name, a, b)
                remove(nil, name)
                local e = b == nil and { name = name, fn = a }
                  or { name = name, code = a, fn = b }
                by_name[name] = e
                list[#list + 1] = e
                return name
              end
              return add, remove
            end
            "##,
        )
        .set_name("host")
        .set_mode(mlua::chunk::ChunkMode::Text)
        .eval()?;
    for (key, add, remove) in [
        (FRAME_HANDLERS, "AddOnFrameHandler", "RemoveOnFrameHandler"),
        (CODE_WATCHES, "AddWatchForCode", "RemoveWatchForCode"),
    ] {
        let list = lua.create_table()?;
        lua.set_named_registry_value(key, &list)?;
        let (a, r): (Function, Function) = make.call(list)?;
        sh.set(add, a)?;
        sh.set(remove, r)?;
    }
    // Scripts "in the background" run here and now, under the same limits; there
    // are no threads and no one to notify.
    {
        let eng2 = eng.clone();
        // `arg` is the script's while it runs; a failure is logged, as in
        // PopTracker, and no callback is told. A callback's own error is too.
        let notes = eng.clone();
        let finish =
            move |lua: &Lua, src: String, name: String, arg: Value, done: Option<Function>| {
                let chunk = lua
                    .load(src)
                    .set_name(name)
                    .set_mode(mlua::chunk::ChunkMode::Text);
                let outer: Value = lua.globals().get("arg")?;
                lua.globals().set("arg", arg)?;
                let result = chunk.eval::<Value>();
                lua.globals().set("arg", outer)?;
                let finished = match (result, done) {
                    (Ok(v), Some(done)) => done.call::<()>(v),
                    (Ok(_), None) => Ok(()),
                    (Err(e), _) => Err(e),
                };
                match finished {
                    Err(e) if is_memory_error(&e) => Err(e),
                    Err(e) => {
                        note_error(&notes, &e.to_string());
                        Ok(Value::Nil)
                    }
                    Ok(()) => Ok(Value::Nil),
                }
            };
        let finish2 = finish.clone();
        sh.set(
            "RunScriptAsync",
            lua.create_function(
                move |lua,
                      (_t, file, arg, done, _p): (
                    Table,
                    String,
                    Value,
                    Option<Function>,
                    Value,
                )| {
                    let txt = read_pack_file(&eng2, &file).map_err(to_lua)?;
                    finish(lua, txt, file, arg, done)
                },
            )?,
        )?;
        sh.set(
            "RunStringAsync",
            lua.create_function(
                move |lua, (_t, src, arg, done, _p): (Table, String, Value, Option<Function>, Value)| {
                    finish2(lua, src, "script".to_string(), arg, done)
                },
            )?,
        )?;
    }
    for noop in ["AsyncProgress", "PushMarkdownNotification"] {
        sh.set(noop, lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    }
    // What a pack passes to `PushMarkdownNotification`, an EmoTracker call some
    // still make: PopTracker lacks both, but they cost nothing here.
    let kinds = lua.create_table()?;
    for (i, k) in ["Message", "Warning", "Error", "Celebration"]
        .iter()
        .enumerate()
    {
        kinds.set(*k, i)?;
    }
    g.set("NotificationType", kinds)?;
    g.set("ScriptHost", &sh)?;

    Ok(())
}

/// The module name `ScriptHost:LoadScript` passes a script as `...`:
/// `scripts/a/b.lua` is `a.b`.
fn load_script_module(file: &str) -> String {
    let name = file.strip_prefix("scripts/").unwrap_or(file);
    let name = name
        .strip_suffix(".lua")
        .or_else(|| name.strip_suffix(".LUA"))
        .unwrap_or(name);
    name.replace('/', ".")
}
