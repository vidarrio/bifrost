//! Instance settings the sysadmin edits on the settings page (as opposed to
//! deployment config in `config.toml`): what the public home page says and
//! the host players connect to.

use serde::{Deserialize, Serialize};

use super::Db;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Instance {
    /// The host players' AP clients connect to (each live season adds its
    /// port), e.g. `ap.example.org`. Empty: boards say to ask an admin.
    pub domain: String,
    /// An optional "home" link in the header (e.g. the operator's homepage).
    pub home_url: String,
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
            discords: vec![
                Discord {
                    name: "Archipelago".into(),
                    url: "https://discord.gg/archipelago".into(),
                    note: "official".into(),
                },
                // The invite Archipelago's own launcher links to.
                Discord {
                    name: "AP After Dark".into(),
                    url: "https://discord.gg/fqvNCCRsu4".into(),
                    note: "unrated and 18+ games".into(),
                },
            ],
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

    /// The common-game export token's hash, if the sysadmin made one.
    pub async fn export_token_hash(&self) -> Option<String> {
        self.setting("export_token").await.filter(|h| !h.is_empty())
    }

    /// Set the export token's hash, or drop it (`None`).
    pub async fn set_export_token_hash(&self, hash: Option<&str>) -> Result<(), sqlx::Error> {
        match hash {
            Some(h) => self.set_setting("export_token", h).await,
            None => {
                sqlx::query("DELETE FROM setting WHERE key = 'export_token'")
                    .execute(&self.pool)
                    .await?;
                Ok(())
            }
        }
    }

    /// The key for known-device cookies: made once, then kept.
    pub async fn device_key(&self) -> String {
        if let Some(k) = self.setting("device_key").await.filter(|k| !k.is_empty()) {
            return k;
        }
        let k = crate::auth::new_token();
        let _ = self.set_setting("device_key", &k).await;
        self.setting("device_key").await.unwrap_or(k)
    }

    /// A consistent copy of the whole database at `path` (for backups).
    pub async fn snapshot_to(&self, path: &std::path::Path) -> Result<(), sqlx::Error> {
        sqlx::query("VACUUM INTO ?")
            .bind(path.to_string_lossy().into_owned())
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
    #[test]
    fn settings_saved_with_the_old_password_note_still_load() {
        let i: super::Instance =
            serde_json::from_str(r#"{"domain":"x.example","password_note":"ask"}"#).unwrap();
        assert_eq!(i.domain, "x.example");
    }

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
