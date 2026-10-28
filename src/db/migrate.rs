//! Schema versions. A database records its version in SQLite's
//! `user_version`: 0 is a fresh file, 1 is the baseline
//! ([`super::baseline`]), and each entry of [`STEPS`] adds one. Opening the
//! database runs the steps it hasn't had yet, each in its own transaction,
//! and refuses a database that a newer Bifrost has already moved past.
//!
//! The Bifrost version that last opened the database is kept as the
//! `bifrost_version` setting, so startup can tell an upgrade is happening and
//! back up before the schema changes (see `main`).

use sqlx::SqlitePool;

use super::Db;

/// Schema changes after the baseline, oldest first: `STEPS[i]` takes the
/// schema from version `i + 1` to `i + 2`. Append only: a released step is
/// never edited, reordered or removed. A step is plain SQL and may hold
/// several statements; SQLite can't alter a column in place, so changing one
/// means rebuilding its table (<https://sqlite.org/lang_altertable.html>).
/// Steps run with foreign keys off, so rebuilding a table others reference
/// doesn't cascade into them; the references are checked before the step
/// commits.
const STEPS: &[&str] = &[];

/// The schema version this build writes.
pub const SCHEMA_VERSION: i64 = 1 + STEPS.len() as i64;

const VERSION_KEY: &str = "bifrost_version";

/// Who last used a database: its schema version, and the Bifrost version
/// that last opened it (`None` for a fresh file).
#[derive(Debug, PartialEq)]
pub struct Stamp {
    pub schema: i64,
    pub version: Option<String>,
}

impl Stamp {
    /// The other Bifrost version that last opened this database, if this
    /// start is an upgrade (or a downgrade).
    pub fn changed_from(&self) -> Option<&str> {
        self.version
            .as_deref()
            .filter(|v| *v != env!("CARGO_PKG_VERSION"))
    }

    /// Was this database written by a newer Bifrost, with a schema this build
    /// doesn't know?
    pub fn too_new(&self) -> bool {
        self.schema > SCHEMA_VERSION
    }
}

impl Db {
    /// Read the database's [`Stamp`] without changing anything.
    pub async fn stamp(&self) -> Result<Stamp, sqlx::Error> {
        let schema = user_version(&self.pool).await?;
        // A fresh file has no setting table yet.
        let has_settings: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'setting')",
        )
        .fetch_one(&self.pool)
        .await?;
        let version = if has_settings {
            self.get_setting(VERSION_KEY).await?
        } else {
            None
        };
        Ok(Stamp { schema, version })
    }

    /// Bring the schema up to [`SCHEMA_VERSION`] and record this Bifrost's
    /// version as the one that last opened the database.
    pub async fn migrate(&self) -> Result<(), sqlx::Error> {
        apply(&self.pool, STEPS).await?;
        self.set_setting(VERSION_KEY, env!("CARGO_PKG_VERSION"))
            .await
    }
}

async fn user_version(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(pool)
        .await
}

async fn apply(pool: &SqlitePool, steps: &[&str]) -> Result<(), sqlx::Error> {
    let latest = 1 + steps.len() as i64;
    let mut at = user_version(pool).await?;
    if at > latest {
        return Err(sqlx::Error::Protocol(format!(
            "the database is at schema version {at}, newer than this Bifrost knows \
             ({latest}): a newer Bifrost has used it"
        )));
    }
    if at == 0 {
        super::baseline(pool).await?;
        sqlx::query("PRAGMA user_version = 1").execute(pool).await?;
        at = 1;
    }
    for (i, sql) in steps.iter().enumerate().skip(at as usize - 1) {
        let to = i as i64 + 2;
        // `foreign_keys` can't change inside a transaction: switch it off
        // around it, on this one connection, and back on whatever happens.
        let mut conn = pool.acquire().await?;
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await?;
        let res = step(&mut conn, sql, to).await;
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&mut *conn)
            .await?;
        res?;
        tracing::info!("database schema migrated to version {to}");
    }
    Ok(())
}

