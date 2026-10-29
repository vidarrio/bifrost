//! Authentication and authorization.
//!
//! Every request passes through [`middleware`], which:
//!
//! 1. resolves *who* is asking into an optional [`Principal`], using the
//!    instance's single [`AuthMode`] — a native session cookie, or identity
//!    headers from a trusted forward-auth proxy;
//! 2. rejects cross-site state-changing requests (CSRF), by checking the
//!    browser's `Sec-Fetch-Site` / `Origin` headers;
//! 3. enforces the route tiers (public / logged in / admin)
//!    by path prefix, before any handler runs.
//!
//! Handlers read the result with the [`Me`] extractor.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use argon2::Argon2;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header, request::Parts};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use sha2::{Digest, Sha256};

use crate::db::Db;
use crate::net::Cidr;

// ---------------------------------------------------------------- identity

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    User,
    Moderator,
    Sysadmin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Moderator => "moderator",
            Role::Sysadmin => "sysadmin",
        }
    }
    /// Unknown values degrade to the least privilege.
    pub fn parse(s: &str) -> Role {
        Self::try_parse(s).unwrap_or(Role::User)
    }
    pub fn try_parse(s: &str) -> Option<Role> {
        match s {
            "user" => Some(Role::User),
            "moderator" => Some(Role::Moderator),
            "sysadmin" => Some(Role::Sysadmin),
            _ => None,
        }
    }
}

/// The authenticated person behind a request.
#[derive(Clone, Debug)]
pub struct Principal {
    pub account_id: i64,
    pub username: String,
    pub role: Role,
}

/// How this instance authenticates. Exactly one mode is active;
/// switched with the `bifrost auth mode` CLI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AuthMode {
    /// Bifrost's own username + password accounts.
    Native,
    /// Identity headers from a trusted forward-auth proxy (e.g. oauth2-proxy).
    ForwardAuth,
}

impl AuthMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthMode::Native => "native",
            AuthMode::ForwardAuth => "forward-auth",
        }
    }
    pub fn parse(s: &str) -> Option<AuthMode> {
        match s {
            "native" => Some(AuthMode::Native),
            "forward-auth" => Some(AuthMode::ForwardAuth),
            _ => None,
        }
    }
}

/// Things a principal may do. The single place authorization
/// is decided.
#[derive(Clone, Copy, Debug)]
pub enum Action<'a> {
    /// Start a new season (its creator then runs it).
    CreateSeason,
    /// Run a season — signups, apworlds, generate, console, archive, co-mods —
    /// given who runs it (its creator and co-moderators).
    ManageSeason { mods: &'a [i64] },
    /// Instance-level: users, links, auth, connect password.
    ManageInstance,
}

pub fn authorize(p: Option<&Principal>, action: Action) -> bool {
    let Some(p) = p else { return false };
    match action {
        Action::CreateSeason => p.role >= Role::Moderator,
        Action::ManageSeason { mods } => {
            p.role >= Role::Sysadmin || (p.role >= Role::Moderator && mods.contains(&p.account_id))
        }
        Action::ManageInstance => p.role >= Role::Sysadmin,
    }
}

/// Extractor for the request's principal (set by [`middleware`]).
#[derive(Clone, Debug, Default)]
pub struct Me(pub Option<Principal>);

impl Me {
    pub fn principal(&self) -> Option<&Principal> {
        self.0.as_ref()
    }
    /// `(username, is sysadmin)` for instance-level pages; season handlers use
    /// `SeasonCtx::ident` (may manage *that* season). The username is
    /// `"unknown"` when anonymous.
    pub fn ident(&self) -> (String, bool) {
        (self.username(), self.can(Action::ManageInstance))
    }
    pub fn username(&self) -> String {
        match &self.0 {
            Some(p) => p.username.clone(),
            None => "unknown".into(),
        }
    }
    /// The account id — what per-user rows are keyed by. `0` when anonymous
    /// (never a real account), so it matches and owns nothing.
    pub fn id(&self) -> i64 {
        self.0.as_ref().map(|p| p.account_id).unwrap_or(0)
    }
    pub fn can(&self, action: Action) -> bool {
        authorize(self.principal(), action)
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Me {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(parts.extensions.get::<Me>().cloned().unwrap_or_default())
    }
}

// ---------------------------------------------------------------- secrets

/// Hash a password with argon2id (OWASP-recommended parameters). CPU-heavy —
/// call via `spawn_blocking`.
pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

/// Constant-time password check against an argon2 PHC string. CPU-heavy.
pub fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| {
            Argon2::default()
                .verify_password(password.as_bytes(), &h)
                .is_ok()
        })
        .unwrap_or(false)
}

/// A valid argon2 hash of a random password: verified against when the
/// username doesn't exist, so a miss costs the same as a wrong password and
/// timing doesn't reveal which usernames exist.
pub fn dummy_hash() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| hash_password(&new_token()).expect("hash dummy password"))
}

/// A fresh 256-bit random token, hex-encoded (sessions, links, setup).
pub fn new_token() -> String {
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    hex(&b)
}

