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

/// A release newer than this build.
#[derive(Clone, Debug, PartialEq)]
pub struct Newer {
    pub version: String,
    pub url: String,
}

/// What the last check found: when, and the newer release (if any).
pub type Checked = (i64, Result<Option<Newer>, String>);

static LAST: Mutex<Option<Checked>> = Mutex::new(None);

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn enabled() -> bool {
    config::var("BIFROST_UPDATE_CHECK").is_none_or(|v| {
        !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        )
    })
}

/// The last check's outcome, `None` before the first.
pub fn last() -> Option<Checked> {
    LAST.lock().unwrap().clone()
}

/// The newer release the last check found.
pub fn newer() -> Option<Newer> {
    last().and_then(|(_, r)| r.ok().flatten())
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
            check().await;
            tokio::time::sleep(EVERY).await;
        }
    });
}

async fn check() {
    let res = apupdate::releases(REPO)
        .await
        .map(|rs| pick(&rs, current()));
    match &res {
        Ok(Some(n)) if newer().as_ref() != Some(n) => {
            tracing::info!(
                "Bifrost {} is out (this is {}): {}",
                n.version,
                current(),
                n.url
            )
        }
        Err(e) => tracing::warn!("checking for newer Bifrost releases: {e}"),
        _ => {}
    }
    *LAST.lock().unwrap() = Some((crate::auth::now(), res));
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
