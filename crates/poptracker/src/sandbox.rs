//! The sandboxed Lua VM, and helpers for the errors pack Lua raises.

use crate::engine::{E, MAX_PACK_ERROR_LEN, MAX_PACK_ERRORS};
use crate::{Error, Result};
use mlua::{Lua, LuaOptions, StdLib, Value};

/// Memory ceiling for the sandboxed Lua VM (guards runaway allocation).
const LUA_MEMORY_LIMIT: usize = 256 * 1024 * 1024;

/// Keep a pack handler's error for the tracker to report (see `MAX_PACK_ERRORS`).
pub(crate) fn note_error(eng: &E, text: &str) {
    let mut e = eng.borrow_mut();
    e.pack_error_count += 1;
    if e.pack_errors.len() < MAX_PACK_ERRORS {
        let end = (0..=text.len().min(MAX_PACK_ERROR_LEN))
            .rev()
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(0);
        e.pack_errors.push(text[..end].to_string());
    }
}

/// Is this Lua running out of memory, however many callbacks it came through?
pub(crate) fn is_memory_error(e: &mlua::Error) -> bool {
    match e {
        mlua::Error::MemoryError(_) => true,
        mlua::Error::CallbackError { cause, .. } => is_memory_error(cause),
        _ => false,
    }
}

pub(crate) fn to_lua(e: Error) -> mlua::Error {
    mlua::Error::runtime(e.to_string())
}

/// Run a pack's Lua file, as source only: a chunk that starts with Lua's
/// bytecode signature is refused (see `sandboxed_lua`).
pub(crate) fn run_pack_source(lua: &Lua, src: &str, name: &str) -> mlua::Result<()> {
    lua.load(src)
        .set_name(name)
        .set_mode(mlua::chunk::ChunkMode::Text)
        .exec()
}

/// Build the sandboxed Lua VM: only string/table/math/coroutine/utf8 stdlib
/// (base functions like `print`/`pairs` are always present) and PopTracker's
/// four harmless `os` functions, i.e. no `io`/`package`/`debug`, the rest of
/// `os` or Lua's own `require` (the host adds a pack-only one), with a memory
/// cap. Pack Lua runs here, so this
/// is the boundary that keeps an uploaded pack from touching the host.
pub(crate) fn sandboxed_lua() -> Result<Lua> {
    let lua = Lua::new_with(
        StdLib::STRING
            | StdLib::TABLE
            | StdLib::MATH
            | StdLib::COROUTINE
            | StdLib::OS
            | StdLib::UTF8,
        LuaOptions::default(),
    )?;
    lua.set_memory_limit(LUA_MEMORY_LIMIT)?;
    // `dofile`/`loadfile` are BASE-library functions in Lua 5.4 (not `package`),
    // so excluding the package lib doesn't drop them — and they read+run files
    // off disk. Remove them explicitly; they're the obvious filesystem escape.
    {
        let g = lua.globals();
        for f in ["dofile", "loadfile"] {
            let _ = g.set(f, Value::Nil);
        }
    }
    // Source code only, never precompiled chunks: Lua 5.4 doesn't verify
    // bytecode, and crafted bytecode can break out of the sandbox. `load`
    // always gets mode "t" (its env argument is passed on only when given,
    // since an explicit nil would mean an empty environment), and
    // `string.dump`, the way to produce bytecode from inside, is removed.
    // Pack files are loaded as text by the host as well (ChunkMode::Text).
    lua.load(
        r##"
        local load, select = load, select
        _G.load = function(chunk, name, _mode, ...)
          if select("#", ...) > 0 then return load(chunk, name, "t", ...) end
          return load(chunk, name, "t")
        end
        string.dump = nil
        -- Only what can't touch the host: the rest of `os` runs programs and
        -- removes files, and is out of reach once this table replaces it.
        local os = os
        _G.os = { clock = os.clock, date = os.date, difftime = os.difftime, time = os.time }
        "##,
    )
    .set_name("sandbox")
    .set_mode(mlua::chunk::ChunkMode::Text)
    .exec()?;
    Ok(lua)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Pack, Session};

    /// Bytecode for `x = 1`, compiled outside the sandbox.
    fn bytecode() -> Vec<u8> {
        Lua::new().load("x = 1").into_function().unwrap().dump(true)
    }

    #[test]
    fn sandbox_load_takes_source_only() {
        let lua = sandboxed_lua().unwrap();
        // Source still loads, with the globals as its environment by default...
        let n: i64 = lua
            .load(r#"return load("return math.floor(2.5)")()"#)
            .eval()
            .unwrap();
        assert_eq!(n, 2);
        // ...or an explicit one.
        let v: i64 = lua
            .load(r#"return load("return y", "c", "t", { y = 7 })()"#)
            .eval()
            .unwrap();
        assert_eq!(v, 7);
        // Binary chunks are refused, even when the caller asks for mode "b".
        lua.globals()
            .set("BC", lua.create_string(bytecode()).unwrap())
            .unwrap();
        for call in [
            r#"return load(BC, "bc", "b")"#,
            r#"return load(BC)"#,
            r#"local s = BC; return load(function() local r = s; s = nil; return r end)"#,
        ] {
            let (f, err): (Value, Option<String>) = lua.load(call).eval().unwrap();
            assert!(f.is_nil(), "a binary chunk must not load: {call}");
            let err = err.unwrap_or_default();
            assert!(err.contains("binary"), "{call}: {err}");
        }
        // And there's no way to make bytecode inside.
        let dump: Value = lua.load("return string.dump").eval().unwrap();
        assert!(dump.is_nil());
    }

    #[test]
    fn pack_scripts_must_be_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("scripts")).unwrap();
        std::fs::write(dir.path().join("scripts/init.lua"), bytecode()).unwrap();
        let pack = Pack::load(dir.path()).unwrap();
        // Real bytecode always contains bytes that aren't valid UTF-8, so
        // reading the file refuses it first.
        let err = Session::new(&pack)
            .err()
            .expect("a precompiled init.lua must be refused");
        assert!(err.to_string().contains("binary"), "{err}");
    }

    #[test]
    fn pack_files_run_as_text_only() {
        // The backstop behind the UTF-8 read: a chunk that starts with the
        // bytecode signature is refused by the text-only mode itself.
        let lua = sandboxed_lua().unwrap();
        let err = run_pack_source(&lua, "\x1bLua x = 1", "fake.lua").unwrap_err();
        // Refused for being binary at all, not merely for being malformed
        // ("bad binary format"), which is what happens without the mode.
        assert!(err.to_string().contains("mode is 't'"), "{err}");
        run_pack_source(&lua, "x = 1", "ok.lua").unwrap();
    }

    #[test]
    fn sandbox_excludes_dangerous_stdlib() {
        let lua = sandboxed_lua().unwrap();
        let g = lua.globals();
        for banned in ["io", "package", "require", "dofile", "loadfile", "debug"] {
            assert!(
                matches!(g.get::<Value>(banned).unwrap(), Value::Nil),
                "{banned} must be unavailable to pack Lua"
            );
        }
        let os: mlua::Table = g.get("os").unwrap();
        let mut names: Vec<String> = os
            .pairs::<String, Value>()
            .map(|kv| kv.unwrap().0)
            .collect();
        names.sort();
        assert_eq!(names, ["clock", "date", "difftime", "time"]);
        for allowed in ["string", "table", "math", "print", "pairs", "type"] {
            assert!(
                !matches!(g.get::<Value>(allowed).unwrap(), Value::Nil),
                "{allowed} should be present"
            );
        }
    }
}
