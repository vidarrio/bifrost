//! The instance's catalog of common games: co-op games the sysadmin hosts
//! (a Minecraft server, a Factorio server, ...) that everyone in a season
//! plays as one shared Archipelago slot. Each setup is a folder under the data
//! dir's `common/`, named by its key, with a `game.toml`:
//!
//! ```toml
//! name = "Minecraft 26.2"          # shown on the board and the game page
//! slot = "Vanilla"                 # the player name its YAML declares
//! yaml = "player.yaml"             # (default) the slot's player options
//! apworld = "minecraft.apworld"    # a world Archipelago doesn't ship, if any
//! address = "mc.example:25565"     # where players join; a season can override it
//! guide = "guide.md"               # (default) how to join, in Markdown
//!
//! [identifier]                     # the name players register for the allowlist
//! kind = "minecraft"               # setups of one kind share players' names
//! label = "Minecraft username"
//! pattern = "[A-Za-z0-9_]{3,16}"
//! hint = "3 to 16 letters, digits or underscores"
//!
//! [[download]]                     # client files the game page offers
//! file = "client/aprandomizer-2.2.1-26.2.jar"
//! label = "Archipelago Randomizer mod"
//! ```
//!
//! Bifrost never reaches the game's server: the sysadmin deploys the seed's
//! file for the slot and applies the allowlist, by hand or with their own sync.

use std::path::{Component, Path, PathBuf};

use crate::config;

/// The largest `game.toml` or guide read.
const MAX_TEXT: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Setup {
    pub key: String,
    pub name: String,
    pub slot: String,
    /// The YAML's file in the setup's folder.
    pub yaml: String,
    pub apworld: Option<String>,
    pub address: String,
    /// The join guide's Markdown (empty if none).
    pub guide: String,
    pub identifier: Option<Identifier>,
    pub downloads: Vec<Download>,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identifier {
    pub kind: String,
    pub label: String,
    pub pattern: String,
    #[serde(default)]
    pub hint: String,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Download {
    /// Relative to the setup's folder.
    pub file: String,
    pub label: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    name: String,
    slot: String,
    #[serde(default)]
    yaml: Option<String>,
    #[serde(default)]
    apworld: Option<String>,
    #[serde(default)]
    address: String,
    #[serde(default)]
    guide: Option<String>,
    #[serde(default)]
    identifier: Option<Identifier>,
    #[serde(default)]
    download: Vec<Download>,
}

/// A setup's key, which is its folder's name: lower-case letters, digits and
/// `.`, `_` or `-`, starting with a letter or digit, up to 40 characters.
pub fn valid_key(key: &str) -> bool {
    let b = key.as_bytes();
    (1..=40).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')
        })
}

/// An identifier's kind: lower-case letters, digits, `_` or `-`.
fn valid_kind(kind: &str) -> bool {
    (1..=32).contains(&kind.len())
        && kind
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-'))
}

/// A file inside `dir`, by a relative path with no way out of it: no `..`, no
/// absolute path, and no link anywhere along it.
fn inside(dir: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel);
    if rel.is_empty() || !p.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(format!(
            "{rel}: needs to be a path inside the setup's folder"
        ));
    }
    let mut at = dir.to_path_buf();
    for c in p.components() {
        at.push(c);
        let meta = std::fs::symlink_metadata(&at).map_err(|_| format!("{rel}: not found"))?;
        if meta.file_type().is_symlink() {
            return Err(format!("{rel}: links aren't followed"));
        }
    }
    if !std::fs::symlink_metadata(&at).is_ok_and(|m| m.is_file()) {
        return Err(format!("{rel}: not a file"));
    }
    Ok(at)
}

fn read_text(path: &Path) -> Result<String, String> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let meta = std::fs::metadata(path).map_err(|e| format!("{name}: {e}"))?;
    if meta.len() > MAX_TEXT {
        return Err(format!("{name}: over {} KiB", MAX_TEXT / 1024));
    }
    std::fs::read_to_string(path).map_err(|e| format!("{name}: {e}"))
}

