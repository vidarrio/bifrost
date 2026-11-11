//! Pack JSON (JSONC) parsing, and the serde_json to Lua bridge.

use crate::Result;
use mlua::{Lua, Value};

/// Parse a pack JSON file tolerating JSONC/JSON5 (comments + trailing commas),
/// which PopTracker packs use freely; strict `serde_json` rejects those.
pub(crate) fn parse_pack_json(txt: &str) -> std::result::Result<serde_json::Value, String> {
    if nesting(txt) > MAX_NESTING {
        return Err(format!("nested more than {MAX_NESTING} deep"));
    }
    json5::from_str::<serde_json::Value>(txt).map_err(|e| e.to_string())
}

/// The deepest a pack's JSON may nest. The parser recurses without a limit of
/// its own, and real packs stay within a few dozen levels.
const MAX_NESTING: usize = 256;

/// How deep `txt`'s arrays and objects nest, skipping strings and comments.
/// Malformed input only has to come out no shallower than the parser sees it.
fn nesting(txt: &str) -> usize {
    let (mut depth, mut deepest) = (0usize, 0usize);
    let mut chars = txt.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '[' | '{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            ']' | '}' => depth = depth.saturating_sub(1),
            '"' | '\'' => {
                while let Some(d) = chars.next() {
                    match d {
                        '\\' => {
                            chars.next();
                        }
                        d if d == c => break,
                        _ => {}
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                // Where the parser ends one: any JSON5 line terminator.
                for d in chars.by_ref() {
                    if json5::char::is_json5_line_terminator(d) {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut star = false;
                for d in chars.by_ref() {
                    if star && d == '/' {
                        break;
                    }
                    star = d == '*';
                }
            }
            _ => {}
        }
    }
    deepest
}

// ---- serde_json -> Lua bridge (slot_data / data-storage values) --------------

pub(crate) fn json_to_lua(lua: &Lua, v: &serde_json::Value) -> Result<Value> {
    Ok(match v {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else {
                Value::Number(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::String(lua.create_string(s)?),
        serde_json::Value::Array(a) => {
            let t = lua.create_table()?;
            for (i, e) in a.iter().enumerate() {
                t.set(i as i64 + 1, json_to_lua(lua, e)?)?;
            }
            Value::Table(t)
        }
        serde_json::Value::Object(o) => {
            let t = lua.create_table()?;
            for (k, e) in o {
                t.set(k.as_str(), json_to_lua(lua, e)?)?;
            }
            Value::Table(t)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_json_nesting_is_capped() {
        let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(parse_pack_json(&deep(MAX_NESTING)).is_ok());
        assert!(parse_pack_json(&deep(MAX_NESTING + 1)).is_err());
        // A pack this deep used to be refused by the old parser; the new one
        // would overflow the stack.
        assert!(parse_pack_json(&deep(100_000)).is_err());
        // Brackets in strings and comments don't count.
        let txt = format!(
            "{{\"a\": \"{}\", 'b': '\\'{}', /* {} */ // {}\n \"c\": [1]}}",
            "[".repeat(300),
            "{".repeat(300),
            "[".repeat(300),
            "{".repeat(300)
        );
        assert_eq!(nesting(&txt), 2);
        assert!(parse_pack_json(&txt).is_ok());
        // A line comment ends wherever the parser ends it.
        for end in ["\n", "\r", "\u{2028}", "\u{2029}"] {
            let txt = format!("// x{end}{}", deep(MAX_NESTING + 1));
            assert!(parse_pack_json(&txt).is_err(), "{end:?}");
        }
    }

    #[test]
    fn parses_jsonc_with_comments_and_trailing_commas() {
        // PopTracker packs use JSONC; strict JSON would reject both of these.
        let txt = r#"[
            // a comment
            { "name": "A", "codes": "a", },  /* block */
            { "name": "B", "codes": "b" },
        ]"#;
        let v = parse_pack_json(txt).expect("JSONC should parse");
        assert_eq!(v.as_array().unwrap().len(), 2);
    }
}
