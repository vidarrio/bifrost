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

impl Db {
    /// The instance settings (defaults until a sysadmin saves them).
    pub async fn instance(&self) -> Instance {
        sqlx::query_scalar::<_, String>("SELECT value FROM setting WHERE key = 'instance'")
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default()
    }

    pub async fn set_instance(&self, inst: &Instance) -> Result<(), sqlx::Error> {
        let v = serde_json::to_string(inst).expect("settings serialize");
        sqlx::query(
            "INSERT INTO setting (key, value) VALUES ('instance', ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(v)
        .execute(&self.pool)
        .await?;
        Ok(())
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
        let _ = std::fs::remove_dir_all(&dir);
    }
}