/// What the database stores for a token.
pub fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Username rules: 3–32 of `[A-Za-z0-9_.-]`.
pub fn valid_username(s: &str) -> bool {
    (3..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

pub const MIN_PASSWORD: usize = 10;
pub const MAX_PASSWORD: usize = 256;

pub fn password_problem(pw: &str) -> Option<String> {
    if pw.chars().count() < MIN_PASSWORD {
        Some(format!(
            "Password must be at least {MIN_PASSWORD} characters."
        ))
    } else if pw.len() > MAX_PASSWORD {
        Some(format!("Password must be at most {MAX_PASSWORD} bytes."))
    } else {
        None
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------- state

pub const SESSION_COOKIE: &str = "bifrost_session";
/// The session cookie's name over HTTPS: the `__Host-` prefix makes browsers
/// refuse it unless it was set by this host, `Secure`, for `/`, so a sibling
/// subdomain can't plant one.
pub const SECURE_SESSION_COOKIE: &str = "__Host-bifrost_session";
pub const SESSION_TTL: Duration = Duration::from_secs(30 * 24 * 3600);

/// Forward-auth settings (`BIFROST_FWD_*`). Identity headers are trusted only
/// from a configured proxy: a shared-secret header and/or a source-address
/// allowlist. With neither configured, forward-auth authenticates no one.
#[derive(Clone, Debug, Default)]
pub struct ForwardAuth {
    /// `(header name, expected value)` the proxy adds to every request.
    pub secret: Option<(String, String)>,
    /// Peer addresses / CIDRs the proxy connects from
    /// (`BIFROST_TRUSTED_PROXIES`, shared with rate limiting).
    pub trusted: Vec<Cidr>,
    pub subject_header: String,
    pub username_header: String,
    pub groups_header: String,
    /// IdP group → role. Non-empty = the IdP is authoritative for roles.
    pub role_groups: Vec<(Role, String)>,
    /// Where the header bar's sign-in / sign-out links point.
    pub sign_in_url: String,
    pub sign_out_url: String,
}

impl ForwardAuth {
    pub fn from_env(trusted: Vec<Cidr>) -> Result<Self, String> {
        use crate::config::var;
        let secret = match (var("BIFROST_FWD_SECRET_HEADER"), var("BIFROST_FWD_SECRET")) {
            (Some(h), Some(v)) => Some((h, v)),
            (None, None) => None,
            _ => return Err("set both BIFROST_FWD_SECRET_HEADER and BIFROST_FWD_SECRET".into()),
        };
        let role_groups = match var("BIFROST_FWD_ROLE_GROUPS") {
            Some(list) => parse_role_groups(&list)?,
            None => Vec::new(),
        };
        Ok(Self {
            secret,
            trusted,
            subject_header: var("BIFROST_FWD_SUBJECT_HEADER")
                .unwrap_or_else(|| "x-auth-request-user".into()),
            username_header: var("BIFROST_FWD_USERNAME_HEADER")
                .unwrap_or_else(|| "x-auth-request-preferred-username".into()),
            groups_header: var("BIFROST_FWD_GROUPS_HEADER")
                .unwrap_or_else(|| "x-auth-request-groups".into()),
            role_groups,
            sign_in_url: var("BIFROST_FWD_SIGN_IN_URL").unwrap_or_else(|| "/oauth2/sign_in".into()),
            sign_out_url: var("BIFROST_FWD_SIGN_OUT_URL")
                .unwrap_or_else(|| "/oauth2/sign_out".into()),
        })
    }

    pub fn configured(&self) -> bool {
        self.secret.is_some() || !self.trusted.is_empty()
    }

    /// Did this request come from the trusted proxy? Every configured check
    /// must pass.
    fn trusts(&self, headers: &HeaderMap, peer: Option<IpAddr>) -> bool {
        if !self.configured() {
            return false;
        }
        if let Some((name, want)) = &self.secret {
            let got = headers
                .get(name.as_str())
                .map(HeaderValue::as_bytes)
                .unwrap_or(b"");
            if !ct_eq(got, want.as_bytes()) {
                return false;
            }
        }
        if !self.trusted.is_empty() {
            let Some(ip) = peer else { return false };
            if !self.trusted.iter().any(|c| c.contains(ip)) {
                return false;
            }
        }
        true
    }

    /// The role the IdP grants, or `None` when roles are managed in the
    /// console. `Err` when a mapping exists and the user is in none of its
    /// groups — like the proxy's `allowed_groups`, they get no access.
    fn mapped_role(&self, groups: &str) -> Result<Option<Role>, ()> {
        if self.role_groups.is_empty() {
            return Ok(None);
        }
        let mine: Vec<&str> = groups.split(',').map(str::trim).collect();
        self.role_groups
            .iter()
            .filter(|(_, g)| mine.contains(&g.as_str()))
            .map(|(r, _)| *r)
            .max()
            .map(Some)
            .ok_or(())
    }
}

/// `sysadmin=archipelago_admins,moderator=archipelago_moderators,user=archipelago_users`.
fn parse_role_groups(list: &str) -> Result<Vec<(Role, String)>, String> {
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|pair| {
            let (r, g) = pair.split_once('=').ok_or_else(|| {
                format!("BIFROST_FWD_ROLE_GROUPS: expected role=group, got {pair:?}")
            })?;
            let role = Role::try_parse(r.trim())
                .ok_or_else(|| format!("BIFROST_FWD_ROLE_GROUPS: unknown role {r:?}"))?;
            Ok((role, g.trim().to_string()))
        })
        .collect()
}

pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Failed-login backoff, per username and client, plus a softer one per
/// username across all clients.
///
/// - Per client: after [`FREE_FAILURES`], each further failure doubles the
///   lockout (from 30 s, capped at 15 min). Keyed by client too, so one
///   client's failures can't lock an account for everyone. A client is an IPv4
///   address or an IPv6 /64 ([`crate::net::client_group`]).
/// - Per username: after [`NAME_FREE_FAILURES`] failures from any clients,
///   attempts on that username wait a little (doubling, capped at a minute).
///   That bounds guessing spread over many addresses. Browsers that have
///   signed in to the account before ([`AuthInner::known_device`]) skip this
///   wait, so nobody can keep the owner out by re-arming it.
///
/// An attempt counts as a failure until it succeeds ([`Throttle::attempt`]
/// before checking the password), so parallel attempts can't slip past the
/// backoff. While a map is full, keys without an entry share one overflow
/// entry, held to the per-username schedule (at most a minute) and skipped by
/// known browsers. Entries idle for an hour are forgotten; past [`THROTTLE_PRUNE_AT`]
/// entries they're pruned (at most once a minute), and past
/// [`THROTTLE_MAX`] the older half goes. In memory only — a restart forgives.
#[derive(Default)]
pub struct Throttle {
    state: Mutex<ThrottleState>,
}

#[derive(Default)]
struct ThrottleState {
    clients: HashMap<ThrottleKey, (u32, Instant)>,
    names: HashMap<String, (u32, Instant)>,
    pruned: Option<Instant>,
}

/// The entries an attempt was counted against (for [`Throttle::cancel`]).
#[derive(Debug)]
pub struct Attempt {
    ckey: ThrottleKey,
    nkey: String,
}

impl ThrottleState {
    /// The entries a key counts against: its own, or once a map is full and
    /// it has none there, the shared overflow entry.
    fn keys(&self, key: ThrottleKey) -> (ThrottleKey, String) {
        let name = if self.names.contains_key(&key.0) || self.names.len() < THROTTLE_MAX {
            key.0.clone()
        } else {
            OVERFLOW.to_string()
        };
        let client = if self.clients.contains_key(&key) || self.clients.len() < THROTTLE_MAX {
            key
        } else {
            (OVERFLOW.to_string(), None)
        };
        (client, name)
    }
}

/// (username, client group).
type ThrottleKey = (String, Option<IpAddr>);

const FREE_FAILURES: u32 = 5;
const NAME_FREE_FAILURES: u32 = 20;
/// How long a failure is remembered without further attempts.
const FORGET_AFTER: Duration = Duration::from_secs(3600);
/// Entries kept before idle ones are pruned, and the most kept at all.
const THROTTLE_PRUNE_AT: usize = 10_000;
const THROTTLE_MAX: usize = 50_000;

/// Keep at most `max` entries of `map`: those with the most failures, then
/// the most recent. A spray of one-off keys can't push out an account or a
/// client that's actually being guessed at.
fn keep_heaviest<K: std::hash::Hash + Eq>(map: &mut HashMap<K, (u32, Instant)>, max: usize) {
    if map.len() <= max {
        return;
    }
    let mut v: Vec<_> = map.drain().collect();
    v.sort_unstable_by_key(|e| std::cmp::Reverse(e.1));
    v.truncate(max);
    map.extend(v);
}

/// Where failures for keys that no longer fit are counted (no valid username
/// contains a NUL), so a full map still holds new keys back as one.
const OVERFLOW: &str = "\0overflow";

impl Throttle {
    fn key(username: &str, client: Option<IpAddr>) -> ThrottleKey {
        (
            username.to_lowercase(),
            client.map(crate::net::client_group),
        )
    }

    /// Start a sign-in attempt: `Err(seconds)` while `client` must wait, else
    /// the attempt is counted as a failure until [`succeed`](Self::succeed).
    /// `known` (a browser that signed in to this account before) skips the
    /// per-username wait.
    pub fn attempt(
        &self,
        username: &str,
        client: Option<IpAddr>,
        known: bool,
    ) -> Result<Attempt, u64> {
        let mut st = self.state.lock().unwrap();
        let now = Instant::now();
        let busy = st.clients.len().max(st.names.len()) > THROTTLE_PRUNE_AT;
        if busy
            && st
                .pruned
                .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(60))
        {
            st.pruned = Some(now);
            st.clients
                .retain(|_, e| now.duration_since(e.1) < FORGET_AFTER);
            st.names
                .retain(|_, e| now.duration_since(e.1) < FORGET_AFTER);
            keep_heaviest(&mut st.clients, THROTTLE_MAX / 2);
            keep_heaviest(&mut st.names, THROTTLE_MAX / 2);
        }
        let (ckey, nkey) = st.keys(Self::key(username, client));
        let wait = |e: Option<&(u32, Instant)>, lockout: fn(u32) -> Duration| match e {
            Some(&(n, last)) if now.duration_since(last) < FORGET_AFTER => {
                lockout(n).saturating_sub(now.duration_since(last))
            }
            _ => Duration::ZERO,
        };
        // The shared overflow entries are held to the gentle per-username
        // schedule, and a known browser skips them, so keeping the maps full
        // can't lock the owner out.
        let overflow = ckey.0 == OVERFLOW;
        let mut w = if overflow && known {
            Duration::ZERO
        } else if overflow {
            wait(st.clients.get(&ckey), Self::name_lockout)
        } else {
            wait(st.clients.get(&ckey), Self::lockout)
        };
        if !known {
            w = w.max(wait(st.names.get(&nkey), Self::name_lockout));
        }
        if !w.is_zero() {
            return Err(w.as_secs().max(1));
        }
        Self::bump(st.clients.entry(ckey.clone()).or_insert((0, now)), now);
        Self::bump(st.names.entry(nkey.clone()).or_insert((0, now)), now);
        Ok(Attempt { ckey, nkey })
    }

    /// Take back an attempt that never got to check the password (the
    /// server was too busy).
    pub fn cancel(&self, attempt: &Attempt) {
        let mut st = self.state.lock().unwrap();
        if let Some(e) = st.clients.get_mut(&attempt.ckey) {
            e.0 = e.0.saturating_sub(1);
        }
        if let Some(e) = st.names.get_mut(&attempt.nkey) {
            e.0 = e.0.saturating_sub(1);
        }
    }

    fn bump(e: &mut (u32, Instant), now: Instant) {
        if now.duration_since(e.1) >= FORGET_AFTER {
            e.0 = 0;
        }
        e.0 += 1;
        e.1 = now;
    }

    /// A successful sign-in: forgive that client, and the username's count.
    pub fn succeed(&self, username: &str, client: Option<IpAddr>) {
        let key = Self::key(username, client);
        let mut st = self.state.lock().unwrap();
        st.names.remove(&key.0);
        st.clients.remove(&key);
    }

    /// A password reset: forgive every client.
    pub fn forgive(&self, username: &str) {
        let u = username.to_lowercase();
        let mut st = self.state.lock().unwrap();
        st.clients.retain(|(name, _), _| *name != u);
        st.names.remove(&u);
    }

    fn lockout(failures: u32) -> Duration {
        if failures < FREE_FAILURES {
            return Duration::ZERO;
        }
        let secs = 30u64.saturating_mul(1 << (failures - FREE_FAILURES).min(10));
        Duration::from_secs(secs.min(15 * 60))
    }

    fn name_lockout(failures: u32) -> Duration {
        if failures < NAME_FREE_FAILURES {
            return Duration::ZERO;
        }
        let secs = 1u64 << (failures - NAME_FREE_FAILURES).min(6);
        Duration::from_secs(secs.min(60))
    }
}

/// Password hashing (argon2, 19 MiB each) runs a few at a time, so a burst of
/// sign-ins, invites or resets can't take the host's memory. The permit
/// travels with the blocking job, so it's held until argon2 is done even if
/// the request goes away; and only so many may queue for one.
static HASHING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
static HASHING_QUEUE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
const HASHING_QUEUE_MAX: usize = 64;

pub const BUSY: &str = "The server is busy — try again in a moment.";

/// Run `f` on the blocking pool under a hashing permit; `None` when too many
/// are already waiting.
async fn hashing<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    use std::sync::atomic::Ordering;
    /// A place in the queue, given back however the wait ends (also when
    /// the request is dropped while waiting).
    struct Queued;
    impl Drop for Queued {
        fn drop(&mut self) {
            HASHING_QUEUE.fetch_sub(1, Ordering::SeqCst);
        }
    }
    if HASHING_QUEUE.fetch_add(1, Ordering::SeqCst) >= HASHING_QUEUE_MAX {
        HASHING_QUEUE.fetch_sub(1, Ordering::SeqCst);
        return None;
    }
    let queued = Queued;
    let permit = HASHING.acquire().await;
    drop(queued);
    let permit = permit.ok()?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        f()
    })
    .await
    .ok()
}