/// A YAML's top-level `name:`, when it's a plain one-line value. A YAML that
/// says it some other way is checked once the seed is rolled.
fn yaml_name(text: &str) -> Option<String> {
    let line = text.lines().find(|l| l.starts_with("name:"))?;
    let v = line["name:".len()..].trim();
    let v = v.split(" #").next().unwrap_or(v).trim();
    let v = v
        .strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .or_else(|| v.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
        .unwrap_or(v);
    (!v.is_empty() && !v.contains(['"', '\'', '{', '['])).then(|| v.to_string())
}

/// The identifier's pattern as a whole-name match.
pub fn name_rule(pattern: &str) -> Result<regex::Regex, String> {
    regex::RegexBuilder::new(&format!("^(?:{pattern})$"))
        .size_limit(1 << 20)
        .build()
        .map_err(|e| {
            let why = e.to_string();
            let why = why.lines().last().unwrap_or("").trim().to_string();
            format!("identifier.pattern isn't a valid regular expression ({why})")
        })
}

impl Identifier {
    /// Whether a player's name fits the setup's rule.
    pub fn accepts(&self, name: &str) -> bool {
        name_rule(&self.pattern).is_ok_and(|r| fits(&r, name))
    }

    /// The names of `names` that fit the setup's rule.
    pub fn keep(&self, names: &mut Vec<String>) {
        match name_rule(&self.pattern) {
            Ok(r) => names.retain(|n| fits(&r, n)),
            Err(_) => names.clear(),
        }
    }
}

/// Whatever the setup's rule, a name is one line of at most 64 characters,
/// without control characters or Unicode's line and paragraph separators, as
/// the allowlist goes out one name per line.
fn fits(rule: &regex::Regex, name: &str) -> bool {
    (1..=64).contains(&name.chars().count())
        && !name
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
        && rule.is_match(name)
}

impl Identifier {}

/// Read one setup from `root/<key>/`.
pub fn load_from(root: &Path, key: &str) -> Result<Setup, String> {
    if !valid_key(key) {
        return Err(
            "the folder's name isn't a valid key (lower-case letters, digits, '.', '_' or '-')"
                .into(),
        );
    }
    let dir = root.join(key);
    if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
        return Err("not in the catalog".into());
    }
    let text = read_text(&inside(&dir, "game.toml")?)?;
    let m: Manifest = toml::from_str(&text).map_err(|e| {
        format!(
            "game.toml: {}",
            e.message().lines().next().unwrap_or("invalid TOML")
        )
    })?;
    let name = m.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
        return Err("name: needs 1 to 60 characters, on one line".into());
    }
    let slot = crate::worlds::slot_name_of(&m.slot).map_err(|e| format!("slot: {e}"))?;
    let yaml = m.yaml.unwrap_or_else(|| "player.yaml".into());
    let yaml_text = read_text(&inside(&dir, &yaml).map_err(|e| format!("yaml: {e}"))?)?;
    if let Some(name) = yaml_name(&yaml_text)
        && !name.eq_ignore_ascii_case(&slot)
    {
        return Err(format!("yaml: its name: is {name}, not the slot {slot}"));
    }
    if let Some(w) = &m.apworld {
        if w.contains('/') || !w.ends_with(".apworld") || w == "tracker.apworld" {
            return Err("apworld: needs to be an .apworld file in the setup's folder".into());
        }
        inside(&dir, w).map_err(|e| format!("apworld: {e}"))?;
    }
    let address = m.address.trim().to_string();
    if address.chars().count() > 200 || address.chars().any(char::is_control) {
        return Err("address: needs to be one line, up to 200 characters".into());
    }
    let guide = match m.guide {
        Some(g) => read_text(&inside(&dir, &g).map_err(|e| format!("guide: {e}"))?)?,
        None => match inside(&dir, "guide.md") {
            Ok(p) => read_text(&p)?,
            Err(_) => String::new(),
        },
    };
    if let Some(id) = &m.identifier {
        if !valid_kind(&id.kind) {
            return Err("identifier.kind: lower-case letters, digits, '_' or '-'".into());
        }
        if id.label.trim().is_empty() || id.label.chars().count() > 60 {
            return Err("identifier.label: needs 1 to 60 characters".into());
        }
        name_rule(&id.pattern)?;
    }
    for d in &m.download {
        inside(&dir, &d.file).map_err(|e| format!("download: {e}"))?;
        if d.label.trim().is_empty() {
            return Err(format!("download {}: needs a label", d.file));
        }
    }
    Ok(Setup {
        key: key.to_string(),
        name,
        slot,
        yaml,
        apworld: m.apworld,
        address,
        guide,
        identifier: m.identifier,
        downloads: m.download,
    })
}

/// Every folder in `root`, read as a setup or with why it can't be, by key.
pub fn catalog_from(root: &Path) -> Vec<(String, Result<Setup, String>)> {
    let mut keys: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    keys.sort();
    keys.into_iter()
        .map(|k| {
            let s = load_from(root, &k);
            (k, s)
        })
        .collect()
}

pub fn load(key: &str) -> Result<Setup, String> {
    load_from(&config::get().common_dir(), key)
}

pub fn catalog() -> Vec<(String, Result<Setup, String>)> {
    catalog_from(&config::get().common_dir())
}

/// A file of a setup's folder (its YAML, apworld or a download), by its
/// relative path as the setup names it.
pub fn file(key: &str, rel: &str) -> Result<PathBuf, String> {
    if !valid_key(key) {
        return Err("not in the catalog".into());
    }
    inside(&config::get().common_dir().join(key), rel)
}

