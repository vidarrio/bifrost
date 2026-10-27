//! Persistence for native auth: accounts, login sessions, and the
//! sysadmin-minted links (password resets, single-use invites, multi-use
//! community invites). Tokens are never stored — only their SHA-256 — so a
//! leaked database can't be replayed as sessions or links.

use sqlx::{FromRow, SqlitePool};

use super::Db;
use crate::auth::{AuthMode, Principal, Role};

pub(super) async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    // One row per person. `password_hash` is set for native accounts;
    // `idp_subject` (the IdP's stable id) for IdP-provisioned ones.
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS account (
             id INTEGER PRIMARY KEY,
             username TEXT NOT NULL UNIQUE COLLATE NOCASE,
             password_hash TEXT,
             idp_subject TEXT UNIQUE,
             role TEXT NOT NULL DEFAULT 'user',
             disabled INTEGER NOT NULL DEFAULT 0,
             created_at INTEGER NOT NULL
           )"#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS session (
             token_hash TEXT PRIMARY KEY,
             account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
             created_at INTEGER NOT NULL,
             expires_at INTEGER NOT NULL
           )"#,
    )
    .execute(pool)
    .await?;
    // kind: 'reset' (account_id set, single use), 'invite' (single use),
    // 'community' (max_uses NULL = unlimited until expiry/revocation).
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS auth_link (
             id INTEGER PRIMARY KEY,
             token_hash TEXT NOT NULL UNIQUE,
             kind TEXT NOT NULL CHECK (kind IN ('reset', 'invite', 'community')),
             account_id INTEGER REFERENCES account(id) ON DELETE CASCADE,
             label TEXT NOT NULL DEFAULT '',
             max_uses INTEGER,
             uses INTEGER NOT NULL DEFAULT 0,
             expires_at INTEGER NOT NULL,
             revoked INTEGER NOT NULL DEFAULT 0,
             created_by INTEGER,
             created_at INTEGER NOT NULL
           )"#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, FromRow, Clone)]
pub struct AccountRow {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub disabled: bool,
    /// Native accounts have a password; IdP-provisioned ones don't.
    pub has_password: bool,
    pub idp: bool,
}

#[derive(Debug, FromRow, Clone)]
pub struct LinkRow {
    pub id: i64,
    pub kind: String,
    pub label: String,
    /// The account a reset link is for.
    pub username: Option<String>,
    pub max_uses: Option<i64>,
    pub uses: i64,
    pub expires_at: i64,
    pub revoked: bool,
}

impl LinkRow {
    /// Still redeemable at `now`?
    pub fn live(&self, now: i64) -> bool {
        !self.revoked && self.expires_at > now && self.max_uses.is_none_or(|m| self.uses < m)
    }
}

/// Why redeeming an invite failed.
#[derive(Debug, PartialEq)]
pub enum RedeemError {
    /// Unknown, expired, revoked or used up.
    BadLink,
    UsernameTaken,
    Db(String),
}

/// The link kinds the console can mint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LinkKind {
    Reset,
    Invite,
    Community,
}

impl LinkKind {
    fn as_str(self) -> &'static str {
        match self {
            LinkKind::Reset => "reset",
            LinkKind::Invite => "invite",
            LinkKind::Community => "community",
        }
    }
}

#[derive(FromRow)]
struct PrincipalRow {
    id: i64,
    username: String,
    role: String,
}

impl From<PrincipalRow> for Principal {
    fn from(r: PrincipalRow) -> Self {
        Principal {
            account_id: r.id,
            username: r.username,
            role: Role::parse(&r.role),
        }
    }
}

const ACCOUNT_COLS: &str = "id, username, role, disabled,
     password_hash IS NOT NULL AS has_password, idp_subject IS NOT NULL AS idp";

impl Db {
    pub async fn auth_mode(&self) -> AuthMode {
        match self.get_setting("auth_mode").await {
            Ok(Some(v)) => AuthMode::parse(&v).unwrap_or(AuthMode::Native),
            _ => AuthMode::Native,
        }
    }