/// Hash a password off the async runtime, a few at a time.
pub async fn hash_password_async(password: String) -> Result<String, String> {
    hashing(move || hash_password(&password))
        .await
        .unwrap_or_else(|| Err(BUSY.into()))
}

/// Verify a password off the async runtime, a few at a time (against the
/// dummy hash when `hash` is `None`, so a missing user takes as long).
/// `None` when the server is too busy to try.
pub async fn verify_password_async(hash: Option<String>, password: String) -> Option<bool> {
    hashing(move || verify_password(hash.as_deref().unwrap_or_else(|| dummy_hash()), &password))
        .await
}

/// The known-device MAC for an account.
fn device_mac(key: &str, username: &str, password_hash: &str) -> [u8; 32] {
    let msg = format!("device:{}:{password_hash}", username.to_lowercase());
    hmac_sha256(key.as_bytes(), msg.as_bytes())
}

/// HMAC-SHA256 (RFC 2104).
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |b: u8| k.map(|x| x ^ b);
    let inner = Sha256::new()
        .chain_update(pad(0x36))
        .chain_update(msg)
        .finalize();
    Sha256::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

/// Remembers that this browser signed in to an account: exempts it from the
/// per-username sign-in wait ([`Throttle`]).
pub const DEVICE_COOKIE: &str = "bifrost_device";
const DEVICE_TTL: Duration = Duration::from_secs(365 * 24 * 3600);