/// The apworld file names the catalog's setups ship, which uploads can't take.
pub fn apworld_files() -> Vec<String> {
    catalog()
        .into_iter()
        .filter_map(|(_, s)| s.ok()?.apworld)
        .collect()
}

/// The apworlds the catalog ships, by path, for trusting them by content.
pub fn apworld_paths() -> Vec<PathBuf> {
    catalog()
        .into_iter()
        .filter_map(|(k, s)| {
            let w = s.ok()?.apworld?;
            file(&k, &w).ok()
        })
        .collect()
}

/// A setup's guide as HTML. The guide is the sysadmin's, but it's still
/// written to a page every player opens, so raw HTML in it is shown as text
/// and links only go to web addresses or within Bifrost. `{address}` and
/// `{slot}` in its text are the season's server address and slot name.
pub fn guide_html(md: &str, address: &str, slot: &str) -> String {
    use pulldown_cmark::{CowStr, Event, Options, Parser, Tag};
    let fill = |t: &str| t.replace("{address}", address).replace("{slot}", slot);
    let safe_url = |u: &str| {
        let lower = u.trim_start().to_ascii_lowercase();
        match lower.find(':') {
            // A scheme only before any '/', '?' or '#'.
            Some(i) if !lower[..i].contains(['/', '?', '#']) => {
                lower.starts_with("https:") || lower.starts_with("http:")
            }
            _ => true,
        }
    };
    let events =
        Parser::new_ext(md, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH).map(|ev| {
            match ev {
                Event::Html(h) | Event::InlineHtml(h) => Event::Text(h),
                Event::Text(t) => Event::Text(CowStr::from(fill(&t))),
                Event::Code(t) => Event::Code(CowStr::from(fill(&t))),
                Event::Start(Tag::Link {
                    link_type,
                    dest_url,
                    title,
                    id,
                }) => Event::Start(Tag::Link {
                    link_type,
                    dest_url: if safe_url(&dest_url) {
                        dest_url
                    } else {
                        CowStr::from("#")
                    },
                    title,
                    id,
                }),
                Event::Start(Tag::Image {
                    link_type,
                    dest_url,
                    title,
                    id,
                }) => Event::Start(Tag::Image {
                    link_type,
                    dest_url: if safe_url(&dest_url) {
                        dest_url
                    } else {
                        CowStr::from("#")
                    },
                    title,
                    id,
                }),
                e => e,
            }
        });
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, events);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bifrost-common-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn setup(root: &Path, key: &str, toml: &str) {
        let d = root.join(key);
        std::fs::create_dir_all(d.join("client")).unwrap();
        std::fs::write(d.join("game.toml"), toml).unwrap();
        std::fs::write(d.join("player.yaml"), "name: Vanilla\n").unwrap();
        std::fs::write(d.join("minecraft.apworld"), "zip").unwrap();
        std::fs::write(d.join("client/mod.jar"), "jar").unwrap();
    }

    #[test]
    fn a_setup_reads_from_its_folder() {
        let r = root("read");
        setup(
            &r,
            "vanilla.26.2",
            r#"name = "Minecraft 26.2"
slot = "Vanilla"
apworld = "minecraft.apworld"
address = "mc.example:25565"
[identifier]
kind = "minecraft"
label = "Minecraft username"
pattern = "[A-Za-z0-9_]{3,16}"
[[download]]
file = "client/mod.jar"
label = "The mod"
"#,
        );
        std::fs::write(r.join("vanilla.26.2/guide.md"), "Join **{address}**").unwrap();
        let s = load_from(&r, "vanilla.26.2").unwrap();
        assert_eq!(s.name, "Minecraft 26.2");
        assert_eq!(s.slot, "Vanilla");
        assert_eq!(s.yaml, "player.yaml");
        assert_eq!(s.apworld.as_deref(), Some("minecraft.apworld"));
        assert_eq!(s.guide, "Join **{address}**");
        let id = s.identifier.unwrap();
        assert!(id.accepts("Steve_01"));
        assert!(!id.accepts("St"));
        assert!(!id.accepts("Steve; rm"));
        let loose = Identifier {
            pattern: ".{3,90}".into(),
            ..id.clone()
        };
        assert!(!loose.accepts("Ann\rBob"), "one line");
        assert!(
            !loose.accepts("Ann\u{2028}Bob"),
            "nor Unicode's line separator"
        );
        assert!(
            !loose.accepts("Ann\u{2029}Bob"),
            "nor its paragraph separator"
        );
        let mut names = vec!["Ann".to_string(), "Ann\u{2028}Bob".to_string()];
        loose.keep(&mut names);
        assert_eq!(names, ["Ann"]);
        assert!(!loose.accepts(&"x".repeat(65)), "at most 64");
        let unicode = Identifier {
            pattern: "(?i)\\p{L}+".into(),
            ..id.clone()
        };
        assert!(unicode.accepts("Åsa"));
        assert_eq!(s.downloads.len(), 1);
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn a_broken_setup_says_why() {
        let r = root("broken");
        let base = "name = \"X\"\nslot = \"Vanilla\"\n";
        for (toml, why) in [
            ("name = \"X\"\n", "slot"),
            (
                &format!("{base}apworld = \"../x.apworld\"\n") as &str,
                "apworld",
            ),
            (&format!("{base}apworld = \"tracker.apworld\"\n"), "apworld"),
            (&format!("{base}yaml = \"nope.yaml\"\n"), "yaml"),
            (
                &format!("{base}[identifier]\nkind = \"MC\"\nlabel = \"x\"\npattern = \"a\"\n"),
                "kind",
            ),
            (
                &format!("{base}[identifier]\nkind = \"mc\"\nlabel = \"x\"\npattern = \"(\"\n"),
                "pattern",
            ),
            (
                &format!("{base}[[download]]\nfile = \"/etc/passwd\"\nlabel = \"x\"\n"),
                "download",
            ),
            (
                &format!("{base}[[download]]\nfile = \"client/../../x\"\nlabel = \"x\"\n"),
                "download",
            ),
            (&format!("{base}surprise = 1\n"), "game.toml"),
            ("name = \"X\"\nslot = \"{player}\"\n", "slot"),
            ("name = \"X\"\nslot = \"Other\"\n", "not the slot"),
        ] {
            setup(&r, "s", toml);
            let e = load_from(&r, "s").unwrap_err();
            assert!(e.contains(why), "{toml:?}: {e}");
        }
        // A link out of the folder isn't followed.
        setup(
            &r,
            "s",
            "name = \"X\"\nslot = \"Vanilla\"\n[[download]]\nfile = \"out\"\nlabel = \"x\"\n",
        );
        std::os::unix::fs::symlink("/etc/passwd", r.join("s/out")).unwrap();
        assert!(load_from(&r, "s").unwrap_err().contains("link"));
        assert!(load_from(&r, "Bad Key").is_err());
        assert!(
            load_from(&r, "missing")
                .unwrap_err()
                .contains("not in the catalog")
        );
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn the_example_setup_reads() {
        let r = root("example");
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/examples/common/minecraft-26.2");
        let d = r.join("minecraft-26.2");
        std::fs::create_dir_all(d.join("client")).unwrap();
        for f in ["game.toml", "player.yaml", "guide.md"] {
            std::fs::copy(src.join(f), d.join(f)).unwrap();
        }
        // What it leaves out for the sysadmin to add.
        for f in [
            "minecraft.apworld",
            "client/neoforge-26.2.0.90-installer.jar",
            "client/aprandomizer-2.2.1-26.2.jar",
        ] {
            std::fs::write(d.join(f), "x").unwrap();
        }
        let s = load_from(&r, "minecraft-26.2").unwrap();
        assert!(
            std::fs::read_to_string(d.join("player.yaml"))
                .unwrap()
                .contains(&format!("name: {}\n", s.slot))
        );
        assert!(
            guide_html(&s.guide, "mc.example:25565", &s.slot)
                .contains("<strong>mc.example:25565</strong>")
        );
        assert!(s.identifier.unwrap().accepts("Steve"));
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn keys_are_plain() {
        for k in ["vanilla.26.2", "atm10", "factorio-2_0"] {
            assert!(valid_key(k), "{k}");
        }
        for k in ["", ".hidden", "Vanilla", "a/b", "a b", &"x".repeat(41)] {
            assert!(!valid_key(k), "{k}");
        }
    }

    #[test]
    fn a_guide_cant_run_script() {
        let html = guide_html(
            "Join <b>{address}</b> as `{slot}`\n\n<script>alert(1)</script>\n\n[x](javascript:alert(1)) [y](https://ok.example) [z](game/common/a/0)\n\n![i](data:text/html,x)",
            "mc.example:25565",
            "Vanilla",
        );
        assert!(html.contains("mc.example:25565"), "{html}");
        assert!(html.contains("<code>Vanilla</code>"), "{html}");
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("<b>"), "{html}");
        assert!(!html.contains("javascript:"), "{html}");
        assert!(!html.contains("data:"), "{html}");
        assert!(html.contains(r#"href="https://ok.example""#), "{html}");
        assert!(html.contains(r#"href="game/common/a/0""#), "{html}");
    }

    #[test]
    fn a_filled_in_address_is_text() {
        let html = guide_html("Join {address}", "<img src=x onerror=alert(1)>", "S");
        assert!(!html.contains("<img"), "{html}");
    }
}
