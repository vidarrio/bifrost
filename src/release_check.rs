//! Telling sysadmins a newer Bifrost is out. Once a day Bifrost asks GitHub
//! for its own releases (through the same helper that lists Archipelago
//! releases, so `GITHUB_TOKEN` applies) and remembers the newest one that's
//! newer than this build. The console shows it to sysadmins; nothing is
//! downloaded or installed. `BIFROST_UPDATE_CHECK=off` turns it off.

use std::sync::Mutex;
use std::time::Duration;

use crate::apupdate::{self, Release};
use crate::config;

const REPO: &str = "vidarrio/bifrost";
/// The first check waits a little, so a restart loop doesn't hammer GitHub.
const FIRST: Duration = Duration::from_secs(5 * 60);
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// After a failed check, try again sooner.
const RETRY: Duration = Duration::from_secs(60 * 60);

/// A release newer than this build.
#[derive(Clone, Debug, PartialEq)]
pub struct Newer {
    pub version: String,
    pub url: String,
}

/// What the last check found: when, and the newer release (if any).
pub type Checked = (i64, Result<Option<Newer>, String>);

static LAST: Mutex<Option<Checked>> = Mutex::new(None);
/// The newest newer release any check found: a failed check doesn't hide it.
static NEWER: Mutex<Option<Newer>> = Mutex::new(None);

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn enabled() -> bool {
    config::var("BIFROST_UPDATE_CHECK").is_none_or(|v| switch(&v))
}

/// `BIFROST_UPDATE_CHECK`'s value: on unless it says off. Anything else is
/// on, with a warning.
fn switch(v: &str) -> bool {
    match v.trim().to_ascii_lowercase().as_str() {
        "0" | "false" | "off" | "no" => false,
        "1" | "true" | "on" | "yes" => true,
        _ => {
            tracing::warn!("BIFROST_UPDATE_CHECK={v:?} isn't on or off; checking for updates");
            true
        }
    }
}

/// The last check's outcome, `None` before the first.
pub fn last() -> Option<Checked> {
    LAST.lock().unwrap().clone()
}

/// The newer release the checks found, kept through failed checks.
pub fn newer() -> Option<Newer> {
    NEWER.lock().unwrap().clone()
}

/// The daily check loop.
pub fn spawn() {
    if !enabled() {
        tracing::info!("checks for newer Bifrost releases are off (BIFROST_UPDATE_CHECK)");
        return;
    }
    tokio::spawn(async {
        tokio::time::sleep(FIRST).await;
        loop {
            let wait = if check().await { EVERY } else { RETRY };
            tokio::time::sleep(wait).await;
        }
    });
}

/// One check; whether it reached GitHub.
async fn check() -> bool {
    let res = apupdate::releases(REPO)
        .await
        .map(|rs| pick(&rs, current()));
    let ok = res.is_ok();
    record(crate::auth::now(), res);
    ok
}

fn record(at: i64, res: Result<Option<Newer>, String>) {
    match &res {
        Ok(found) => {
            let mut newer = NEWER.lock().unwrap();
            if let Some(n) = found
                && newer.as_ref() != Some(n)
            {
                tracing::info!(
                    "Bifrost {} is out (this is {}): {}",
                    n.version,
                    current(),
                    n.url
                );
            }
            newer.clone_from(found);
        }
        Err(e) => tracing::warn!("checking for newer Bifrost releases: {e}"),
    }
    *LAST.lock().unwrap() = Some((at, res));
}

/// The newest release newer than `current`. Pre-releases count only when
/// `current` is one itself.
fn pick(releases: &[Release], current: &str) -> Option<Newer> {
    let cur = parse(current)?;
    let pre_ok = cur.1.is_some();
    releases
        .iter()
        .filter_map(|r| Some((parse(&r.tag)?, r)))
        .filter(|((_, pre), r)| pre_ok || (pre.is_none() && !r.prerelease))
        .filter(|(v, _)| newer_than(v, &cur))
        .max_by(|(a, _), (b, _)| {
            if newer_than(a, b) {
                std::cmp::Ordering::Greater
            } else if newer_than(b, a) {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .map(|(_, r)| Newer {
            version: r.tag.trim_start_matches('v').to_string(),
            url: format!("https://github.com/{REPO}/releases/tag/{}", r.tag),
        })
}

type Version = ((u64, u64, u64), Option<String>);

/// `v1.2.3` / `1.2.3-rc.1` → its numbers and pre-release suffix.
fn parse(v: &str) -> Option<Version> {
    let v = v.trim().trim_start_matches('v');
    let v = v.split('+').next()?;
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p.to_string())),
        None => (v, None),
    };
    let mut n = core.split('.').map(|p| p.parse::<u64>().ok());
    let v = (n.next()??, n.next()??, n.next()??);
    n.next().is_none().then_some((v, pre))
}