/// Everything the middleware and the account handlers share.
pub struct AuthInner {
    pub db: Db,
    pub mode: AuthMode,
    pub fwd: ForwardAuth,
    /// Mark the session cookie `Secure` (`BIFROST_INSECURE_COOKIES=1` turns it
    /// off for plain-HTTP development).
    pub secure_cookies: bool,
    pub throttle: Throttle,
    /// Key for the known-device cookie ([`DEVICE_COOKIE`]), kept in the
    /// database so it survives restarts.
    pub device_key: String,
    /// First-run setup token while no account exists (native mode).
    pub setup_token: Mutex<Option<String>>,
}

pub type Auth = Arc<AuthInner>;

impl AuthInner {
    /// The session cookie's name: `__Host-` prefixed unless cookies are
    /// insecure (plain-HTTP development, where browsers refuse the prefix).
    pub fn session_cookie_name(&self) -> &'static str {
        if self.secure_cookies {
            SECURE_SESSION_COOKIE
        } else {
            SESSION_COOKIE
        }
    }

    /// `Set-Cookie` value for a new session (or, with `None`, clearing it).
    pub fn session_cookie(&self, token: Option<&str>) -> HeaderValue {
        HeaderValue::from_str(&session_cookie_value(self.secure_cookies, token))
            .expect("cookie is ascii")
    }

    /// The session token from the request's cookies, if any (only under this
    /// instance's cookie name: over HTTPS, an unprefixed one is ignored).
    pub fn session_token(&self, headers: &HeaderMap) -> Option<String> {
        cookie(headers, self.session_cookie_name())
    }

    /// Bound to the account's current password hash too, so a password
    /// reset retires every browser's tag.
    fn device_tag(&self, username: &str, password_hash: &str) -> String {
        hex(&device_mac(&self.device_key, username, password_hash))
    }

    /// `Set-Cookie` value marking this browser as one that signed in as
    /// `username` (with the password whose hash is `password_hash`). Sent only
    /// to `/login`, the one page that reads it.
    pub fn device_cookie(&self, username: &str, password_hash: &str) -> HeaderValue {
        let secure = if self.secure_cookies { "; Secure" } else { "" };
        let v = format!(
            "{DEVICE_COOKIE}={}; Path=/login; HttpOnly; SameSite=Lax; Max-Age={}{secure}",
            self.device_tag(username, password_hash),
            DEVICE_TTL.as_secs()
        );
        HeaderValue::from_str(&v).expect("cookie is ascii")
    }

    /// Has this browser signed in as `username`, with its current password?
    pub fn known_device(&self, headers: &HeaderMap, username: &str, password_hash: &str) -> bool {
        cookie(headers, DEVICE_COOKIE).is_some_and(|t| {
            ct_eq(
                t.as_bytes(),
                self.device_tag(username, password_hash).as_bytes(),
            )
        })
    }

    /// Where "sign in" / "sign out" point for this mode (the header bar).
    pub fn sign_in_url(&self) -> &str {
        match self.mode {
            AuthMode::Native => "/login",
            AuthMode::ForwardAuth => &self.fwd.sign_in_url,
        }
    }
    pub fn sign_out_url(&self) -> &str {
        match self.mode {
            AuthMode::Native => "/logout",
            AuthMode::ForwardAuth => &self.fwd.sign_out_url,
        }
    }

    /// Start a session for `account_id`; returns the cookie to set.
    pub async fn start_session(&self, account_id: i64) -> Result<HeaderValue, String> {
        let token = new_token();
        let now = now();
        self.db
            .create_session(
                &token_hash(&token),
                account_id,
                now,
                now + SESSION_TTL.as_secs() as i64,
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(self.session_cookie(Some(&token)))
    }
}

