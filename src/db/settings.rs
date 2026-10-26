//! Instance settings the sysadmin edits on the settings page (as opposed to
//! deployment config in `config.toml`): what the public home page says and
//! the host players connect to.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::Db;

pub(super) async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS setting (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Instance {
    /// The host players' AP clients connect to (each live season adds its
    /// port), e.g. `ap.example.org`. Empty: "ask an admin".
    pub domain: String,
    /// An optional "home" link in the header (e.g. the operator's homepage).
    pub home_url: String,
    /// Shown under the connect address: where players get the password.
    pub password_note: String,
    /// Community Discord servers listed on the home page.
    pub discords: Vec<Discord>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Discord {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub note: String,
}

impl Default for Instance {
    fn default() -> Self {
        Self {
            domain: String::new(),
            home_url: String::new(),
            password_note: "Ask a moderator for the password.".into(),
            discords: vec![Discord {
                name: "Archipelago".into(),
                url: "https://discord.gg/archipelago".into(),
                note: "official".into(),
            }],
        }
    }
}

/// Whether uploaded apworlds that aren't in the catalog can be used right
/// away, or wait for a moderator or sysadmin to approve them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approval {
    Auto,
    Manual,
}

impl Approval {
    pub fn as_str(self) -> &'static str {
        match self {
            Approval::Auto => "auto",
            Approval::Manual => "manual",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Approval::Auto),
            "manual" => Some(Approval::Manual),
            _ => None,
        }
    }
}

impl Db {
    pub(super) async fn get_setting(&self, key: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT value FROM setting WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
    }

    async fn setting(&self, key: &str) -> Option<String> {
        self.get_setting(key).await.ok().flatten()
    }

    pub(super) async fn set_setting(&self, key: &str, value: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO setting (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// How uploaded apworlds are approved (manual until a sysadmin changes it).
    pub async fn approval(&self) -> Approval {
        self.setting("apworld_approval")
            .await
            .and_then(|v| Approval::parse(&v))
            .unwrap_or(Approval::Manual)
    }

    pub async fn set_approval(&self, a: Approval) -> Result<(), sqlx::Error> {
        self.set_setting("apworld_approval", a.as_str()).await
    }

    /// The Archipelago version new generations and checks use, if a sysadmin
    /// picked one (otherwise the bundled install).
    pub async fn ap_active(&self) -> Option<String> {
        self.setting("ap_active").await.filter(|v| !v.is_empty())
    }

    pub async fn set_ap_active(&self, version: Option<&str>) -> Result<(), sqlx::Error> {
        self.set_setting("ap_active", version.unwrap_or("")).await
    }

    /// The instance settings (defaults until a sysadmin saves them).
    pub async fn instance(&self) -> Instance {
        self.setting("instance")
            .await
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default()
    }

    pub async fn set_instance(&self, inst: &Instance) -> Result<(), sqlx::Error> {
        let v = serde_json::to_string(inst).expect("settings serialize");
        self.set_setting("instance", &v).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn instance_settings_round_trip() {
        let dir = std::env::temp_dir().join(format!("bifrost-settings-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("t.db");
        let _ = std::fs::remove_file(&path);
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        assert_eq!(db.instance().await, Instance::default());
        let inst = Instance {
            domain: "ap.example.org".into(),
            discords: vec![],
            ..Instance::default()
        };
        db.set_instance(&inst).await.unwrap();
        assert_eq!(db.instance().await, inst);
        assert_eq!(db.approval().await, Approval::Manual);
        db.set_approval(Approval::Auto).await.unwrap();
        assert_eq!(db.approval().await, Approval::Auto);
        assert_eq!(db.ap_active().await, None);
        db.set_ap_active(Some("0.6.9")).await.unwrap();
        assert_eq!(db.ap_active().await.as_deref(), Some("0.6.9"));
        db.set_ap_active(None).await.unwrap();
        assert_eq!(db.ap_active().await, None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
