//! DuckDB-backed recording sink.
//!
//! Each parsed [`ChannelMessage`] frame is dispatched to its channel's table
//! via [`crate::frames::duckdb::tables`] — one transaction per drained batch
//! over the reusable [`Db`]. The schema is generated from the per-channel
//! [`TableSpec`](crate::frames::duckdb::table::TableSpec)s, so DDL and inserts
//! can't drift. A monotonic per-frame `seq`, persisted in `_meta.last_seq`,
//! resumes on reopen so a frame's rows stay groupable across sessions. Opening
//! enforces the version contract — a fresh file is stamped `schema_version`; an
//! existing tape is appended to when compatible and cleanly rejected when not.

use crate::{
    error::{Error, Result},
    frames::{
        CaptureSink,
        capture::{RecordingDeclaration, RecordingFilter, RecordingIntegrity},
        duckdb::{Db, tables},
    },
    schema,
    sink::Sink,
};
use chrono::Utc;
use duckdb::params;
use kraken_core::ChannelMessage;

/// Union a reopened session's list into a comma-joined `_meta` value,
/// first-seen order preserved.
fn absorb_meta_list(db: &Db, key: &str, run: &[String]) -> Result<()> {
    let stored = db.meta_get(key)?.unwrap_or_default();
    let mut merged: Vec<String> = stored
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    for item in run {
        if !merged.contains(item) {
            merged.push(item.clone());
        }
    }
    db.meta_set(key, &merged.join(","))
}

pub struct DuckdbSink {
    db: Db,
    // Monotonic per-frame id shared by every row of a frame, resumed from
    // `_meta.last_seq` on reopen so two sessions never collide a `seq`.
    seq: i64,
    /// The shared per-frame capture gate ([`RecordingFilter`]); a frame outside
    /// this session's declared channels has no table to land in and is skipped,
    /// not an error.
    gate: RecordingFilter,
}

impl DuckdbSink {
    /// Open (creating if absent) the tape at `path`, acquire its advisory lock,
    /// ensure the schema, and apply the version contract. Errors before
    /// returning if another process holds the tape, or if an existing
    /// tape's schema is incompatible / not a kraken recording (so the
    /// command fails before opening any socket).
    pub fn open(path: &std::path::Path, meta: &RecordingDeclaration) -> Result<Self> {
        let db = Db::open(path)?;
        // An empty database (no tables at all) cannot hold recorded data: it
        // is either brand new or the artifact of a crash inside a previous
        // open(), between the engine creating the file and the bootstrap
        // transaction committing. Either way, bootstrap it; a file with
        // tables but no stamp stays refused below.
        let is_new = db.scalar_u64(
            "SELECT count(*) FROM information_schema.tables WHERE table_catalog = \
             current_database() AND table_schema = 'main'",
        )? == 0;
        // Generate the DDL for `_meta` plus a table for each channel this session
        // records, and apply it idempotently — so a tape carries only the
        // tables it uses, not an empty table for every channel. Reopening to
        // record an additional channel adds its table via the same `CREATE
        // TABLE IF NOT EXISTS`; existing tables are left intact.
        let declared = crate::frames::capture::declared_channels(meta);
        let specs = tables::schema_tables_for(&declared);
        let mut ddl = vec![schema::META_DDL.to_string()];
        ddl.extend(specs.iter().map(|spec| spec.create_sql()));
        // CREATE TABLE IF NOT EXISTS never alters an existing table, so a
        // tape written before a spec gained a nullable column migrates here.
        ddl.extend(specs.iter().flat_map(|spec| spec.add_column_sql()));
        let stmts: Vec<&str> = ddl.iter().map(String::as_str).collect();
        // Bootstrap atomically: the DDL plus the version stamp/validation run in one
        // transaction, so a crash during open() leaves the tape either fully
        // stamped or absent — never a table set with no `schema_version`.
        db.execute_batch("BEGIN TRANSACTION")?;
        let bootstrap = (|| -> Result<()> {
            db.ensure_schema(&stmts)?;
            if is_new {
                db.meta_set("schema_version", schema::SCHEMA_VERSION)?;
                db.meta_set("source", &meta.source)?;
                db.meta_set("symbols", &meta.symbols.join(","))?;
                db.meta_set("channels", &meta.channels.join(","))?;
                db.meta_set(
                    "window_start",
                    &crate::time::format_instant(meta.window_start),
                )?;
                db.meta_set("cli_version", &meta.cli_version)?;
            } else {
                crate::frames::duckdb::ensure_recording_stamp(&db, "append")?;
                // A reopen restarts the capture: clear the finalize stamp so a crash
                // reads back unfinalized, and absorb this session's identity lists.
                db.execute_batch(
                    "DELETE FROM _meta WHERE key IN ('window_end', 'events_dropped', \
                     'frames_unparsed', 'reconnect_count')",
                )?;
                absorb_meta_list(&db, "symbols", &meta.symbols)?;
                absorb_meta_list(&db, "channels", &meta.channels)?;
            }
            Ok(())
        })();
        if let Err(e) = bootstrap {
            db.rollback();
            return Err(e);
        }
        db.execute_batch("COMMIT")?;

        // Resume the per-frame seq so a reopened session continues the sequence
        // rather than restarting at 0 and colliding with an earlier session's ids.
        // A present-but-unparseable value means a corrupt tape — refuse rather
        // than silently reset to 0 (which would collide new rows with existing ones).
        let seq = match db.meta_get("last_seq")? {
            None => 0,
            Some(s) => s.parse::<i64>().map_err(|_| {
                Error::Damaged(format!(
                    "tape has a corrupt last_seq metadata value '{s}'; refusing to write"
                ))
            })?,
        };

        Ok(Self {
            db,
            seq,
            gate: RecordingFilter::new(declared),
        })
    }