/// The session `Set-Cookie` value: `__Host-` named and `Secure` over HTTPS,
/// always `Path=/` with no `Domain` (as the prefix requires).
fn session_cookie_value(secure: bool, token: Option<&str>) -> String {
    let (name, flag) = if secure {
        (SECURE_SESSION_COOKIE, "; Secure")
    } else {
        (SESSION_COOKIE, "")
    };
    let (value, age) = match token {
        Some(t) => (t, SESSION_TTL.as_secs()),
        None => ("", 0),
    };
    format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={age}{flag}")
}

/// A cookie's value from the request, if set and non-empty.
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
        .filter(|v| !v.is_empty())
}

// ---------------------------------------------------------------- tiers

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tier {
    Public,
    LoggedIn,
    /// Season administration: moderators and up (handlers then check that the
    /// moderator runs *this* season).
    Admin,
    /// Instance administration: sysadmins only.
    Sysadmin,
}

/// Route tiers: plain path-prefix match (as the proxy's
/// `PathPrefix` did), most specific first, public by default.
const SYSADMIN_PREFIXES: &[&str] = &[
    "/admin/users",
    "/admin/links",
    "/admin/settings",
    "/admin/catalog",
];
const ADMIN_PREFIXES: &[&str] = &["/admin"];
const LOGGED_IN_PREFIXES: &[&str] = &[
    "/claim",
    "/hint",
    "/signup",
    "/game",
    "/prioritize",
    "/minesweeper",
    "/api",
    "/mc",
];

pub fn tier(path: &str) -> Tier {
    if SYSADMIN_PREFIXES.iter().any(|p| path.starts_with(p)) {
        Tier::Sysadmin
    } else if ADMIN_PREFIXES.iter().any(|p| path.starts_with(p)) {
        Tier::Admin
    } else if LOGGED_IN_PREFIXES.iter().any(|p| path.starts_with(p)) {
        Tier::LoggedIn
    } else {
        Tier::Public
    }
}

// ---------------------------------------------------------------- csrf

/// Is this state-changing request plausibly from our own pages? Browsers send
/// `Sec-Fetch-Site` (modern) or at least `Origin` on cross-site POSTs; a
/// request with neither is not from a browser and can't be a CSRF.
fn same_origin(headers: &HeaderMap) -> bool {
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        return matches!(site, "same-origin" | "none");
    }
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let authority = origin.split_once("://").map(|(_, a)| a).unwrap_or("");
    !host.is_empty() && authority.eq_ignore_ascii_case(host)
}

// ---------------------------------------------------------------- middleware

pub async fn middleware(State(auth): State<Auth>, mut req: Request, next: Next) -> Response {
    let state_changing = !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if state_changing && !same_origin(req.headers()) {
        return (StatusCode::FORBIDDEN, "Cross-site request refused.").into_response();
    }

    let path = req.uri().path().to_string();
    let tier = tier(&path);
    let principal = match auth.mode {
        AuthMode::Native => match auth.session_token(req.headers()) {
            Some(t) => auth.db.session_principal(&token_hash(&t), now()).await,
            None => None,
        },
        // Identity headers are only meaningful where the proxy authenticated
        // the request (and overwrote them) — on public routes a client could
        // supply its own.
        AuthMode::ForwardAuth if tier != Tier::Public => {
            let peer = req
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0.ip());
            match forward_auth_principal(&auth, req.headers(), peer).await {
                Ok(p) => p,
                Err(()) => return (StatusCode::FORBIDDEN, "Not authorized.").into_response(),
            }
        }
        AuthMode::ForwardAuth => None,
    };

    match (tier, &principal) {
        (Tier::Public, _) => {}
        (_, None) if req.uri().path().starts_with("/api") => {
            return api_unauthenticated(&auth).await;
        }
        (_, None) => return unauthenticated(&auth, &req),
        (Tier::Admin, Some(p)) if p.role < Role::Moderator => {
            return (StatusCode::FORBIDDEN, "Moderators only.").into_response();
        }
        (Tier::Sysadmin, Some(p)) if p.role < Role::Sysadmin => {
            return (StatusCode::FORBIDDEN, "Sysadmins only.").into_response();
        }
        _ => {}
    }

    req.extensions_mut().insert(Me(principal));
    next.run(req).await
}