/// Run one step and stamp version `to`, all in one transaction, which rolls
/// back if the step fails or leaves a dangling reference.
async fn step(conn: &mut sqlx::SqliteConnection, sql: &str, to: i64) -> Result<(), sqlx::Error> {
    use sqlx::Connection;
    let mut tx = conn.begin().await?;
    sqlx::raw_sql(sql).execute(&mut *tx).await?;
    let dangling: Vec<(String, i64, String)> =
        sqlx::query_as("SELECT \"table\", rowid, parent FROM pragma_foreign_key_check")
            .fetch_all(&mut *tx)
            .await?;
    if let Some((table, row, parent)) = dangling.first() {
        return Err(sqlx::Error::Protocol(format!(
            "schema step to version {to} left {} reference(s) dangling, e.g. {table} row {row} \
             pointing into {parent}",
            dangling.len()
        )));
    }
    sqlx::raw_sql(&format!("PRAGMA user_version = {to}"))
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh(name: &str) -> (std::path::PathBuf, Db) {
        let dir =
            std::env::temp_dir().join(format!("bifrost-migrate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::connect(dir.join("t.db").to_str().unwrap())
            .await
            .unwrap();
        (dir, db)
    }

    #[tokio::test]
    async fn a_fresh_database_gets_the_baseline_and_this_version() {
        let (dir, db) = fresh("fresh").await;
        assert_eq!(
            db.stamp().await.unwrap(),
            Stamp {
                schema: 0,
                version: None
            }
        );
        db.migrate().await.unwrap();
        let stamp = db.stamp().await.unwrap();
        assert_eq!(stamp.schema, SCHEMA_VERSION);
        assert_eq!(stamp.version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
        assert_eq!(stamp.changed_from(), None);
        // Opening it again changes nothing.
        db.migrate().await.unwrap();
        assert_eq!(db.stamp().await.unwrap(), stamp);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn steps_run_once_each_in_order() {
        let (dir, db) = fresh("steps").await;
        let steps = [
            "CREATE TABLE t (a INTEGER)",
            "ALTER TABLE t ADD COLUMN b TEXT NOT NULL DEFAULT 'x'; INSERT INTO t (a) VALUES (1)",
        ];
        apply(&db.pool, &steps[..1]).await.unwrap();
        assert_eq!(user_version(&db.pool).await.unwrap(), 2);
        // A later build with one more step runs only the new one.
        apply(&db.pool, &steps).await.unwrap();
        apply(&db.pool, &steps).await.unwrap();
        assert_eq!(user_version(&db.pool).await.unwrap(), 3);
        let rows: Vec<(i64, String)> = sqlx::query_as("SELECT a, b FROM t")
            .fetch_all(&db.pool)
            .await
            .unwrap();
        assert_eq!(rows, vec![(1, "x".to_string())]);
        // An older build refuses the database rather than run on it.
        assert!(apply(&db.pool, &steps[..1]).await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_failed_step_leaves_the_database_at_the_version_before() {
        let (dir, db) = fresh("failed").await;
        let steps = [
            "CREATE TABLE t (a INTEGER)",
            "CREATE TABLE u (a); SELECT nope FROM t",
        ];
        assert!(apply(&db.pool, &steps).await.is_err());
        assert_eq!(user_version(&db.pool).await.unwrap(), 2);
        let u: Option<String> =
            sqlx::query_scalar("SELECT name FROM sqlite_master WHERE name = 'u'")
                .fetch_optional(&db.pool)
                .await
                .unwrap();
        assert_eq!(u, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn rebuilding_a_referenced_table_keeps_the_rows_that_reference_it() {
        let (dir, db) = fresh("rebuild").await;
        let steps = [
            "CREATE TABLE p (id INTEGER PRIMARY KEY, a TEXT);
             CREATE TABLE c (id INTEGER PRIMARY KEY, p INTEGER NOT NULL REFERENCES p(id) ON DELETE CASCADE);
             INSERT INTO p VALUES (1, 'x'); INSERT INTO c VALUES (1, 1);",
            // The rebuild SQLite's docs prescribe for changing a column.
            "CREATE TABLE p_new (id INTEGER PRIMARY KEY, a TEXT NOT NULL DEFAULT '');
             INSERT INTO p_new SELECT id, a FROM p;
             DROP TABLE p;
             ALTER TABLE p_new RENAME TO p;",
        ];
        apply(&db.pool, &steps).await.unwrap();
        let kids: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM c")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(kids, 1, "the rebuild cascaded into the child table");
        // Foreign keys are back on afterwards (the pool hands back the
        // idle connection the step ran on).
        let on: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(on, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_step_that_leaves_dangling_references_is_rolled_back() {
        let (dir, db) = fresh("dangling").await;
        let steps = [
            "CREATE TABLE p (id INTEGER PRIMARY KEY);
             CREATE TABLE c (id INTEGER PRIMARY KEY, p INTEGER REFERENCES p(id));
             INSERT INTO p VALUES (1); INSERT INTO c VALUES (1, 1);",
            "DELETE FROM p",
        ];
        let err = apply(&db.pool, &steps).await.unwrap_err().to_string();
        assert!(err.contains("dangling"), "{err}");
        let on: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(on, 1, "foreign keys stay off after a failed step");
        assert_eq!(user_version(&db.pool).await.unwrap(), 2);
        let parents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM p")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(parents, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stamp_tells_an_upgrade_from_the_same_version() {
        let s = |v: Option<&str>| Stamp {
            schema: SCHEMA_VERSION,
            version: v.map(str::to_string),
        };
        assert_eq!(s(None).changed_from(), None);
        assert_eq!(s(Some(env!("CARGO_PKG_VERSION"))).changed_from(), None);
        assert_eq!(s(Some("0.0.0-old")).changed_from(), Some("0.0.0-old"));
        assert!(!s(None).too_new());
        assert!(
            Stamp {
                schema: SCHEMA_VERSION + 1,
                version: None
            }
            .too_new()
        );
    }
}
