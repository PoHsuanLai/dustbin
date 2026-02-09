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
        Ok(data_dir.join("dusty").join("dusty.db"))
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS binaries (
                path TEXT PRIMARY KEY,
                count INTEGER DEFAULT 0,
                first_seen INTEGER,
                last_seen INTEGER,
                source TEXT,
                package_name TEXT
            );

            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT
            );

            CREATE TABLE IF NOT EXISTS dylib_deps (
                binary_path TEXT NOT NULL,
                lib_path TEXT NOT NULL,
                PRIMARY KEY (binary_path, lib_path)
            );

            CREATE TABLE IF NOT EXISTS lib_packages (
                lib_path TEXT PRIMARY KEY,
                manager TEXT NOT NULL,
                package_name TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS deps_meta (
                binary_path TEXT PRIMARY KEY,
                analyzed_at INTEGER NOT NULL,
                binary_mtime INTEGER
            );

            CREATE INDEX IF NOT EXISTS idx_dylib_deps_lib ON dylib_deps(lib_path);

            CREATE TABLE IF NOT EXISTS path_aliases (
                alias_path TEXT PRIMARY KEY,
                canonical_path TEXT NOT NULL
            );
            ",
        )?;

        Ok(())
    }

    pub fn record_exec(&self, path: &str, source: Option<&str>) -> Result<()> {
        // Check if this path is an alias (resolved symlink) for a canonical path
        let canonical = self.resolve_alias(path)?;
        let effective_path = canonical.as_deref().unwrap_or(path);

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

        self.conn.execute(
            "
            INSERT INTO binaries (path, count, first_seen, last_seen, source)
            VALUES (?1, 1, ?2, ?2, ?3)
            ON CONFLICT(path) DO UPDATE SET
                count = count + 1,
                last_seen = ?2
            ",
            params![effective_path, now, source],
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
            "SELECT path, count, last_seen, source, package_name
             FROM binaries
             ORDER BY count DESC",
        )?;

        let records = stmt.query_map([], |row| {
            Ok(BinaryRecord {
                path: row.get(0)?,
                count: row.get(1)?,
                last_seen: row.get(2)?,
                source: row.get(3)?,
                package_name: row.get(4)?,
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

    /// Register a binary from a package manager scan (with count = 0 if new).
    /// Uses COALESCE to fill in missing fields without clobbering existing data.
    pub fn register_binary(&self, path: &str, package_name: &str, source: &str) -> Result<bool> {
        let rows = self.conn.execute(
            "INSERT INTO binaries (path, count, first_seen, last_seen, source, package_name)
             VALUES (?1, 0, NULL, NULL, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET
                 source = COALESCE(binaries.source, excluded.source),
                 package_name = COALESCE(binaries.package_name, excluded.package_name)",
            params![path, source, package_name],
        )?;
        Ok(rows > 0)
    }

    /// Backfill source and package_name for binaries discovered by the daemon
    /// that haven't been categorized yet (package_name IS NULL).
    pub fn backfill_uncategorized<F>(&self, categorize: F) -> Result<u64>
    where
        F: Fn(&str) -> (String, String),
    {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM binaries WHERE package_name IS NULL")?;
        let paths: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        let mut count = 0u64;
        for path in &paths {
            let (source, pkg_name) = categorize(path);
            self.conn.execute(
                "UPDATE binaries SET source = ?2, package_name = ?3 WHERE path = ?1",
                params![path, source, pkg_name],
            )?;
            count += 1;
        }
        Ok(count)
    }

    /// Remove binaries from the database whose files no longer exist on disk.
    pub fn prune_missing(&self) -> Result<u64> {
        let mut stmt = self.conn.prepare("SELECT path FROM binaries")?;
        let paths: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        let mut count = 0u64;
        for path in &paths {
            if !std::path::Path::new(path).exists() {
                self.conn.execute(
                    "DELETE FROM binaries WHERE path = ?1",
                    params![path],
                )?;
                // Also clean up aliases pointing to this binary
                self.conn.execute(
                    "DELETE FROM path_aliases WHERE canonical_path = ?1",
                    params![path],
                )?;
                count += 1;
            }
        }
        Ok(count)
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

    // --- Dynamic library dependency methods ---

    /// Store dynamic library dependencies for a binary (replaces any existing)
    pub fn store_dylib_deps(&self, binary_path: &str, lib_paths: &[String]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM dylib_deps WHERE binary_path = ?1",
            params![binary_path],
        )?;
        for lib_path in lib_paths {
            tx.execute(
                "INSERT OR IGNORE INTO dylib_deps (binary_path, lib_path) VALUES (?1, ?2)",
                params![binary_path, lib_path],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Cache a library path → package mapping
    pub fn store_lib_package(
        &self,
        lib_path: &str,
        manager: &str,
        package_name: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO lib_packages (lib_path, manager, package_name) VALUES (?1, ?2, ?3)",
            params![lib_path, manager, package_name],
        )?;
        Ok(())
    }

    /// Mark a binary as analyzed with its current mtime
    pub fn mark_deps_analyzed(&self, binary_path: &str, mtime: Option<i64>) -> Result<()> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
        self.conn.execute(
            "INSERT OR REPLACE INTO deps_meta (binary_path, analyzed_at, binary_mtime) VALUES (?1, ?2, ?3)",
            params![binary_path, now, mtime],
        )?;
        Ok(())
    }

    /// Get when a binary was last analyzed and its mtime at that time
    pub fn get_deps_analyzed_at(&self, binary_path: &str) -> Result<Option<(i64, Option<i64>)>> {
        let result = self
            .conn
            .query_row(
                "SELECT analyzed_at, binary_mtime FROM deps_meta WHERE binary_path = ?1",
                params![binary_path],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();
        Ok(result)
    }

    /// Get all library paths that haven't been resolved to a package yet
    pub fn get_unresolved_libs(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT d.lib_path FROM dylib_deps d
             LEFT JOIN lib_packages lp ON d.lib_path = lp.lib_path
             WHERE lp.lib_path IS NULL",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all binary paths that use a given library
    pub fn get_binaries_using_lib(&self, lib_path: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT binary_path FROM dylib_deps WHERE lib_path = ?1",
        )?;
        let rows = stmt.query_map(params![lib_path], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all resolved library packages: (lib_path, manager, package_name)
    pub fn get_all_lib_packages(&self) -> Result<Vec<(String, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT lib_path, manager, package_name FROM lib_packages",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Clear all dependency analysis cache (for --refresh)
    pub fn clear_all_deps(&self) -> Result<()> {
        self.conn.execute_batch(
            "DELETE FROM dylib_deps; DELETE FROM lib_packages; DELETE FROM deps_meta;",
        )?;
        Ok(())
    }

    // --- Path alias methods ---

    /// Register a resolved path as an alias for a canonical (symlink) path.
    /// e.g., alias="/opt/homebrew/Cellar/rust/1.93.0/bin/cargo" -> canonical="/opt/homebrew/bin/cargo"
    pub fn register_alias(&self, alias_path: &str, canonical_path: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO path_aliases (alias_path, canonical_path) VALUES (?1, ?2)",
            params![alias_path, canonical_path],
        )?;
        Ok(())
    }

    /// Look up the canonical path for a given alias (resolved path).
    /// Returns None if no alias mapping exists.
    pub fn resolve_alias(&self, alias_path: &str) -> Result<Option<String>> {
        let result = self
            .conn
            .query_row(
                "SELECT canonical_path FROM path_aliases WHERE alias_path = ?1",
                params![alias_path],
                |row| row.get(0),
            )
            .ok();
        Ok(result)
    }

    /// Get all alias paths (resolved symlink targets) as a set.
    /// Used to filter out phantom entries when detecting duplicates.
    pub fn get_all_alias_paths(&self) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT alias_path FROM path_aliases",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<Result<std::collections::HashSet<_>, _>>().map_err(Into::into)
    }

}