async fn forward_auth_principal(
    auth: &AuthInner,
    headers: &HeaderMap,
    peer: Option<IpAddr>,
) -> Result<Option<Principal>, ()> {
    let fwd = &auth.fwd;
    if !fwd.trusts(headers, peer) {
        tracing::warn!(
            "forward-auth: request from an untrusted source ({peer:?}); ignoring identity"
        );
        return Ok(None);
    }
    let get = |h: &str| {
        headers
            .get(h)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let Some(subject) = get(&fwd.subject_header) else {
        return Ok(None);
    };
    let username = get(&fwd.username_header).unwrap_or(subject);
    let role = fwd.mapped_role(get(&fwd.groups_header).unwrap_or(""))?;
    match auth.db.idp_principal(subject, username, role, now()).await {
        Ok(Some(p)) => Ok(Some(p)),
        Ok(None) => Err(()), // disabled in the console
        Err(e) => {
            tracing::warn!("forward-auth: provisioning {username}: {e}");
            Ok(None)
        }
    }
}

/// An anonymous request to a gated API: 401 with JSON, so client code (e.g.
/// the header's `/api/me`) can offer sign-in and still show the home link.
async fn api_unauthenticated(auth: &AuthInner) -> Response {
    let home = auth.db.instance().await.home_url;
    let body = serde_json::json!({ "user": null, "admin": false, "sign_in": auth.sign_in_url(), "home": home });
    (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response()
}

/// An anonymous request to a gated page: sign-in (APIs: `api_unauthenticated`).
fn unauthenticated(auth: &AuthInner, req: &Request) -> Response {
    if req.method() != Method::GET {
        return (StatusCode::UNAUTHORIZED, "Sign in required.").into_response();
    }
    match auth.mode {
        AuthMode::Native => {
            // As requested, before any season-prefix rewrite.
            let uri = req
                .extensions()
                .get::<axum::extract::OriginalUri>()
                .map(|o| o.0.clone())
                .unwrap_or_else(|| req.uri().clone());
            let rd = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
            Redirect::to(&format!("/login?rd={}", pct(rd))).into_response()
        }
        // The proxy should have intercepted this; reaching us means it isn't
        // in front of this route (or isn't trusted).
        AuthMode::ForwardAuth => (StatusCode::UNAUTHORIZED, "Sign in required.").into_response(),
    }
}

/// Percent-encode a query value.
pub fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A post-login redirect target, only if it's a local path (no open redirect).
/// Visible ASCII only: browsers drop tabs/newlines from a Location (so
/// `/\t/evil` would become `//evil`), and non-ASCII can't be a header value.
pub fn safe_redirect(rd: Option<&str>) -> String {
    match rd {
        Some(r)
            if r.starts_with('/')
                && !r.starts_with("//")
                && !r.contains('\\')
                && r.bytes().all(|b| b.is_ascii_graphic()) =>
        {
            r.to_string()
        }
        _ => "/".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_proxy_rule_covers_every_signed_in_prefix() {
        // The user guide's forward-auth proxy rule must gate everything that
        // isn't public, or a proxy set up from it lets identity headers through.
        let guide = include_str!("../docs/USER_GUIDE.md");
        let rule = guide
            .lines()
            .find(|l| l.starts_with("PathRegexp(`^/s/"))
            .expect("the guide's season rule");
        let alts = rule
            .split_once('(')
            .and_then(|(_, r)| r.rsplit_once('(').map(|(_, a)| a))
            .and_then(|a| a.split_once(')').map(|(a, _)| a))
            .expect("an alternation in the rule");
        let alts: Vec<&str> = alts.split('|').collect();
        for p in SYSADMIN_PREFIXES
            .iter()
            .chain(ADMIN_PREFIXES)
            .chain(LOGGED_IN_PREFIXES)
        {
            let first = p.trim_start_matches('/').split('/').next().unwrap();
            assert!(
                alts.contains(&first),
                "{p} isn't in the guide's rule: {rule}"
            );
            assert!(
                guide.contains(&format!("`/{first}`")),
                "{p} isn't in the guide's prefix list"
            );
        }
    }

    #[test]
    fn tiers_match_design_14() {
        assert_eq!(tier("/admin"), Tier::Admin);
        assert_eq!(tier("/admin/seasons"), Tier::Admin);
        assert_eq!(tier("/admin/users"), Tier::Sysadmin);
        assert_eq!(tier("/admin/links/invite"), Tier::Sysadmin);
        assert_eq!(tier("/admin/settings"), Tier::Sysadmin);
        assert_eq!(tier("/admin/catalog"), Tier::Sysadmin);
        assert_eq!(tier("/admin/worlds/approve"), Tier::Admin);
        for p in [
            "/claim",
            "/hint",
            "/signup/check",
            "/game/3",
            "/game/3/poptracker/asset",
            "/prioritize",
            "/minesweeper/new",
            "/api/me",
            "/mc/username",
        ] {
            assert_eq!(tier(p), Tier::LoggedIn, "{p}");
        }
        for p in [
            "/",
            "/board",
            "/board/fragment",
            "/check",
            "/log",
            "/season/1",
            "/health",
            "/favicon.svg",
            "/login",
            "/logout",
            "/setup",
            "/invite/x",
            "/reset/x",
        ] {
            assert_eq!(tier(p), Tier::Public, "{p}");
        }
        // Plain prefix semantics, like the proxy's PathPrefix: gate more, never less.
        assert_eq!(tier("/gameboy"), Tier::LoggedIn);
    }

    #[test]
    fn season_management_is_owners_and_sysadmins() {
        let p = |id, role| Principal {
            account_id: id,
            username: String::new(),
            role,
        };
        let mods = [7, 9];
        let manage = Action::ManageSeason { mods: &mods };
        assert!(
            authorize(Some(&p(1, Role::Sysadmin)), manage),
            "sysadmin: any season"
        );
        assert!(authorize(Some(&p(9, Role::Moderator)), manage), "co-mod");
        assert!(
            !authorize(Some(&p(2, Role::Moderator)), manage),
            "someone else's season"
        );
        assert!(!authorize(Some(&p(7, Role::User)), manage), "demoted owner");
        assert!(authorize(
            Some(&p(2, Role::Moderator)),
            Action::CreateSeason
        ));
        assert!(!authorize(Some(&p(2, Role::User)), Action::CreateSeason));
        assert!(!authorize(
            Some(&p(2, Role::Moderator)),
            Action::ManageInstance
        ));
        assert!(!authorize(None, Action::CreateSeason));
    }

    #[test]
    fn passwords_round_trip() {
        let h = hash_password("correct horse battery").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password(&h, "correct horse battery"));
        assert!(!verify_password(&h, "wrong"));
        assert!(!verify_password("garbage", "x"));
        assert!(!verify_password(dummy_hash(), ""));
    }

    #[test]
    fn tokens_are_random_and_hashed() {
        let (a, b) = (new_token(), new_token());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert_eq!(token_hash(&a), token_hash(&a));
        assert_ne!(token_hash(&a), a);
    }

    #[test]
    fn username_and_password_rules() {
        assert!(valid_username("ann_b.c-1"));
        assert!(!valid_username("an"));
        assert!(!valid_username("ann b"));
        assert!(!valid_username("<script>"));
        assert!(password_problem("short").is_some());
        assert!(password_problem("long enough pw").is_none());
    }

    fn hm(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn csrf_origin_check() {
        assert!(same_origin(&hm(&[("sec-fetch-site", "same-origin")])));
        assert!(!same_origin(&hm(&[("sec-fetch-site", "cross-site")])));
        assert!(!same_origin(&hm(&[("sec-fetch-site", "same-site")])));
        assert!(same_origin(&hm(&[
            ("origin", "https://ap.example"),
            ("host", "ap.example")
        ])));
        assert!(!same_origin(&hm(&[
            ("origin", "https://evil.example"),
            ("host", "ap.example")
        ])));
        assert!(!same_origin(&hm(&[
            ("origin", "null"),
            ("host", "ap.example")
        ])));
        assert!(same_origin(&hm(&[])), "non-browser client");
    }

    #[test]
    fn session_cookie_is_read() {
        let h = hm(&[("cookie", "a=1; bifrost_session=abc; b=2")]);
        assert_eq!(cookie(&h, SESSION_COOKIE).as_deref(), Some("abc"));
        assert_eq!(
            cookie(&hm(&[("cookie", "bifrost_session=")]), SESSION_COOKIE),
            None
        );
        // Over HTTPS only the __Host- cookie counts.
        assert_eq!(cookie(&h, SECURE_SESSION_COOKIE), None);
        // …and it's set the way the prefix requires.
        let v = session_cookie_value(true, Some("t"));
        assert!(v.starts_with("__Host-bifrost_session=t; Path=/;"), "{v}");
        assert!(v.ends_with("; Secure") && !v.contains("Domain"), "{v}");
        let v = session_cookie_value(false, None);
        assert!(
            v.starts_with("bifrost_session=; Path=/;") && v.contains("Max-Age=0"),
            "{v}"
        );
        assert!(!v.contains("Secure"), "{v}");
        let h = hm(&[("cookie", "bifrost_session=x; __Host-bifrost_session=y")]);
        assert_eq!(cookie(&h, SECURE_SESSION_COOKIE).as_deref(), Some("y"));
    }

    #[test]
    fn redirects_stay_local() {
        assert_eq!(safe_redirect(Some("/game/3?x=1")), "/game/3?x=1");
        assert_eq!(safe_redirect(Some("//evil.example")), "/");
        assert_eq!(safe_redirect(Some("https://evil.example")), "/");
        assert_eq!(safe_redirect(Some("/\\evil.example")), "/");
        assert_eq!(safe_redirect(None), "/");
        assert_eq!(safe_redirect(Some("/\t/evil.example")), "/");
        assert_eq!(safe_redirect(Some("/\n/evil.example")), "/");
        assert_eq!(safe_redirect(Some("/ /evil.example")), "/");
        assert_eq!(safe_redirect(Some("/caf\u{e9}")), "/");
    }

    #[test]
    fn throttle_backs_off_then_forgives() {
        let t = Throttle::default();
        let ip = |s: &str| Some(s.parse().unwrap());
        let (me, them) = (ip("10.0.0.1"), ip("10.0.0.2"));
        // Each attempt counts until it succeeds.
        for _ in 0..FREE_FAILURES {
            assert!(t.attempt("Ann", them, false).is_ok());
        }
        assert!(t.attempt("ANN", them, false).is_err());
        // Another client's failures don't lock the account for me.
        assert!(t.attempt("ann", me, false).is_ok());
        t.succeed("ann", me);
        assert!(
            t.attempt("ann", them, false).is_err(),
            "my sign-in doesn't forgive them"
        );
        t.forgive("ANN");
        assert!(
            t.attempt("ann", them, false).is_ok(),
            "a reset forgives everyone"
        );
        assert_eq!(Throttle::lockout(100), Duration::from_secs(15 * 60));
    }

    #[test]
    fn throttle_counts_an_ipv6_64_as_one_client() {
        let t = Throttle::default();
        for i in 0..FREE_FAILURES {
            let addr = format!("2001:db8:1:2::{}", i + 1);
            assert!(t.attempt("ann", Some(addr.parse().unwrap()), false).is_ok());
        }
        assert!(
            t.attempt("ann", Some("2001:db8:1:2::ff".parse().unwrap()), false)
                .is_err()
        );
    }

    #[test]
    fn throttle_slows_guessing_spread_over_many_clients() {
        let t = Throttle::default();
        for i in 0..NAME_FREE_FAILURES {
            let addr = format!("10.1.{}.{}", i / 250, i % 250 + 1);
            assert!(t.attempt("ann", Some(addr.parse().unwrap()), false).is_ok());
        }
        // A fresh client now waits briefly; never longer than a minute.
        let fresh = Some("10.9.9.9".parse().unwrap());
        let wait = t.attempt("ann", fresh, false).unwrap_err();
        assert!((1..=60).contains(&wait), "{wait}");
        assert_eq!(Throttle::name_lockout(1000), Duration::from_secs(60));
        // A browser that signed in to the account before isn't held up.
        assert!(t.attempt("ann", fresh, true).is_ok());
        // The owner's sign-in clears the username's count.
        t.succeed("ann", fresh);
        assert!(t.attempt("ann", fresh, false).is_ok());
    }

    #[test]
    fn throttle_memory_stays_bounded() {
        let t = Throttle::default();
        for i in 0..(THROTTLE_MAX as u32 + 5_000) {
            let ip = std::net::Ipv4Addr::from(0x0a00_0000 + i);
            let _ = t.attempt(&format!("u{i}"), Some(ip.into()), false);
        }
        let st = t.state.lock().unwrap();
        // (Plus the one shared overflow entry each.)
        assert!(st.clients.len() <= THROTTLE_MAX + 1 && st.names.len() <= THROTTLE_MAX + 1);
    }

    #[test]
    fn spraying_names_doesnt_evict_an_account_under_attack() {
        let t = Throttle::default();
        for i in 0..NAME_FREE_FAILURES {
            let ip = std::net::Ipv4Addr::from(0x0a01_0000 + i);
            assert!(t.attempt("ann", Some(ip.into()), false).is_ok());
        }
        for i in 0..(THROTTLE_MAX as u32 + 5_000) {
            let ip = std::net::Ipv4Addr::from(0x0b00_0000 + i);
            let _ = t.attempt(&format!("u{i}"), Some(ip.into()), false);
        }
        // Still counted: a new browser still waits.
        let fresh = Some("10.200.0.1".parse().unwrap());
        assert!(t.attempt("ann", fresh, false).is_err());
    }

    #[test]
    fn a_full_map_cant_lock_the_owner_out() {
        let t = Throttle::default();
        for i in 0..(THROTTLE_MAX as u32 + 50) {
            let ip = std::net::Ipv4Addr::from(0x0c00_0000 + i);
            let _ = t.attempt(&format!("u{i}"), Some(ip.into()), false);
        }
        let fresh = Some("10.250.0.1".parse().unwrap());
        // A new browser waits at most a minute; a known one not at all.
        if let Err(w) = t.attempt("ann", fresh, false) {
            assert!(w <= 60, "{w}");
        }
        for _ in 0..50 {
            let _ = t.attempt("x", Some("10.251.0.1".parse().unwrap()), false);
        }
        assert!(t.attempt("ann", fresh, true).is_ok());
    }

    #[test]
    fn a_cancelled_attempt_isnt_counted() {
        let t = Throttle::default();
        let me = Some("10.0.0.1".parse().unwrap());
        for _ in 0..FREE_FAILURES + 3 {
            let a = t.attempt("ann", me, false).unwrap();
            t.cancel(&a);
        }
        assert!(t.attempt("ann", me, false).is_ok());
    }

    #[test]
    fn device_cookie_is_per_account_and_keyed() {
        let tag = |key, user, pw| device_mac(key, user, pw);
        assert_eq!(tag("k", "Ann", "h"), tag("k", "ann", "h"));
        assert_ne!(tag("k", "ann", "h"), tag("k", "bob", "h"));
        assert_ne!(tag("k", "ann", "h"), tag("other", "ann", "h"));
        assert_ne!(
            tag("k", "ann", "h"),
            tag("k", "ann", "h2"),
            "a reset retires it"
        );
        // RFC 4231 test case 2.
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn proxy_trust() {
        let mut f = ForwardAuth::default();
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        assert!(
            !f.trusts(&hm(&[]), ip("10.0.0.1")),
            "unconfigured trusts no one"
        );
        f.trusted = vec![Cidr::parse("10.0.0.0/24").unwrap()];
        assert!(f.trusts(&hm(&[]), ip("10.0.0.86")));
        assert!(f.trusts(&hm(&[]), ip("::ffff:10.0.0.86")));
        assert!(!f.trusts(&hm(&[]), ip("10.0.1.1")));
        assert!(!f.trusts(&hm(&[]), None));
        f.secret = Some(("x-proxy-secret".into(), "s3cret".into()));
        assert!(
            !f.trusts(&hm(&[]), ip("10.0.0.86")),
            "both checks must pass"
        );
        assert!(f.trusts(&hm(&[("x-proxy-secret", "s3cret")]), ip("10.0.0.86")));
    }

    #[test]
    fn role_mapping() {
        let mut f = ForwardAuth::default();
        assert_eq!(
            f.mapped_role("anything"),
            Ok(None),
            "no mapping: console roles"
        );
        f.role_groups = parse_role_groups("sysadmin=admins@idp, user=users@idp").unwrap();
        assert_eq!(f.mapped_role("users@idp"), Ok(Some(Role::User)));
        assert_eq!(
            f.mapped_role("users@idp,admins@idp"),
            Ok(Some(Role::Sysadmin))
        );
        assert_eq!(f.mapped_role("others@idp"), Err(()));
        assert!(parse_role_groups("boss=x").is_err());
    }
}