    pub async fn set_auth_mode(&self, mode: AuthMode) -> Result<(), sqlx::Error> {
        self.set_setting("auth_mode", mode.as_str()).await
    }

    pub async fn account_count(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM account")
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0)
    }

    pub async fn accounts(&self) -> Vec<AccountRow> {
        sqlx::query_as(&format!(
            "SELECT {ACCOUNT_COLS} FROM account ORDER BY username COLLATE NOCASE"
        ))
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    pub async fn account_by_name(&self, username: &str) -> Option<AccountRow> {
        sqlx::query_as(&format!(
            "SELECT {ACCOUNT_COLS} FROM account WHERE username = ?"
        ))
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    pub async fn username(&self, id: i64) -> Option<String> {
        sqlx::query_scalar("SELECT username FROM account WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    /// Create a native account. Errors if the username is taken.
    pub async fn create_account(
        &self,
        username: &str,
        password_hash: &str,
        role: Role,
        now: i64,
    ) -> Result<i64, sqlx::Error> {
        let r = sqlx::query(
            "INSERT INTO account (username, password_hash, role, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(username)
        .bind(password_hash)
        .bind(role.as_str())
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(r.last_insert_rowid())
    }

    /// The account's password hash, for verifying a login (enabled native
    /// accounts only).
    pub async fn login_lookup(&self, username: &str) -> Option<(i64, String)> {
        sqlx::query_as(
            "SELECT id, password_hash FROM account
             WHERE username = ? AND disabled = 0 AND password_hash IS NOT NULL",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    pub async fn set_role(&self, account_id: i64, role: Role) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE account SET role = ? WHERE id = ?")
            .bind(role.as_str())
            .bind(account_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Disable/enable an account. Disabling also ends its sessions.
    pub async fn set_disabled(&self, account_id: i64, disabled: bool) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE account SET disabled = ? WHERE id = ?")
            .bind(disabled)
            .bind(account_id)
            .execute(&self.pool)
            .await?;
        if disabled {
            self.end_sessions(account_id).await?;
        }
        Ok(())
    }

    /// How many enabled sysadmins exist (guards against locking everyone out).
    pub async fn active_sysadmins(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM account WHERE role = 'sysadmin' AND disabled = 0")
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0)
    }

    // ---- sessions ----

    pub async fn create_session(
        &self,
        token_hash: &str,
        account_id: i64,
        now: i64,
        expires_at: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO session (token_hash, account_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
        )
        .bind(token_hash)
        .bind(account_id)
        .bind(now)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The principal behind a live session on an enabled account.
    pub async fn session_principal(&self, token_hash: &str, now: i64) -> Option<Principal> {
        sqlx::query_as::<_, PrincipalRow>(
            "SELECT a.id, a.username, a.role FROM session s JOIN account a ON a.id = s.account_id
             WHERE s.token_hash = ? AND s.expires_at > ? AND a.disabled = 0",
        )
        .bind(token_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .map(Into::into)
    }

    pub async fn end_session(&self, token_hash: &str) {
        let _ = sqlx::query("DELETE FROM session WHERE token_hash = ?")
            .bind(token_hash)
            .execute(&self.pool)
            .await;
    }

    pub async fn end_sessions(&self, account_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM session WHERE account_id = ?")
            .bind(account_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn purge_expired_sessions(&self, now: i64) {
        let _ = sqlx::query("DELETE FROM session WHERE expires_at <= ?")
            .bind(now)
            .execute(&self.pool)
            .await;
    }

    // ---- IdP-provisioned accounts ----

    /// Find (or create) the account for an IdP subject, keeping its username in
    /// step with the IdP. `role` is applied when the IdP is authoritative for
    /// roles (a group mapping is configured); otherwise a new account starts as
    /// a user and keeps whatever role the console gives it. `None` if the
    /// account is disabled. When the IdP's username is taken by another
    /// account (e.g. a native one from before the switch), a new account gets
    /// `name-2`, `name-3`, … and an existing one keeps its current name —
    /// accounts are never linked by name.
    pub async fn idp_principal(
        &self,
        subject: &str,
        username: &str,
        role: Option<Role>,
        now: i64,
    ) -> Result<Option<Principal>, sqlx::Error> {
        // Every signed-in request lands here: answer from a read when nothing
        // changed, and only take the write lock when something did.
        let current: Option<(i64, String, String, bool)> = sqlx::query_as(
            "SELECT id, username, role, disabled FROM account WHERE idp_subject = ?",
        )
        .bind(subject)
        .fetch_optional(&self.pool)
        .await?;
        if let Some((id, name, r, disabled)) = &current {
            if *disabled {
                return Ok(None);
            }
            let role_ok = role.is_none_or(|want| want.as_str() == r);
            if role_ok && (name == username || self.username_taken(username, Some(*id)).await?) {
                return Ok(Some(
                    PrincipalRow {
                        id: *id,
                        username: name.clone(),
                        role: r.clone(),
                    }
                    .into(),
                ));
            }
        }
        // Reads then writes: take the write lock up front (see `transition`).
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing: Option<(i64, bool)> =
            sqlx::query_as("SELECT id, disabled FROM account WHERE idp_subject = ?")
                .bind(subject)
                .fetch_optional(&mut *tx)
                .await?;
        let id = match existing {
            Some((_, true)) => return Ok(None),
            Some((id, false)) => {
                sqlx::query(
                    "UPDATE account SET username = ? WHERE id = ? AND username <> ? \
                     AND NOT EXISTS (SELECT 1 FROM account WHERE username = ? AND id <> ?)",
                )
                .bind(username)
                .bind(id)
                .bind(username)
                .bind(username)
                .bind(id)
                .execute(&mut *tx)
                .await?;
                id
            }
            None => {
                let mut name = username.to_string();
                let mut n = 1;
                while sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM account WHERE username = ?",
                )
                .bind(&name)
                .fetch_one(&mut *tx)
                .await?
                    > 0
                {
                    n += 1;
                    name = format!("{username}-{n}");
                }
                if n > 1 {
                    tracing::warn!(
                        "forward-auth: username {username} is taken; the IdP user {subject} gets {name}"
                    );
                }
                sqlx::query(
                    "INSERT INTO account (username, idp_subject, role, created_at) VALUES (?, ?, ?, ?)",
                )
                .bind(&name)
                .bind(subject)
                .bind(role.unwrap_or(Role::User).as_str())
                .bind(now)
                .execute(&mut *tx)
                .await?
                .last_insert_rowid()
            }
        };
        if let Some(role) = role {
            sqlx::query("UPDATE account SET role = ? WHERE id = ?")
                .bind(role.as_str())
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        let p: PrincipalRow = sqlx::query_as("SELECT id, username, role FROM account WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(p.into()))
    }

    /// Is `username` held by an account other than `except`? (Case-insensitive,
    /// like the column.)
    async fn username_taken(
        &self,
        username: &str,
        except: Option<i64>,
    ) -> Result<bool, sqlx::Error> {
        let n: i64 =
            sqlx::query_scalar("SELECT count(*) FROM account WHERE username = ? AND id IS NOT ?")
                .bind(username)
                .bind(except)
                .fetch_one(&self.pool)
                .await?;
        Ok(n > 0)
    }

    // ---- links ----

    #[allow(clippy::too_many_arguments)]
    pub async fn create_link(
        &self,
        token_hash: &str,
        kind: LinkKind,
        account_id: Option<i64>,
        label: &str,
        max_uses: Option<i64>,
        expires_at: i64,
        created_by: Option<i64>,
        now: i64,
    ) -> Result<i64, sqlx::Error> {
        let r = sqlx::query(
            "INSERT INTO auth_link (token_hash, kind, account_id, label, max_uses, expires_at, created_by, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(token_hash)
        .bind(kind.as_str())
        .bind(account_id)
        .bind(label)
        .bind(max_uses)
        .bind(expires_at)
        .bind(created_by)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(r.last_insert_rowid())
    }

    pub async fn links(&self) -> Vec<LinkRow> {
        sqlx::query_as(
            "SELECT l.id, l.kind, l.label, a.username, l.max_uses, l.uses, l.expires_at,
                    l.revoked
             FROM auth_link l LEFT JOIN account a ON a.id = l.account_id
             ORDER BY l.created_at DESC, l.id DESC",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    pub async fn revoke_link(&self, id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE auth_link SET revoked = 1 WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Look up a still-redeemable link of one of `kinds` (without using it).
    pub async fn live_link(
        &self,
        token_hash: &str,
        kinds: &[LinkKind],
        now: i64,
    ) -> Option<LinkRow> {
        let row: LinkRow = sqlx::query_as(
            "SELECT l.id, l.kind, l.label, a.username, l.max_uses, l.uses, l.expires_at,
                    l.revoked
             FROM auth_link l LEFT JOIN account a ON a.id = l.account_id
             WHERE l.token_hash = ?",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()?;
        let kind_ok = kinds.iter().any(|k| k.as_str() == row.kind);
        (kind_ok && row.live(now)).then_some(row)
    }

    /// Consume one use of an invite link and create the account it admits,
    /// atomically: a taken username leaves the link's use count untouched.
    pub async fn redeem_invite(
        &self,
        token_hash: &str,
        username: &str,
        password_hash: &str,
        now: i64,
    ) -> Result<i64, RedeemError> {
        let db = |e: sqlx::Error| RedeemError::Db(e.to_string());
        let mut tx = self.pool.begin().await.map_err(db)?;
        let used = sqlx::query(
            "UPDATE auth_link SET uses = uses + 1
             WHERE token_hash = ? AND kind IN ('invite', 'community') AND revoked = 0
               AND expires_at > ? AND (max_uses IS NULL OR uses < max_uses)",
        )
        .bind(token_hash)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        if used.rows_affected() != 1 {
            return Err(RedeemError::BadLink);
        }
        let created = sqlx::query(
            "INSERT INTO account (username, password_hash, role, created_at) VALUES (?, ?, 'user', ?)",
        )
        .bind(username)
        .bind(password_hash)
        .bind(now)
        .execute(&mut *tx)
        .await;
        let id = match created {
            Ok(r) => r.last_insert_rowid(),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(RedeemError::UsernameTaken);
            }
            Err(e) => return Err(db(e)),
        };
        tx.commit().await.map_err(db)?;
        Ok(id)
    }

    /// Consume a reset link: set the account's new password and end all its
    /// sessions. Returns the account id, or `None` if the link isn't live.
    pub async fn redeem_reset(
        &self,
        token_hash: &str,
        password_hash: &str,
        now: i64,
    ) -> Result<Option<i64>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let account: Option<i64> = sqlx::query_scalar(
            "UPDATE auth_link SET uses = uses + 1
             WHERE token_hash = ? AND kind = 'reset' AND revoked = 0 AND expires_at > ?
               AND (max_uses IS NULL OR uses < max_uses)
             RETURNING account_id",
        )
        .bind(token_hash)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        let Some(account) = account else {
            return Ok(None);
        };
        sqlx::query("UPDATE account SET password_hash = ? WHERE id = ?")
            .bind(password_hash)
            .bind(account)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM session WHERE account_id = ?")
            .bind(account)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(account))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh database file per test (not `:memory:` — each pooled
    /// connection would get its own empty in-memory database).
    async fn db() -> Db {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("bifrost-authtest-{}-{n}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Db::open(&path.to_string_lossy()).await.unwrap()
    }

    #[tokio::test]
    async fn session_resolves_until_expiry_or_disable() {
        let db = db().await;
        let id = db
            .create_account("Ann", "h", Role::Sysadmin, 0)
            .await
            .unwrap();
        db.create_session("t", id, 0, 100).await.unwrap();
        let p = db.session_principal("t", 50).await.unwrap();
        assert_eq!((p.username.as_str(), p.role), ("Ann", Role::Sysadmin));
        assert!(db.session_principal("t", 100).await.is_none(), "expired");
        db.set_disabled(id, true).await.unwrap();
        db.set_disabled(id, false).await.unwrap();
        assert!(
            db.session_principal("t", 50).await.is_none(),
            "disable ends sessions"
        );
    }

    #[tokio::test]
    async fn usernames_are_case_insensitive() {
        let db = db().await;
        db.create_account("Ann", "h", Role::User, 0).await.unwrap();
        assert!(db.create_account("ann", "h", Role::User, 0).await.is_err());
        assert!(db.login_lookup("ANN").await.is_some());
    }

    #[tokio::test]
    async fn single_use_invite_admits_one_account() {
        let db = db().await;
        db.create_link("inv", LinkKind::Invite, None, "", Some(1), 100, None, 0)
            .await
            .unwrap();
        assert!(db.redeem_invite("inv", "a", "h", 10).await.is_ok());
        assert_eq!(
            db.redeem_invite("inv", "b", "h", 10).await,
            Err(RedeemError::BadLink)
        );
    }

    #[tokio::test]
    async fn community_invite_is_multi_use_until_expiry_or_revocation() {
        let db = db().await;
        let id = db
            .create_link(
                "com",
                LinkKind::Community,
                None,
                "discord",
                None,
                100,
                None,
                0,
            )
            .await
            .unwrap();
        assert!(db.redeem_invite("com", "a", "h", 10).await.is_ok());
        assert!(db.redeem_invite("com", "b", "h", 10).await.is_ok());
        assert_eq!(
            db.redeem_invite("com", "c", "h", 100).await,
            Err(RedeemError::BadLink)
        );
        db.revoke_link(id).await.unwrap();
        assert_eq!(
            db.redeem_invite("com", "d", "h", 10).await,
            Err(RedeemError::BadLink)
        );
        assert_eq!(db.links().await[0].uses, 2);
    }

    #[tokio::test]
    async fn taken_username_does_not_burn_an_invite() {
        let db = db().await;
        db.create_account("a", "h", Role::User, 0).await.unwrap();
        db.create_link("inv", LinkKind::Invite, None, "", Some(1), 100, None, 0)
            .await
            .unwrap();
        assert_eq!(
            db.redeem_invite("inv", "A", "h", 10).await,
            Err(RedeemError::UsernameTaken)
        );
        assert!(db.redeem_invite("inv", "b", "h", 10).await.is_ok());
    }

    #[tokio::test]
    async fn reset_link_sets_password_once_and_ends_sessions() {
        let db = db().await;
        let id = db.create_account("a", "old", Role::User, 0).await.unwrap();
        db.create_session("s", id, 0, 100).await.unwrap();
        db.create_link("r", LinkKind::Reset, Some(id), "", Some(1), 100, None, 0)
            .await
            .unwrap();
        assert!(
            db.redeem_invite("r", "x", "h", 10).await.is_err(),
            "not an invite"
        );
        assert_eq!(db.redeem_reset("r", "new", 10).await.unwrap(), Some(id));
        assert_eq!(db.login_lookup("a").await.unwrap().1, "new");
        assert!(db.session_principal("s", 10).await.is_none());
        assert_eq!(db.redeem_reset("r", "again", 10).await.unwrap(), None);
    }

    #[tokio::test]
    async fn idp_accounts_follow_the_idp() {
        let db = db().await;
        let p = db
            .idp_principal("sub1", "ann", None, 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(p.role, Role::User);
        db.set_role(p.account_id, Role::Moderator).await.unwrap();
        // No mapping: the console's role sticks; username tracks the IdP.
        let p2 = db
            .idp_principal("sub1", "anna", None, 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (p2.account_id, p2.username.as_str(), p2.role),
            (p.account_id, "anna", Role::Moderator)
        );
        // Mapping configured: the IdP is authoritative.
        let p3 = db
            .idp_principal("sub1", "anna", Some(Role::Sysadmin), 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(p3.role, Role::Sysadmin);
        assert!(
            db.login_lookup("anna").await.is_none(),
            "no native login for IdP accounts"
        );
        db.set_disabled(p.account_id, true).await.unwrap();
        assert!(
            db.idp_principal("sub1", "anna", None, 0)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn idp_names_never_collide_with_other_accounts() {
        let db = db().await;
        let native = db
            .create_account("Alice", "h", Role::Sysadmin, 0)
            .await
            .unwrap();
        // A new IdP user whose name is taken gets a suffix, not the account.
        let p = db
            .idp_principal("sub-a", "alice", None, 0)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(p.account_id, native);
        assert_eq!((p.username.as_str(), p.role), ("alice-2", Role::User));
        let q = db
            .idp_principal("sub-b", "alice", None, 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(q.username, "alice-3");
        // Renamed by the IdP to a taken name: keeps its own.
        let r = db
            .idp_principal("sub-b", "ALICE-2", None, 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (r.account_id, r.username.as_str()),
            (q.account_id, "alice-3")
        );
    }

    #[tokio::test]
    async fn per_user_rows_follow_the_account() {
        use crate::db::ClaimOutcome;
        let db = db().await;
        let p = db
            .idp_principal("sub", "old", None, 0)
            .await
            .unwrap()
            .unwrap();
        let s = db.create_season("S", None, 0).await.unwrap();
        sqlx::query(
            "INSERT INTO bifrost_slot (season, name, ap_slot, game) VALUES (?, 'Slot', 1, 'G')",
        )
        .bind(s)
        .execute(&db.pool)
        .await
        .unwrap();
        let slot = db.slots(s).await.unwrap()[0].id;
        assert!(matches!(
            db.claim_one(s, slot, p.account_id).await.unwrap(),
            ClaimOutcome::Ok
        ));
        db.add_credits(s, p.account_id, 2).await.unwrap();
        db.set_mc_username(p.account_id, "Steve").await.unwrap();

        // The IdP renames the user: everything stays theirs, under the new name.
        db.idp_principal("sub", "new", None, 0).await.unwrap();
        let row = &db.slots(s).await.unwrap()[0];
        assert_eq!(row.claimed_by.as_deref(), Some("new"));
        assert_eq!(row.claimed_by_id, Some(p.account_id));
        assert_eq!(db.credits(s, p.account_id).await, 2);
        assert_eq!(db.mc_whitelist(s).await, ["Steve"]);

        // Someone else can neither take nor release it.
        let other = db.create_account("x", "h", Role::User, 0).await.unwrap();
        assert!(matches!(
            db.claim_one(s, slot, other).await.unwrap(),
            ClaimOutcome::SlotTaken
        ));
        assert!(!db.unclaim(s, slot, other, false).await.unwrap());
        // Nor can another season's admins, through their own season.
        let s2 = db.create_season("S2", None, 0).await.unwrap();
        assert!(!db.unclaim(s2, slot, other, true).await.unwrap());
        assert!(matches!(
            db.claim_one(s2, slot, other).await.unwrap(),
            ClaimOutcome::SlotTaken
        ));
        assert!(db.slot_detail(s2, slot).await.unwrap().is_none());

        // Deleting the account cascades its rows away.
        sqlx::query("DELETE FROM account WHERE id = ?")
            .bind(p.account_id)
            .execute(&db.pool)
            .await
            .unwrap();
        assert!(db.slots(s).await.unwrap()[0].claimed_by.is_none());
        assert_eq!(db.credits(s, p.account_id).await, 0);
    }
}