/// Semver precedence, close enough for release tags: numbers first, then a
/// release beats its pre-releases, which compare part by part.
fn newer_than(a: &Version, b: &Version) -> bool {
    if a.0 != b.0 {
        return a.0 > b.0;
    }
    match (&a.1, &b.1) {
        (None, Some(_)) => true,
        (Some(x), Some(y)) => {
            let part = |p: &str| (p.parse::<u64>().ok(), p.to_string());
            let (xs, ys): (Vec<_>, Vec<_>) = (
                x.split('.').map(part).collect(),
                y.split('.').map(part).collect(),
            );
            for (p, q) in xs.iter().zip(&ys) {
                let ord = match (p, q) {
                    ((Some(m), _), (Some(n), _)) => m.cmp(n),
                    ((Some(_), _), (None, _)) => std::cmp::Ordering::Less,
                    ((None, _), (Some(_), _)) => std::cmp::Ordering::Greater,
                    ((None, s), (None, t)) => s.cmp(t),
                };
                if ord.is_ne() {
                    return ord.is_gt();
                }
            }
            xs.len() > ys.len()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(tag: &str, prerelease: bool) -> Release {
        Release {
            tag: tag.into(),
            name: tag.into(),
            prerelease,
            published: String::new(),
        }
    }

    #[test]
    fn a_failed_check_keeps_the_newer_release_found_before() {
        let n = Newer {
            version: "9.9.9".into(),
            url: "u".into(),
        };
        record(1, Ok(Some(n.clone())));
        record(2, Err("rate limited".into()));
        assert_eq!(newer(), Some(n));
        assert!(matches!(last(), Some((2, Err(_)))));
        // Once a check finds nothing newer (say, after updating), it's gone.
        record(3, Ok(None));
        assert_eq!(newer(), None);
    }

    #[test]
    fn the_switch_reads_config_booleans_and_words() {
        for off in ["0", "false", "OFF", " no "] {
            assert!(!switch(off), "{off}");
        }
        for on in ["1", "true", "on", "yes", "disabled?"] {
            assert!(switch(on), "{on}");
        }
    }

    #[test]
    fn versions_parse() {
        assert_eq!(parse("v1.2.3"), Some(((1, 2, 3), None)));
        assert_eq!(parse("0.2.0-rc.1"), Some(((0, 2, 0), Some("rc.1".into()))));
        assert_eq!(parse("1.2.3+build.5"), Some(((1, 2, 3), None)));
        assert_eq!(parse("1.2"), None);
        assert_eq!(parse("1.2.3.4"), None);
        assert_eq!(parse("nightly"), None);
    }

    #[test]
    fn precedence() {
        let v = |s: &str| parse(s).unwrap();
        assert!(newer_than(&v("0.1.1"), &v("0.1.0")));
        assert!(newer_than(&v("0.10.0"), &v("0.9.9")));
        assert!(newer_than(&v("1.0.0"), &v("1.0.0-rc.2")));
        assert!(newer_than(&v("1.0.0-rc.10"), &v("1.0.0-rc.2")));
        assert!(newer_than(&v("1.0.0-rc.1"), &v("1.0.0-beta.3")));
        assert!(newer_than(&v("1.0.0-rc.1.1"), &v("1.0.0-rc.1")));
        assert!(!newer_than(&v("1.0.0"), &v("1.0.0")));
        assert!(!newer_than(&v("1.0.0-rc.1"), &v("1.0.0")));
    }

    #[test]
    fn picks_the_newest_newer_release() {
        let rs = [
            rel("v0.3.0-rc.1", true),
            rel("v0.2.1", false),
            rel("v0.2.0", false),
            rel("v0.1.0", false),
            rel("not-a-version", false),
        ];
        assert_eq!(
            pick(&rs, "0.1.0"),
            Some(Newer {
                version: "0.2.1".into(),
                url: "https://github.com/vidarrio/bifrost/releases/tag/v0.2.1".into()
            })
        );
        assert_eq!(pick(&rs, "0.2.1"), None, "up to date");
        assert_eq!(pick(&rs, "0.9.0"), None, "ahead of every release");
        // Someone on a pre-release hears about newer ones.
        assert_eq!(
            pick(&rs, "0.2.1-rc.1").map(|n| n.version),
            Some("0.3.0-rc.1".into())
        );
        // A release GitHub marks as a pre-release counts as one.
        assert_eq!(pick(&[rel("v0.5.0", true)], "0.1.0"), None);
    }
}