    /// Write one batch's frames in the open transaction: advance the per-frame
    /// seq, dispatch each frame to its channel table, then persist the seq
    /// high-water mark in the same transaction so a crash mid-session still
    /// resumes cleanly. Borrows `self.db` (the connection), `self.gate`,
    /// and `self.seq` as disjoint fields.
    fn write_batch(&mut self, batch: &[ChannelMessage], recv_ts: &str) -> Result<()> {
        let conn = self.db.conn();
        for frame in batch {
            // Only frames that persist rows advance the seq, so numbering
            // matches the JSONL backend's line numbers and `_meta.last_seq`
            // never counts phantom frames.
            if !self.gate.admits(frame) {
                continue;
            }
            self.seq += 1;
            tables::record_frame(conn, frame, self.seq, recv_ts)?;
        }
        conn.execute(
            "INSERT OR REPLACE INTO _meta (key, value) VALUES ('last_seq', ?)",
            params![self.seq.to_string()],
        )
        .map_err(Error::from)?;
        Ok(())
    }
}

impl Sink for DuckdbSink {
    type Item = ChannelMessage;

    fn record(&mut self, batch: &[ChannelMessage]) -> Result<()> {
        let recv_ts = crate::time::format_instant(Utc::now());
        // A failed batch rolls back and ends the session, so advancing the seq on
        // the error path is harmless (no later batch observes it).
        self.db.execute_batch("BEGIN TRANSACTION")?;
        match self.write_batch(batch, &recv_ts) {
            Ok(()) => {
                self.db.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(e) => {
                self.db.rollback();
                Err(e)
            }
        }
    }
}

impl CaptureSink for DuckdbSink {
    fn finalize(self, summary: RecordingIntegrity) -> Result<()> {
        // One transaction: `window_end` is the completeness signal, so it must
        // never be observable without its loss/reconnect counts.
        self.db.execute_batch("BEGIN TRANSACTION")?;
        let stamp = (|| -> Result<()> {
            self.db
                .meta_set("events_dropped", &summary.events_dropped.to_string())?;
            self.db
                .meta_set("frames_unparsed", &summary.frames_unparsed.to_string())?;
            self.db
                .meta_set("reconnect_count", &summary.reconnect_count.to_string())?;
            self.db.meta_set(
                "window_end",
                &crate::time::format_instant(summary.window_end),
            )
        })();
        if let Err(e) = stamp {
            self.db.rollback();
            return Err(e);
        }
        self.db.execute_batch("COMMIT")?;
        // Fold the WAL into the main file so a later reader sees everything.
        self.db.execute_batch("CHECKPOINT")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::{capture::RecordingIntegrity, duckdb::Db};
    use std::path::Path;

    fn meta() -> RecordingDeclaration {
        RecordingDeclaration {
            source: schema::SOURCE.to_string(),
            symbols: vec!["BTC/USD".to_string()],
            channels: vec![
                "ticker".into(),
                "trades".into(),
                "book".into(),
                "ohlc".into(),
            ],
            window_start: "2026-01-01T00:00:00Z".parse().unwrap(),
            cli_version: "test".to_string(),
        }
    }

    fn ticker_frame(last: f64) -> ChannelMessage {
        let raw = format!(
            r#"{{"channel":"ticker","type":"update","data":[{{"symbol":"BTC/USD",
               "bid":1.0,"bid_qty":2.0,"ask":3.0,"ask_qty":4.0,"last":{last},
               "volume":5.0,"vwap":6.0,"low":0.5,"high":9.0,"change":-1.0,"change_pct":-2.5,
               "timestamp":"2026-01-01T00:00:00.000000Z"}}]}}"#
        );
        ChannelMessage::parse(&raw).expect("ticker frame parses")
    }

    fn trade_frame(symbol: &str, price: f64, qty: f64, id: u64) -> ChannelMessage {
        let raw = format!(
            r#"{{"channel":"trade","type":"update","data":[{{"symbol":"{symbol}","side":"buy",
               "price":{price},"qty":{qty},"ord_type":"market","trade_id":{id},
               "timestamp":"2026-01-01T00:00:00.000000Z"}}]}}"#
        );
        ChannelMessage::parse(&raw).expect("trade frame parses")
    }

    fn book_snapshot() -> ChannelMessage {
        let raw = r#"{"channel":"book","type":"snapshot","data":[{"symbol":"BTC/USD",
            "bids":[{"price":100.0,"qty":1.0},{"price":99.5,"qty":2.0}],
            "asks":[{"price":100.5,"qty":3.0}],"checksum":4242,
            "timestamp":"2026-01-01T00:00:00.000000Z"}]}"#;
        ChannelMessage::parse(raw).expect("book frame parses")
    }

    fn ohlc_frame(timestamp: Option<&str>) -> ChannelMessage {
        let timestamp = timestamp
            .map(|ts| format!(r#","timestamp":"{ts}""#))
            .unwrap_or_default();
        let raw = format!(
            r#"{{"channel":"ohlc","type":"update","data":[{{"symbol":"BTC/USD",
            "open":1.0,"high":2.0,"low":0.5,"close":1.5,"vwap":1.2,"trades":7,"volume":123.0,
            "interval_begin":"2026-01-01T00:00:00.000000000Z","interval":1{timestamp}}}]}}"#
        );
        ChannelMessage::parse(&raw).expect("ohlc frame parses")
    }

    fn count(path: &Path, table: &str) -> u64 {
        let db = Db::open(path).unwrap();
        db.scalar_u64(&format!("SELECT count(*) FROM {table}"))
            .unwrap()
    }

    #[test]
    fn captures_all_channels_and_reads_back() {
        // The core capture-and-read proof: one frame per channel, then reopen the
        // file and assert the rows landed with the expected shape and exact decimals.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.duckdb");

        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(&[
            ticker_frame(62590.8),
            trade_frame("BTC/USD", 50000.1, 0.5, 1),
            book_snapshot(),
            ohlc_frame(None),
        ])
        .unwrap();
        sink.finalize(RecordingIntegrity {
            window_end: "2026-01-01T00:01:00Z".parse().unwrap(),
            ..RecordingIntegrity::now()
        })
        .unwrap();

        assert_eq!(count(&path, "ticker"), 1);
        assert_eq!(count(&path, "trades"), 1);
        // 2 bids + 1 ask = 3 book level rows, all sharing one seq.
        assert_eq!(count(&path, "book"), 3);
        assert_eq!(count(&path, "ohlc"), 1);

        let db = Db::open(&path).unwrap();
        assert_eq!(
            db.meta_get("schema_version").unwrap().as_deref(),
            Some(schema::SCHEMA_VERSION)
        );
        // Decimals stored exactly (no float drift), across ticker and trades.
        assert_eq!(
            db.scalar_u64("SELECT count(*) FROM ticker WHERE last = 62590.8")
                .unwrap(),
            1
        );
        assert_eq!(
            db.scalar_u64("SELECT count(*) FROM trades WHERE price = 50000.1")
                .unwrap(),
            1
        );
        // The book frame's levels group under a single seq.
        assert_eq!(
            db.scalar_u64("SELECT count(DISTINCT seq) FROM book")
                .unwrap(),
            1
        );
    }

    #[test]
    fn creates_only_the_recorded_channel_tables() {
        // A session recording one channel must not create empty tables for the others —
        // the tape carries only `_meta` plus the channels it actually used.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.duckdb");
        let meta = RecordingDeclaration {
            channels: vec!["ticker".into()],
            ..meta()
        };

        let mut sink = DuckdbSink::open(&path, &meta).unwrap();
        sink.record(&[ticker_frame(1.0)]).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let db = Db::open(&path).unwrap();
        let exists = |t: &str| {
            db.scalar_u64(&format!(
                "SELECT count(*) FROM information_schema.tables WHERE table_name = '{t}'"
            ))
            .unwrap()
        };
        assert_eq!(
            exists("ticker"),
            1,
            "the recorded channel's table is created"
        );
        assert_eq!(exists("_meta"), 1, "_meta is always present");
        assert_eq!(exists("trades"), 0, "an unused channel gets no table");
        assert_eq!(exists("book"), 0, "an unused channel gets no table");
        assert_eq!(exists("ohlc"), 0, "an unused channel gets no table");
    }

    #[test]
    fn reopening_to_add_a_channel_creates_the_new_table_and_keeps_the_old() {
        // Reopening the same tape to record a *different* channel adds that table while
        // leaving the first channel's data intact (CREATE TABLE IF NOT EXISTS).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.duckdb");

        let mut s1 = DuckdbSink::open(
            &path,
            &RecordingDeclaration {
                channels: vec!["ticker".into()],
                ..meta()
            },
        )
        .unwrap();
        s1.record(&[ticker_frame(1.0)]).unwrap();
        s1.finalize(RecordingIntegrity::now()).unwrap();

        let mut s2 = DuckdbSink::open(
            &path,
            &RecordingDeclaration {
                channels: vec!["trade".into()],
                ..meta()
            },
        )
        .unwrap();
        s2.record(&[trade_frame("BTC/USD", 1.0, 1.0, 1)]).unwrap();
        s2.finalize(RecordingIntegrity::now()).unwrap();

        assert_eq!(count(&path, "ticker"), 1, "first session's rows survive");
        assert_eq!(
            count(&path, "trades"),
            1,
            "second session's table was added"
        );
    }

    #[test]
    fn reopen_continues_seq_so_frames_stay_distinct() {
        // Reopening must not reset `seq`: a book frame written in a second session
        // gets a fresh seq, so grouping by seq never merges it with an earlier
        // session's frame (the read contract). Without the resume, both
        // frames would share seq=1.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.duckdb");

        let mut s1 = DuckdbSink::open(&path, &meta()).unwrap();
        s1.record(&[book_snapshot()]).unwrap();
        s1.finalize(RecordingIntegrity::now()).unwrap();

        let mut s2 = DuckdbSink::open(&path, &meta()).unwrap();
        s2.record(&[book_snapshot()]).unwrap();
        s2.finalize(RecordingIntegrity::now()).unwrap();

        let db = Db::open(&path).unwrap();
        assert_eq!(
            db.scalar_u64("SELECT count(DISTINCT seq) FROM book")
                .unwrap(),
            2,
            "each session's frame must get its own seq"
        );
    }

    #[test]
    fn frame_for_an_undeclared_channel_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.duckdb");
        let ticker_only = RecordingDeclaration {
            channels: vec!["ticker".into()],
            ..meta()
        };

        let mut sink = DuckdbSink::open(&path, &ticker_only).unwrap();
        sink.record(&[trade_frame("BTC/USD", 1.0, 1.0, 1), ticker_frame(2.0)])
            .unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        assert_eq!(count(&path, "ticker"), 1, "declared channel lands");
        let db = Db::open(&path).unwrap();
        assert_eq!(
            db.scalar_u64(
                "SELECT count(*) FROM information_schema.tables WHERE table_name = 'trades'"
            )
            .unwrap(),
            0,
            "the undeclared channel's table is never created"
        );
    }

    #[test]
    fn empty_unstamped_file_is_rescued_as_new() {
        // The crash artifact: Db::open created the physical file, then the
        // process died before the bootstrap transaction committed. An empty
        // database holds no recorded data, so reopening bootstraps it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.duckdb");
        drop(Db::open(&path).unwrap()); // file exists, zero tables, no stamp

        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(&[trade_frame("BTC/USD", 1.0, 1.0, 1)]).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        assert_eq!(count(&path, "trades"), 1);
        let db = Db::open(&path).unwrap();
        assert_eq!(
            db.meta_get("schema_version").unwrap().as_deref(),
            Some(schema::SCHEMA_VERSION)
        );
    }

    #[test]
    fn unstamped_file_with_tables_stays_refused() {
        // Only the empty crash artifact is rescued: a file that holds tables
        // could hold data this build knows nothing about.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.duckdb");
        {
            let db = Db::open(&path).unwrap();
            db.ensure_schema(&["CREATE TABLE IF NOT EXISTS foreign_data (x INTEGER)"])
                .unwrap();
        }

        let err = match DuckdbSink::open(&path, &meta()) {
            Ok(_) => panic!("an unstamped file with tables must be refused"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::Rejected(_)));
        assert!(err.to_string().contains("not a kraken tape"), "got: {err}");
    }

    #[test]
    fn ohlc_table_from_before_the_timestamp_column_is_migrated_on_open() {
        // Appending to a same-stamp tape written before the ohlc
        // `timestamp` column existed must not fail mid-capture: open migrates
        // the table (ADD COLUMN IF NOT EXISTS) before any frame binds to it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.duckdb");
        let sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();
        Db::open(&path)
            .unwrap()
            .execute_batch("ALTER TABLE ohlc DROP COLUMN timestamp")
            .unwrap();

        let candle = ohlc_frame(Some("2026-01-01T00:00:04.000000Z"));
        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(std::slice::from_ref(&candle)).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        assert_eq!(count(&path, "ohlc"), 1);
    }

    #[test]
    fn frames_that_persist_no_rows_do_not_advance_seq() {
        // A spec-less status frame writes nothing; burning a seq for it would
        // desynchronize numbering from the JSONL backend and make `last_seq`
        // count phantom frames.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.duckdb");
        let status = ChannelMessage::parse(
            r#"{"channel":"status","type":"update","data":[{"system":"online",
               "api_version":"v2","connection_id":1,"version":"2.0"}]}"#,
        )
        .unwrap();

        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(&[status, trade_frame("BTC/USD", 1.0, 1.0, 1)])
            .unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let db = Db::open(&path).unwrap();
        assert_eq!(db.meta_get("last_seq").unwrap().as_deref(), Some("1"));
        assert_eq!(
            db.scalar_u64("SELECT count(*) FROM trades WHERE seq = 1")
                .unwrap(),
            1,
            "the persisting frame takes the first seq"
        );
    }

    #[test]
    fn newer_minor_tape_appends_without_downgrading_the_stamp() {
        // Same MAJOR, newer MINOR: this build may append, and the stamp keeps
        // naming the newer vocabulary — the JSONL sidecar twin of
        // finalize_keeps_the_first_runs_version_stamp.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.duckdb");
        {
            let mut s = DuckdbSink::open(&path, &meta()).unwrap();
            s.record(&[trade_frame("BTC/USD", 1.0, 1.0, 1)]).unwrap();
            s.finalize(RecordingIntegrity::now()).unwrap();
        }
        {
            let db = Db::open(&path).unwrap();
            db.meta_set("schema_version", "1.7").unwrap();
        }

        let mut sink = DuckdbSink::open(&path, &meta()).expect("newer minor appends");
        sink.record(&[trade_frame("BTC/USD", 2.0, 1.0, 2)]).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        assert_eq!(count(&path, "trades"), 2);
        let db = Db::open(&path).unwrap();
        assert_eq!(
            db.meta_get("schema_version").unwrap().as_deref(),
            Some("1.7")
        );
    }

    #[test]
    fn incompatible_schema_is_reported_not_mishandled() {
        // AC#2: an incompatible on-disk schema version is a clean validation error.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.duckdb");

        {
            let mut s = DuckdbSink::open(&path, &meta()).unwrap();
            s.record(&[trade_frame("BTC/USD", 1.0, 1.0, 1)]).unwrap();
            s.finalize(RecordingIntegrity::now()).unwrap();
        }
        // Simulate a future, breaking (different MAJOR) schema version on disk.
        {
            let db = Db::open(&path).unwrap();
            db.meta_set("schema_version", "2.0").unwrap();
        }

        // `DuckdbSink` holds a non-Debug connection, so match rather than unwrap_err.
        let err = match DuckdbSink::open(&path, &meta()) {
            Ok(_) => panic!("incompatible schema must be rejected"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::Rejected(_)));
        assert!(err.to_string().contains("incompatible"));
    }

    #[test]
    fn corrupt_last_seq_is_rejected_not_silently_reset() {
        // A present-but-unparseable last_seq means a corrupt tape; reopening must
        // refuse rather than reset to 0 and collide new rows with the existing ones.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.duckdb");
        {
            let mut s = DuckdbSink::open(&path, &meta()).unwrap();
            s.record(&[trade_frame("BTC/USD", 1.0, 1.0, 1)]).unwrap();
            s.finalize(RecordingIntegrity::now()).unwrap();
        }
        {
            let db = Db::open(&path).unwrap();
            db.meta_set("last_seq", "not-a-number").unwrap();
        }

        let err = match DuckdbSink::open(&path, &meta()) {
            Ok(_) => panic!("corrupt last_seq must be rejected"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::Damaged(_)));
    }

    #[test]
    fn finalize_persists_loss_and_reconnect_metadata() {
        // A session that dropped events or reconnected stamps those counts into `_meta`
        // so a reader can tell the tape has holes / snapshot resets.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.duckdb");

        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(&[trade_frame("BTC/USD", 1.0, 1.0, 1)]).unwrap();
        sink.finalize(RecordingIntegrity {
            window_end: "2026-01-01T00:01:00Z".parse().unwrap(),
            events_dropped: 7,
            frames_unparsed: 3,
            reconnect_count: 2,
        })
        .unwrap();

        let db = Db::open(&path).unwrap();
        assert_eq!(db.meta_get("events_dropped").unwrap().as_deref(), Some("7"));
        assert_eq!(
            db.meta_get("frames_unparsed").unwrap().as_deref(),
            Some("3")
        );
        assert_eq!(
            db.meta_get("reconnect_count").unwrap().as_deref(),
            Some("2")
        );
    }

    #[test]
    fn large_trade_id_round_trips_through_ubigint() {
        // A trade_id beyond i64::MAX would wrap to negative in a signed column; the
        // UBIGINT column stores it losslessly.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.duckdb");
        let big = u64::MAX - 1;

        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(&[trade_frame("BTC/USD", 1.0, 1.0, big)])
            .unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let db = Db::open(&path).unwrap();
        assert_eq!(
            db.scalar_u64(&format!(
                "SELECT count(*) FROM trades WHERE trade_id = {big}"
            ))
            .unwrap(),
            1
        );
    }
}
