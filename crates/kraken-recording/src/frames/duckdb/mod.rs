//! The DuckDB market tape: the columnar recording backend, behind the
//! `duckdb` feature.
//!
//! Writer and reader live side by side — [`DuckdbSink`] persists what
//! [`DuckdbSource`] yields back — over the shared per-channel row codecs in
//! [`tables`] and the data-driven schema in [`table`]. This module itself
//! hosts [`Db`]: a thin, schema-agnostic wrapper over a file-backed `duckdb`
//! connection that bundles the cross-cutting concerns every consumer needs —
//! an advisory [`FileLock`] for a clean single-writer rejection (DuckDB
//! itself permits only one read-write process per file; the lock turns the
//! second process's failure into a friendly rejection before the engine
//! opens), `CREATE TABLE IF NOT EXISTS` schema application, a key/value
//! `_meta` upsert/read, and small scalar-read helpers.

mod sink;
mod source;
pub(crate) mod table;
pub(crate) mod tables;

use std::path::Path;

use duckdb::{Connection, params};
pub use sink::DuckdbSink;
pub use source::DuckdbSource;

use crate::error::{Error, Result};
use crate::lock::FileLock;
use crate::schema;

/// One version gate for both DuckDB halves: `_meta` must exist as a table
/// (probed here — a foreign DuckDB file has none, and querying it would
/// surface a raw catalog error) and carry a stamp sharing this build's
/// MAJOR. Returns the validated stamp — the source's drift policy derives
/// from it. `action` names the refused operation ("append" for the sink,
/// "read" for the source).
pub(crate) fn ensure_recording_stamp(db: &Db, action: &str) -> Result<String> {
    let not_a_recording = || {
        Error::Rejected(format!(
            "existing file is not a kraken tape (no schema metadata); \
             refusing to {action}"
        ))
    };
    if !table::table_exists(db.conn(), "_meta")? {
        return Err(not_a_recording());
    }
    match db.meta_get("schema_version")? {
        Some(found) => {
            schema::ensure_compatible(&found, action)?;
            Ok(found)
        }
        None => Err(not_a_recording()),
    }
}

/// A file-backed DuckDB connection: locked and writable for sinks, lock-free
/// and read-only for sources.
pub struct Db {
    conn: Connection,
    // Held for its `Drop`, which releases the advisory lock when the `Db` (and so
    // the connection) is dropped.
    _lock: Option<FileLock>,
}

impl Db {
    /// Open (creating if absent) the DuckDB file at `path`, after acquiring
    /// the advisory lock at `<path>.lock` — so a second writer is rejected
    /// here, cleanly, not by the engine.
    pub fn open(path: &Path) -> Result<Self> {
        let resource = format!("tape '{}'", path.display());
        let lock = FileLock::acquire(path, resource)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Ok(Self {
            conn,
            _lock: Some(lock),
        })
    }

    /// Open an existing database read-only, without its own advisory lock, so
    /// lock policy stays with the caller. A live writer still excludes readers
    /// at the engine level; callers acquire a *shared* [`FileLock`] first to
    /// turn that into a clean "in use" rejection before the engine's raw error
    /// (and to keep concurrent readers from excluding one another).
    pub fn open_read_only(path: &Path) -> Result<Self> {
        let config = duckdb::Config::default().access_mode(duckdb::AccessMode::ReadOnly)?;
        let conn = Connection::open_with_flags(path, config)?;
        Ok(Self { conn, _lock: None })
    }

    /// Borrow the connection for prepared statements (callers outside this
    /// crate, e.g. `kraken feedback`, insert through the same locked handle).
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        self.conn.execute_batch(sql)?;
        Ok(())
    }

    /// Roll back the open transaction on an error path. A failure is logged, not
    /// returned: the caller is already propagating the original error, and the
    /// engine discards the transaction with the connection anyway.
    pub(crate) fn rollback(&self) {
        if let Err(err) = self.execute_batch("ROLLBACK") {
            tracing::warn!(%err, "ROLLBACK failed; the transaction dies with the connection");
        }
    }

    /// Each statement must be idempotent (`CREATE TABLE IF NOT EXISTS` style):
    /// reopen re-applies the whole list.
    pub fn ensure_schema(&self, ddl: &[&str]) -> Result<()> {
        for stmt in ddl {
            self.conn.execute_batch(stmt)?;
        }
        Ok(())
    }

    /// Upsert a `_meta` key/value pair (the `_meta` table must already exist).
    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO _meta (key, value) VALUES (?, ?)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare("SELECT value FROM _meta WHERE key = ?")?;
        let mut rows = stmt.query(params![key])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get::<_, String>(0)?)),
            None => Ok(None),
        }
    }

    /// Run a single-column, single-row query and return it as a `u64` (counts).
    /// A test/diagnostic convenience: production readers go through the
    /// generated per-table queries in [`table`].
    pub fn scalar_u64(&self, sql: &str) -> Result<u64> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query([])?;
        let row = rows
            .next()?
            .ok_or_else(|| Error::Damaged("query returned no rows".into()))?;
        let value: i64 = row.get(0)?;
        u64::try_from(value)
            .map_err(|_| Error::Damaged(format!("query returned a negative count: {value}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_roundtrip_and_scalar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.duckdb");
        let db = Db::open(&path).unwrap();
        db.ensure_schema(&[
            "CREATE TABLE IF NOT EXISTS _meta (key VARCHAR PRIMARY KEY, value VARCHAR)",
        ])
        .unwrap();
        assert_eq!(db.meta_get("schema_version").unwrap(), None);
        db.meta_set("schema_version", "1.0").unwrap();
        db.meta_set("schema_version", "1.0").unwrap(); // upsert is idempotent
        assert_eq!(
            db.meta_get("schema_version").unwrap().as_deref(),
            Some("1.0")
        );
        assert_eq!(db.scalar_u64("SELECT count(*) FROM _meta").unwrap(), 1);
    }

    #[test]
    fn second_open_of_same_tape_is_rejected() {
        // AC#1 (clean single-writer rejection): while one Db holds the tape,
        // a second open of the same path fails with a `validation` error.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.duckdb");
        let _first = Db::open(&path).unwrap();
        // `Db` holds a non-Debug duckdb connection, so match rather than unwrap_err.
        // Pinned through the envelope mapping: a held tape must stay `validation`.
        match Db::open(&path) {
            Ok(_) => panic!("second open of a held tape must be rejected"),
            Err(e) => assert!(matches!(e, Error::Rejected(_))),
        }
    }
}
