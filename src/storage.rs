#![allow(dead_code)]

use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Database {
    conn: Connection,
}

#[derive(Debug)]
pub struct BinaryRecord {
    pub path: String,
    pub count: i64,
    pub first_seen: Option<i64>,
    pub last_seen: Option<i64>,
    pub source: Option<String>,
    pub package_name: Option<String>,
}

impl Database {
    pub fn open() -> Result<Self> {
        let path = Self::db_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn db_path() -> Result<PathBuf> {
        let data_dir = dirs::data_local_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find local data directory"))?;
        Ok(data_dir.join("dustbin").join("dustbin.db"))
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS binaries (
                path TEXT PRIMARY KEY,
                count INTEGER DEFAULT 0,
                first_seen INTEGER,
                last_seen INTEGER
            );

            CREATE TABLE IF NOT EXISTS packages (
                manager TEXT NOT NULL,
                name TEXT NOT NULL,
                binary_path TEXT NOT NULL,
                PRIMARY KEY (manager, name, binary_path)
            );

            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_packages_binary ON packages(binary_path);
            ",
        )?;
        Ok(())
    }

    pub fn record_exec(&self, path: &str) -> Result<()> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

        self.conn.execute(
            "
            INSERT INTO binaries (path, count, first_seen, last_seen)
            VALUES (?1, 1, ?2, ?2)
            ON CONFLICT(path) DO UPDATE SET
                count = count + 1,
                last_seen = ?2
            ",
            params![path, now],
        )?;
        Ok(())
    }

    pub fn get_tracking_since(&self) -> Result<Option<i64>> {
        let result: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'tracking_since'",
                [],
                |row| row.get(0),
            )
            .ok();

        Ok(result.and_then(|v| v.parse().ok()))
    }

    pub fn set_tracking_since(&self, timestamp: i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('tracking_since', ?1)",
            params![timestamp.to_string()],
        )?;
        Ok(())
    }

    pub fn get_all_binaries(&self) -> Result<Vec<BinaryRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT b.path, b.count, b.first_seen, b.last_seen, p.manager, p.name
             FROM binaries b
             LEFT JOIN packages p ON b.path = p.binary_path
             ORDER BY b.count DESC",
        )?;

        let records = stmt.query_map([], |row| {
            Ok(BinaryRecord {
                path: row.get(0)?,
                count: row.get(1)?,
                first_seen: row.get(2)?,
                last_seen: row.get(3)?,
                source: row.get(4)?,
                package_name: row.get(5)?,
            })
        })?;

        records.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_binary_count(&self) -> Result<i64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM binaries", [], |row| row.get(0))?;
        Ok(count)
    }

    pub fn cache_package(&self, manager: &str, name: &str, binary_path: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO packages (manager, name, binary_path) VALUES (?1, ?2, ?3)",
            params![manager, name, binary_path],
        )?;
        Ok(())
    }

    pub fn get_package_for_binary(&self, binary_path: &str) -> Result<Option<(String, String)>> {
        let result = self
            .conn
            .query_row(
                "SELECT manager, name FROM packages WHERE binary_path = ?1",
                params![binary_path],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();
        Ok(result)
    }

    /// Register a binary from a package manager (with count = 0 if new)
    pub fn register_binary(&self, path: &str, package_name: &str, manager: &str) -> Result<bool> {
        // Insert into binaries table if not exists (count = 0)
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO binaries (path, count, first_seen, last_seen) VALUES (?1, 0, NULL, NULL)",
            params![path],
        )?;

        // Cache the package mapping
        self.cache_package(manager, package_name, path)?;

        Ok(inserted > 0)
    }

    /// Get count of dusty (never used) binaries
    pub fn get_dusty_count(&self) -> Result<i64> {
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM binaries WHERE count = 0", [], |row| {
                    row.get(0)
                })?;
        Ok(count)
    }
}
