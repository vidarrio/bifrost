//! Rate limiting for auth and expensive endpoints.
//!
//! A token bucket per (rule, caller): signed-in callers are keyed by account,
//! anonymous ones by client IP. The client IP is the TCP peer, or — only when
//! the peer is a trusted proxy (`BIFROST_TRUSTED_PROXIES`) — the right-most
//! untrusted address in `X-Forwarded-For`. In memory; a restart resets it.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::auth::Me;
use crate::net::Cidr;

/// `n` requests per `per`, refilled continuously.
#[derive(Clone, Copy, Debug)]
pub struct Rule {
    pub name: &'static str,
    pub n: u32,
    pub per: Duration,
    /// Key by client IP even when signed in (login-style endpoints).
    pub by_ip: bool,
}

const MIN: Duration = Duration::from_secs(60);

/// Which rule (if any) a state-changing request falls under.
pub fn rule_for(method: &Method, path: &str) -> Option<Rule> {
    if method != Method::POST {
        return None;
    }
    let r = |name, n, mins: u64, by_ip| {
        Some(Rule {
            name,
            n,
            per: MIN * mins as u32,
            by_ip,
        })
    };
    match path {
        // Each check runs a sandboxed AP process loading every world.
        "/check" | "/signup/check" => r("check", 10, 10, false),
        "/signup" => r("signup", 10, 10, false),
        "/signup/apworld" => r("upload", 10, 60, false),
        "/login" => r("login", 20, 10, true),
        "/setup" => r("link", 10, 10, true),
        "/hint" => r("hint", 30, 10, false),
        p if p.starts_with("/invite/") || p.starts_with("/reset/") => r("link", 10, 10, true),
        _ => None,
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

#[derive(Default)]
pub struct Limiter {
    buckets: Mutex<HashMap<(&'static str, String), Bucket>>,
    pub trusted_proxies: Vec<Cidr>,
}

impl Limiter {
    pub fn new(trusted_proxies: Vec<Cidr>) -> Self {
        Self {
            buckets: Mutex::default(),
            trusted_proxies,
        }
    }

    /// Take a token for `key` under `rule`; on refusal, seconds until one frees.
    pub fn take(&self, rule: &Rule, key: &str, now: Instant) -> Result<(), u64> {
        let mut map = self.buckets.lock().unwrap();
        if map.len() > 50_000 {
            // Drop idle buckets (no window exceeds an hour) so a key-spraying
            // client can't grow the map without bound; past that, drop all but
            // the last few minutes' (a full refill of every rule is cheap).
            map.retain(|_, b| now.duration_since(b.last) < Duration::from_secs(3600));
            if map.len() > 50_000 {
                map.retain(|_, b| now.duration_since(b.last) < Duration::from_secs(300));
            }
        }
        let cap = rule.n as f64;
        let rate = cap / rule.per.as_secs_f64();
        let b = map.entry((rule.name, key.to_string())).or_insert(Bucket {
            tokens: cap,
            last: now,
        });
        b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * rate).min(cap);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            Err(((1.0 - b.tokens) / rate).ceil() as u64)
        }
    }

    /// The caller's address, honouring `X-Forwarded-For` only from trusted
    /// proxies.
    pub fn client_ip(&self, headers: &HeaderMap, peer: Option<IpAddr>) -> Option<IpAddr> {
        client_ip(&self.trusted_proxies, headers, peer)
    }
}

pub fn client_ip(trusted: &[Cidr], headers: &HeaderMap, peer: Option<IpAddr>) -> Option<IpAddr> {
    let peer = peer?;
    let is_trusted = |ip: IpAddr| trusted.iter().any(|c| c.contains(ip));
    if !is_trusted(peer) {
        return Some(peer);
    }
    // Walk the chain right to left: the first hop our proxies didn't add.
    let chain: Vec<IpAddr> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    Some(
        chain
            .into_iter()
            .rev()
            .find(|ip| !is_trusted(*ip))
            .unwrap_or(peer),
    )
}

pub async fn middleware(State(limiter): State<Arc<Limiter>>, req: Request, next: Next) -> Response {
    let Some(rule) = rule_for(req.method(), req.uri().path()) else {
        return next.run(req).await;
    };
    let account = req
        .extensions()
        .get::<Me>()
        .map(Me::id)
        .filter(|&id| id > 0);
    let key = match account {
        Some(id) if !rule.by_ip => format!("a{id}"),
        _ => {
            let peer = req
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0.ip());
            match limiter.client_ip(req.headers(), peer) {
                Some(ip) => format!("i{}", crate::net::client_group(ip)),
                None => "unknown".into(),
            }
        }
    };
    match limiter.take(&rule, &key, Instant::now()) {
        Ok(()) => next.run(req).await,
        Err(wait) => {
            tracing::info!("rate limit {} hit by {key}", rule.name);
            let mut resp = (
                StatusCode::TOO_MANY_REQUESTS,
                format!("Too many requests — try again in {wait} s."),
            )
                .into_response();
            if let Ok(v) = HeaderValue::from_str(&wait.to_string()) {
                resp.headers_mut().insert(header::RETRY_AFTER, v);
            }
            resp
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_refuses_then_refills() {
        let l = Limiter::default();
        let rule = Rule {
            name: "t",
            n: 3,
            per: Duration::from_secs(30),
            by_ip: false,
        };
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(l.take(&rule, "k", t0).is_ok());
        }
        let wait = l.take(&rule, "k", t0).unwrap_err();
        assert!((1..=10).contains(&wait), "{wait}");
        assert!(l.take(&rule, "other", t0).is_ok(), "keys are independent");
        assert!(
            l.take(&rule, "k", t0 + Duration::from_secs(10)).is_ok(),
            "one token per 10 s"
        );
        assert!(l.take(&rule, "k", t0 + Duration::from_secs(10)).is_err());
    }

    #[test]
    fn rules_cover_the_expensive_and_auth_endpoints() {
        let post = Method::POST;
        for p in [
            "/check",
            "/signup/check",
            "/signup",
            "/signup/apworld",
            "/login",
            "/setup",
            "/invite/abc",
            "/reset/abc",
            "/hint",
        ] {
            assert!(rule_for(&post, p).is_some(), "{p}");
        }
        assert!(rule_for(&Method::GET, "/check").is_none());
        assert!(rule_for(&post, "/claim").is_none());
        assert!(rule_for(&post, "/login").unwrap().by_ip);
    }

    #[test]
    fn forwarded_for_only_from_trusted_proxies() {
        let trusted = vec![Cidr::parse("10.0.0.0/8").unwrap()];
        let mut h = HeaderMap::new();
        h.insert(
            "x-forwarded-for",
            HeaderValue::from_static("1.2.3.4, 10.0.0.7"),
        );
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        // From the proxy: the right-most address the proxy didn't add.
        assert_eq!(client_ip(&trusted, &h, ip("10.0.0.1")), ip("1.2.3.4"));
        // From anywhere else: the header is ignored (spoofable).
        assert_eq!(client_ip(&trusted, &h, ip("5.6.7.8")), ip("5.6.7.8"));
        // A spoofed left-most entry doesn't win over the real client.
        h.insert(
            "x-forwarded-for",
            HeaderValue::from_static("9.9.9.9, 1.2.3.4"),
        );
        assert_eq!(client_ip(&trusted, &h, ip("10.0.0.1")), ip("1.2.3.4"));
        assert_eq!(client_ip(&[], &h, ip("10.0.0.1")), ip("10.0.0.1"));
    }
}
