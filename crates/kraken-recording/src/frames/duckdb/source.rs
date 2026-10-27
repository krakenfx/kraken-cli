//! The DuckDB market-frame source — read pendant of
//! [`DuckdbSink`](crate::frames::duckdb::DuckdbSink). Yields the recorded
//! frames ordered by the writer's own `seq`, each channel table
//! read back through its [`Recordable::frames`] inverse.

use crate::{
    error::{Error, Result},
    frames::{
        CaptureSource, MarketEvent,
        capture::{RecordingDeclaration, RecordingIntegrity, RecordingManifest},
        clamp_non_decreasing,
        duckdb::{
            Db,
            table::{DriftPolicy, Recordable},
        },
        event_instant, shared_capture_lock,
    },
    lock::FileLock,
    schema,
    source::Source,
};
use kraken_core::subscribe::message::{BookData, OhlcData, TickerData, TradeData};
use std::path::Path;

pub struct DuckdbSource {
    db: Db,
    /// Decided once at open from the recording stamp: a newer-minor writer's
    /// unknown vocabulary is skipped, anything else fails loud — the same
    /// rule as the JSONL source's drift gate.
    drift: DriftPolicy,
    /// Held shared for the source's lifetime: a starting recorder gets the
    /// lock's clean "in use" rejection instead of the engine's raw error,
    /// other readers coexist, and the exclusive-only recorder-liveness probe
    /// never mistakes a long read for a live recorder.
    _lock: FileLock,
}

impl DuckdbSource {
    /// Rejects up front — before any decode — when the file is missing, held
    /// by a live writer, not a kraken recording, or a different schema MAJOR.
    pub fn open(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(Error::Rejected(format!(
                "tape '{}' not found",
                path.display()
            )));
        }
        // Acquiring — not probing — closes the check-then-read gap, and the
        // guard must exist before the engine opens so a racing writer is
        // rejected cleanly here rather than by DuckDB's raw engine error.
        let lock = shared_capture_lock(path)?;
        let db = Db::open_read_only(path)?;
        let stamp = crate::frames::duckdb::ensure_recording_stamp(&db, "read")?;
        let drift = if schema::is_newer_minor(&stamp) {
            DriftPolicy::TolerateNewerVocabulary
        } else {
            DriftPolicy::FailLoud
        };
        Ok(Self {
            db,
            drift,
            _lock: lock,
        })
    }

    /// Frames this tape holds, from the persisted `_meta.last_seq` high-water
    /// mark (the monotonic per-frame id). `None` on a tape written before the
    /// stamp existed.
    pub fn frame_count(&self) -> Result<Option<u64>> {
        Ok(self
            .db
            .meta_get("last_seq")?
            .and_then(|value| value.parse::<u64>().ok()))
    }

    /// Read one channel's frames back as market events; a channel this
    /// tape never recorded has no table and reads back empty. The merge
    /// instant is [`event_instant`] — the same rule as the JSONL backend, so
    /// the two yield one `at` sequence for one capture.
    fn collect<M: Recordable>(&self, events: &mut Vec<MarketEvent>) -> Result<()> {
        let conn = self.db.conn();
        for stored in M::frames(conn, self.drift)? {
            let Some(at) = event_instant(&stored.frame) else {
                return Err(Error::Damaged(format!(
                    "damaged recording: no parseable event timestamp in table {} at seq {}",
                    M::table().name,
                    stored.seq
                )));
            };
            events.push(MarketEvent {
                at,
                seq: stored.seq,
                frame: stored.frame,
            });
        }
        Ok(())
    }
}

impl Source for DuckdbSource {
    type Item = MarketEvent;

    /// All recorded frames in write order: the writer's `seq` is monotonic
    /// per frame across channels, so one sort by it restores capture order.
    /// `event_ts` is not monotone in that order (OHLC can report
    /// `interval_begin`), so the merge instant is then clamped non-decreasing
    /// — the composer's `(at, track, seq)` sort must not reorder replay
    /// against capture (the JSONL backend clamps identically).
    fn read(&self) -> Result<impl Iterator<Item = MarketEvent>> {
        // One collect per recordable channel (`capture::RECORDABLE_CHANNELS`);
        // the generic parameter is compile-time dispatch a value list can't
        // provide.
        let mut events = Vec::new();
        self.collect::<TickerData>(&mut events)?;
        self.collect::<TradeData>(&mut events)?;
        self.collect::<BookData>(&mut events)?;
        self.collect::<OhlcData>(&mut events)?;
        events.sort_by_key(|event| event.seq);
        clamp_non_decreasing(&mut events);
        Ok(events.into_iter())
    }
}

impl CaptureSource for DuckdbSource {
    /// The report rebuilt from `_meta`. Reopen clears the summary keys and
    /// finalize stamps them in one transaction, so any missing count next to
    /// a `window_end` is damage.
    fn capture(&self) -> Result<RecordingManifest> {
        let text = |key: &str| -> Result<String> {
            self.db
                .meta_get(key)?
                .ok_or_else(|| Error::Damaged(format!("damaged recording: missing _meta {key}")))
        };
        let count = |key: &str| -> Result<u64> {
            let raw = text(key)?;
            raw.parse().map_err(|_| {
                Error::Damaged(format!("damaged recording: unreadable _meta {key} '{raw}'"))
            })
        };
        let list = |key: &str| -> Result<Vec<String>> {
            Ok(text(key)?
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect())
        };
        let instant = |key: &str, raw: &str| {
            crate::time::parse_instant(raw).ok_or_else(|| {
                Error::Damaged(format!("damaged recording: unreadable _meta {key} '{raw}'"))
            })
        };
        let summary = match self.db.meta_get("window_end")? {
            None => None,
            Some(raw) => Some(RecordingIntegrity {
                window_end: instant("window_end", &raw)?,
                events_dropped: count("events_dropped")?,
                frames_unparsed: count("frames_unparsed")?,
                reconnect_count: count("reconnect_count")?,
            }),
        };
        Ok(RecordingManifest {
            schema_version: text("schema_version")?,
            meta: RecordingDeclaration {
                source: text("source")?,
                symbols: list("symbols")?,
                channels: list("channels")?,
                window_start: instant("window_start", &text("window_start")?)?,
                cli_version: text("cli_version")?,
            },
            summary,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        frames::{
            CaptureSink,
            duckdb::DuckdbSink,
            jsonl::{TapeSink, TapeSource},
        },
        sink::Sink,
    };
    use kraken_core::{ChannelData, ChannelMessage};

    fn meta() -> RecordingDeclaration {
        RecordingDeclaration {
            source: schema::SOURCE.to_string(),
            symbols: vec!["BTC/USD".into()],
            channels: vec![
                "ticker".into(),
                "trade".into(),
                "book".into(),
                "ohlc".into(),
            ],
            window_start: "2026-01-01T00:00:00Z".parse().unwrap(),
            cli_version: "test".into(),
        }
    }

    /// One frame per channel, including the shapes that stress the inverse:
    /// a multi-entry trade frame and a book snapshot with both sides.
    fn frames() -> Vec<ChannelMessage> {
        [
            r#"{"channel":"ticker","type":"update","data":[{"symbol":"BTC/USD",
               "bid":1.0,"bid_qty":2.0,"ask":3.0,"ask_qty":4.0,"last":62590.8,
               "volume":5.0,"vwap":6.0,"low":0.5,"high":9.0,"change":-1.0,"change_pct":-2.5,
               "timestamp":"2026-01-01T00:00:01.000000Z"}]}"#,
            r#"{"channel":"trade","type":"update","data":[
               {"symbol":"BTC/USD","side":"buy","price":50000.1,"qty":0.5,"ord_type":"market",
                "trade_id":1,"timestamp":"2026-01-01T00:00:02.000000Z"},
               {"symbol":"BTC/USD","side":"sell","price":50000.2,"qty":0.25,"ord_type":"limit",
                "trade_id":2,"timestamp":"2026-01-01T00:00:02.500000Z"}]}"#,
            r#"{"channel":"book","type":"snapshot","data":[{"symbol":"BTC/USD",
               "bids":[{"price":100.0,"qty":1.0},{"price":99.5,"qty":2.0}],
               "asks":[{"price":100.5,"qty":3.0}],"checksum":4242,
               "timestamp":"2026-01-01T00:00:03.000000Z"}]}"#,
            r#"{"channel":"ohlc","type":"update","data":[{"symbol":"BTC/USD",
               "open":1.0,"high":2.0,"low":0.5,"close":1.5,"vwap":1.2,"trades":7,"volume":123.0,
               "interval_begin":"2026-01-01T00:00:00.000000000Z","interval":1,
               "timestamp":"2026-01-01T00:00:04.000000Z"}]}"#,
        ]
        .iter()
        .map(|raw| ChannelMessage::parse(raw).expect("fixture frame parses"))
        .collect()
    }

    /// One OHLC candle; the deprecated wire `timestamp` is the fixture's
    /// variable, so its round-trip cases share one body.
    fn candle(timestamp: Option<&str>) -> ChannelMessage {
        let timestamp = timestamp
            .map(|ts| format!(r#","timestamp":"{ts}""#))
            .unwrap_or_default();
        let raw = format!(
            r#"{{"channel":"ohlc","type":"update","data":[{{"symbol":"BTC/USD",
               "open":1.0,"high":2.0,"low":0.5,"close":1.5,"vwap":1.2,"trades":7,"volume":123.0,
               "interval_begin":"2026-01-01T00:00:00.000000000Z","interval":1{timestamp}}}]}}"#
        );
        ChannelMessage::parse(&raw).expect("candle parses")
    }

    fn market_payloads(events: &[MarketEvent]) -> Vec<ChannelMessage> {
        events.iter().map(|e| e.frame.clone()).collect()
    }

    #[test]
    fn reads_back_what_the_sink_wrote_in_write_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        // Two batches of the same channels: adjacent same-channel frames with
        // distinct seqs must regroup as separate frames, not merge.
        let written: Vec<_> = [frames(), frames()].concat();

        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(&written).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let source = DuckdbSource::open(&path).unwrap();
        let events: Vec<_> = source.read().unwrap().collect();
        assert_eq!(market_payloads(&events), written);
        assert!(
            events
                .windows(2)
                .all(|w| (w[0].at, w[0].seq) < (w[1].at, w[1].seq)),
            "recv_ts/seq order is strictly increasing"
        );
    }

    #[test]
    fn both_backends_yield_the_same_event_sequence() {
        // One capture, two backends, one read-back contract: the same
        // payloads under the same merge instants (`at` is event_ts in both)
        // and the same frame numbering (`seq`).
        let dir = tempfile::tempdir().unwrap();
        let duck_path = dir.path().join("market.duckdb");
        let jsonl_path = dir.path().join("market.jsonl");
        let status = ChannelMessage::parse(
            r#"{"channel":"status","type":"update","data":[{"system":"online",
               "api_version":"v2","connection_id":1,"version":"2.0"}]}"#,
        )
        .unwrap();
        // The status frame persists in neither backend and must shift the
        // numbering in neither.
        let written: Vec<_> = [vec![status], frames()].concat();

        let mut duck = DuckdbSink::open(&duck_path, &meta()).unwrap();
        duck.record(&written).unwrap();
        duck.finalize(RecordingIntegrity::now()).unwrap();
        let mut jsonl = TapeSink::open(&jsonl_path, &meta()).unwrap();
        jsonl.record(&written).unwrap();
        jsonl.finalize(RecordingIntegrity::now()).unwrap();

        let from_duck: Vec<_> = DuckdbSource::open(&duck_path)
            .unwrap()
            .read()
            .unwrap()
            .collect();
        let from_jsonl: Vec<_> = TapeSource::open(&jsonl_path)
            .expect("tape exists")
            .read()
            .unwrap()
            .collect();
        assert_eq!(market_payloads(&from_duck), market_payloads(&from_jsonl));
        assert_eq!(
            from_duck.iter().map(|e| e.at).collect::<Vec<_>>(),
            from_jsonl.iter().map(|e| e.at).collect::<Vec<_>>(),
            "one merge-instant sequence regardless of storage backend"
        );
        assert_eq!(
            from_duck.iter().map(|e| e.seq).collect::<Vec<_>>(),
            from_jsonl.iter().map(|e| e.seq).collect::<Vec<_>>(),
            "one frame numbering regardless of storage backend"
        );
    }

    #[test]
    fn capture_reports_the_same_stamp_as_the_sidecar_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let summary = RecordingIntegrity {
            window_end: "2026-01-01T00:01:00Z".parse().unwrap(),
            events_dropped: 7,
            frames_unparsed: 3,
            reconnect_count: 2,
        };
        let sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.finalize(summary.clone()).unwrap();

        let report = DuckdbSource::open(&path).unwrap().capture().unwrap();
        assert_eq!(report.schema_version, schema::SCHEMA_VERSION);
        assert_eq!(report.meta, meta());
        assert_eq!(report.summary, Some(summary));
    }

    #[test]
    fn unfinalized_capture_reads_with_no_summary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        drop(DuckdbSink::open(&path, &meta()).unwrap()); // crash: no finalize

        let report = DuckdbSource::open(&path).unwrap().capture().unwrap();
        assert_eq!(report.summary, None, "unfinalized: completeness unknown");
        assert_eq!(report.meta, meta(), "identity is stamped at open");
    }

    #[test]
    fn reopened_then_crashed_capture_reads_as_unfinalized() {
        // Reopen clears the stale finalize stamp — the old session's summary
        // would describe frames it knows nothing about.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let mut first = DuckdbSink::open(&path, &meta()).unwrap();
        first.record(&frames()).unwrap();
        first
            .finalize(RecordingIntegrity {
                window_end: "2026-01-01T00:01:00Z".parse().unwrap(),
                ..RecordingIntegrity::now()
            })
            .unwrap();

        let mut second = DuckdbSink::open(&path, &meta()).unwrap();
        second.record(&frames()).unwrap();
        drop(second); // crash: no finalize

        let report = DuckdbSource::open(&path).unwrap().capture().unwrap();
        assert_eq!(report.summary, None, "stale finalize stamp cleared");
    }

    #[test]
    fn resumed_capture_reads_all_runs_frames_with_the_last_summary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let first_run = frames();
        let mut first = DuckdbSink::open(&path, &meta()).unwrap();
        first.record(&first_run).unwrap();
        first
            .finalize(RecordingIntegrity {
                window_end: "2026-01-01T00:01:00Z".parse().unwrap(),
                events_dropped: 1,
                ..RecordingIntegrity::now()
            })
            .unwrap();

        let second_run = frames();
        let mut second = DuckdbSink::open(&path, &meta()).unwrap();
        second.record(&second_run).unwrap();
        let last_summary = RecordingIntegrity {
            window_end: "2026-01-01T00:02:00Z".parse().unwrap(),
            events_dropped: 9,
            frames_unparsed: 4,
            reconnect_count: 2,
        };
        second.finalize(last_summary.clone()).unwrap();

        let source = DuckdbSource::open(&path).unwrap();
        let events: Vec<_> = source.read().unwrap().collect();
        assert_eq!(
            market_payloads(&events),
            [first_run, second_run].concat(),
            "frames from all sessions, in write order"
        );
        assert!(
            events
                .windows(2)
                .all(|w| (w[0].at, w[0].seq) < (w[1].at, w[1].seq))
        );
        assert_eq!(source.capture().unwrap().summary, Some(last_summary));
    }

    #[test]
    fn reopen_with_new_channels_unions_the_reported_lists() {
        // The report must describe every frame the file holds, not just the
        // last session's subscription.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let ticker_only = RecordingDeclaration {
            channels: vec!["ticker".into()],
            ..meta()
        };
        let mut first = DuckdbSink::open(&path, &ticker_only).unwrap();
        first
            .record(&[ChannelMessage::parse(
                r#"{"channel":"ticker","type":"update","data":[{"symbol":"BTC/USD",
                   "bid":1.0,"bid_qty":2.0,"ask":3.0,"ask_qty":4.0,"last":5.0,
                   "volume":5.0,"vwap":6.0,"low":0.5,"high":9.0,"change":-1.0,"change_pct":-2.5,
                   "timestamp":"2026-01-01T00:00:01.000000Z"}]}"#,
            )
            .unwrap()])
            .unwrap();
        first.finalize(RecordingIntegrity::now()).unwrap();

        let trade_only = RecordingDeclaration {
            symbols: vec!["ETH/USD".into()],
            channels: vec!["trade".into()],
            ..meta()
        };
        let mut second = DuckdbSink::open(&path, &trade_only).unwrap();
        second
            .record(&[ChannelMessage::parse(
                r#"{"channel":"trade","type":"update","data":[{"symbol":"ETH/USD","side":"buy",
                   "price":1.0,"qty":1.0,"ord_type":"market","trade_id":1,
                   "timestamp":"2026-01-01T00:00:02.000000Z"}]}"#,
            )
            .unwrap()])
            .unwrap();
        second.finalize(RecordingIntegrity::now()).unwrap();

        let report = DuckdbSource::open(&path).unwrap().capture().unwrap();
        assert_eq!(report.meta.channels, ["ticker", "trade"]);
        assert_eq!(report.meta.symbols, ["BTC/USD", "ETH/USD"]);
    }

    #[test]
    fn ohlc_without_the_deprecated_timestamp_round_trips() {
        // Must read back as `None`, not a phantom `timestamp` the wire never sent.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let written = candle(None);

        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(std::slice::from_ref(&written)).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let events: Vec<_> = DuckdbSource::open(&path).unwrap().read().unwrap().collect();
        assert_eq!(market_payloads(&events), [written]);
    }

    #[test]
    fn ohlc_tape_from_before_the_timestamp_column_reads_back() {
        // A reader never migrates: a same-stamp tape written before the
        // ohlc `timestamp` column existed must read back with the field
        // absent, not fail with a binder error after the version gate
        // approved the file.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(&frames()).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();
        crate::frames::duckdb::Db::open(&path)
            .unwrap()
            .execute_batch("ALTER TABLE ohlc DROP COLUMN timestamp")
            .unwrap();

        let events: Vec<_> = DuckdbSource::open(&path).unwrap().read().unwrap().collect();
        let ohlc = events
            .iter()
            .find_map(|e| match &e.frame.body {
                ChannelData::Ohlc(entries) => Some(&entries[0]),
                _ => None,
            })
            .expect("the ohlc frame reads back");
        assert_eq!(
            ohlc.timestamp, None,
            "the missing column is the absent wire field"
        );
    }

    #[test]
    fn ohlc_timestamp_equal_to_interval_begin_round_trips_byte_identical() {
        // The deprecated wire timestamp is stored verbatim: reconstructing it
        // by comparison against interval_begin would read this frame back
        // without it, byte-different from the capture.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let written = candle(Some("2026-01-01T00:00:00.000000000Z"));

        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(std::slice::from_ref(&written)).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let read = market_payloads(
            &DuckdbSource::open(&path)
                .unwrap()
                .read()
                .unwrap()
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            serde_json::to_string(&read[0]).unwrap(),
            serde_json::to_string(&written).unwrap(),
            "the wire timestamp must survive even when it equals interval_begin"
        );
    }

    #[test]
    fn multi_symbol_book_frame_regroups_per_symbol_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let written = ChannelMessage::parse(
            r#"{"channel":"book","type":"update","data":[
               {"symbol":"BTC/USD","bids":[{"price":100.0,"qty":1.0}],
                "asks":[{"price":100.5,"qty":3.0}],"checksum":1,
                "timestamp":"2026-01-01T00:00:01.000000Z"},
               {"symbol":"ETH/USD","bids":[{"price":10.0,"qty":2.0}],
                "asks":[{"price":10.5,"qty":4.0}],"checksum":2,
                "timestamp":"2026-01-01T00:00:01.500000Z"}]}"#,
        )
        .unwrap();

        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(std::slice::from_ref(&written)).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let events: Vec<_> = DuckdbSource::open(&path).unwrap().read().unwrap().collect();
        assert_eq!(market_payloads(&events), [written]);
    }

    #[test]
    fn high_precision_decimal_round_trips_bit_identical() {
        // Pins read_decimal's exactness: 17 significant digits survive the
        // DECIMAL(38,18)->VARCHAR->parse path; a DOUBLE cast double-rounds 1 ulp.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let written = ChannelMessage::parse(
            r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","side":"buy",
               "price":0.30000000000000004,"qty":0.1,"ord_type":"market","trade_id":1,
               "timestamp":"2026-01-01T00:00:01.000000Z"}]}"#,
        )
        .unwrap();

        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(std::slice::from_ref(&written)).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let events: Vec<_> = DuckdbSource::open(&path).unwrap().read().unwrap().collect();
        assert_eq!(market_payloads(&events), [written]);
    }

    #[test]
    fn half_stamped_meta_is_damage_not_unfinalized() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();
        {
            let db = Db::open(&path).unwrap();
            db.execute_batch("DELETE FROM _meta WHERE key = 'events_dropped'")
                .unwrap();
        }

        let err = DuckdbSource::open(&path).unwrap().capture().unwrap_err();
        assert!(matches!(err, Error::Damaged(_)));
    }

    #[test]
    fn missing_tape_and_foreign_file_are_validation_errors() {
        // `DuckdbSource` holds a non-Debug duckdb connection, so match rather
        // than unwrap_err.
        let dir = tempfile::tempdir().unwrap();
        match DuckdbSource::open(&dir.path().join("absent.duckdb")) {
            Ok(_) => panic!("a missing tape must be rejected"),
            Err(e) => assert!(matches!(e, Error::Rejected(_))),
        }

        let foreign = dir.path().join("foreign.duckdb");
        {
            let db = Db::open(&foreign).unwrap();
            db.ensure_schema(&[schema::META_DDL]).unwrap();
        }
        match DuckdbSource::open(&foreign) {
            Ok(_) => panic!("a file without schema metadata must be rejected"),
            Err(e) => assert!(matches!(e, Error::Rejected(_))),
        }
    }

    #[test]
    fn read_back_frames_serialize_byte_identical_to_the_written_ones() {
        // The replay NDJSON contract. DuckDB renders DECIMAL(38,18) at full
        // scale on read ("13588.200000000000000000"); un-normalized, that
        // scale-18 Decimal double-rounds through serde-float and 13588.2
        // replays as 13588.199999999999 — numerically equal (so frame
        // equality can't catch it), byte-different on every consumer surface.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let written = ChannelMessage::parse(
            r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","side":"buy",
               "price":13588.2,"qty":0.5,"ord_type":"market","trade_id":1,
               "timestamp":"2026-01-01T00:00:01.000000Z"}]}"#,
        )
        .expect("trade frame parses");
        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(std::slice::from_ref(&written)).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let read = market_payloads(
            &DuckdbSource::open(&path)
                .unwrap()
                .read()
                .unwrap()
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            serde_json::to_string(&read[0]).unwrap(),
            serde_json::to_string(&written).unwrap(),
            "a replayed frame must serialize exactly like the captured one"
        );
    }

    #[test]
    fn different_major_tape_is_rejected_cleanly() {
        // Breaking bump, no migration path: rejection, not a decode attempt.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        {
            let db = Db::open(&path).unwrap();
            db.ensure_schema(&[schema::META_DDL]).unwrap();
            db.meta_set("schema_version", "2.0").unwrap();
        }
        match DuckdbSource::open(&path) {
            Ok(_) => panic!("a different-major tape must be rejected"),
            Err(e) => {
                assert!(matches!(e, Error::Rejected(_)));
                assert!(e.to_string().contains("incompatible"), "got: {e}");
            }
        }
    }

    #[test]
    fn unknown_vocabulary_row_in_current_schema_fails_loud() {
        // A corrupted current-schema recording (unknown enum value) must fail
        // the read, not silently skip the row: under this build's own stamp
        // there is no newer writer whose vocabulary it could be.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(&frames()).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();
        {
            let db = Db::open(&path).unwrap();
            db.execute_batch("UPDATE trades SET side = 'hedge' WHERE trade_id = 1")
                .unwrap();
        }

        // The Ok side (an iterator) isn't Debug, so match rather than unwrap_err.
        let err = match DuckdbSource::open(&path).unwrap().read() {
            Ok(_) => panic!("a damaged recording must be rejected"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::Damaged(_)), "got: {err}");
    }

    #[test]
    fn unknown_vocabulary_under_a_newer_minor_stamp_skips_the_whole_frame() {
        // The JSONL drift twin: a 1.7 writer's unknown values poison their
        // *frames* — both entries of the two-entry trade frame go, partial
        // frames never surface — while every other frame reads back intact.
        // Two distinct Vocabulary producers fire (trade side, book side), so
        // the policy is pinned per producer, not just per table.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let written = frames();
        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(&written).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();
        {
            let db = Db::open(&path).unwrap();
            db.execute_batch(
                "UPDATE trades SET side = 'hedge' WHERE trade_id = 1; UPDATE book SET side = \
                 'mid' WHERE price = 100.0",
            )
            .unwrap();
            db.meta_set("schema_version", "1.7").unwrap();
        }

        let source = DuckdbSource::open(&path).expect("newer minor is compatible");
        let events: Vec<_> = source.read().unwrap().collect();
        assert_eq!(
            market_payloads(&events),
            [written[0].clone(), written[3].clone()],
            "both poisoned frames are skipped whole, the rest survive"
        );
        assert_eq!(
            events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            [1, 4],
            "surviving frames keep their original seqs"
        );
    }

    #[test]
    fn tolerated_vocabulary_retroactively_removes_the_frames_earlier_rows() {
        // The skip must poison rows decoded *before* the unknown value too:
        // trade_id 2 is the frame's second row, so its sibling was already
        // collected when the poison fires — a row-local skip would leak it
        // back as a partial frame.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let written = frames();
        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(&written).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();
        {
            let db = Db::open(&path).unwrap();
            db.execute_batch("UPDATE trades SET side = 'hedge' WHERE trade_id = 2")
                .unwrap();
            db.meta_set("schema_version", "1.7").unwrap();
        }

        let events: Vec<_> = DuckdbSource::open(&path).unwrap().read().unwrap().collect();
        assert_eq!(
            market_payloads(&events),
            [written[0].clone(), written[2].clone(), written[3].clone()],
            "the already-decoded first entry leaves with its frame"
        );
    }

    #[test]
    fn damaged_row_fails_loud_not_skipped() {
        // Same-MAJOR structural drift (a retyped column) is damage, not newer
        // vocabulary: the read must fail rather than return a shorter stream.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(&frames()).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();
        {
            let db = Db::open(&path).unwrap();
            db.execute_batch(
                "ALTER TABLE trades ALTER COLUMN price SET DATA TYPE VARCHAR; UPDATE trades SET \
                 price = 'oops'",
            )
            .unwrap();
        }

        match DuckdbSource::open(&path).unwrap().read() {
            Ok(_) => panic!("a damaged row must fail the read"),
            Err(e) => {
                assert!(matches!(e, Error::Damaged(_)), "got: {e}");
                assert!(e.to_string().contains("damaged"), "got: {e}");
            }
        }
    }

    #[test]
    fn backward_event_ts_is_clamped_to_capture_order() {
        // event_ts is not monotone in capture order: a candle without the
        // deprecated wire timestamp reports interval_begin, up to a whole
        // interval in the past. The clamp must keep the composer's (at,
        // track, seq) sort from reordering replay against capture order.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let late_candle = candle(None);
        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(&[frames(), vec![late_candle]].concat())
            .unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let events: Vec<_> = DuckdbSource::open(&path).unwrap().read().unwrap().collect();
        assert_eq!(events.len(), 5);
        assert!(
            events.windows(2).all(|w| w[0].seq < w[1].seq),
            "read order is capture order"
        );
        assert!(
            events
                .windows(2)
                .all(|w| (w[0].at, w[0].seq) <= (w[1].at, w[1].seq)),
            "the merge key must preserve capture order, got: {:?}",
            events.iter().map(|e| (e.seq, e.at)).collect::<Vec<_>>()
        );
        assert_eq!(
            events[4].at, events[3].at,
            "the candle's interval_begin clamps up to the previous frame's instant"
        );
    }

    #[test]
    fn open_is_rejected_while_a_recorder_is_live() {
        // The shared guard, not the engine, must reject: the advisory lock
        // fires before open_read_only, so the raw engine error never leaks.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let _recorder = DuckdbSink::open(&path, &meta()).unwrap();

        match DuckdbSource::open(&path) {
            Ok(_) => panic!("a live recorder must reject the read"),
            Err(e) => {
                assert!(matches!(e, Error::Rejected(_)), "got: {e}");
                assert!(e.to_string().contains("in use"), "got: {e}");
            }
        }
    }

    #[test]
    fn live_reader_rejects_a_starting_recorder() {
        // The reverse direction: a running read holds the lock shared, so a
        // recorder starting mid-read fails its exclusive acquire cleanly
        // instead of dying on the engine's file lock.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(&frames()).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();
        let _reader = DuckdbSource::open(&path).unwrap();

        match DuckdbSink::open(&path, &meta()) {
            Ok(_) => panic!("a live reader must reject a starting recorder"),
            Err(e) => {
                assert!(matches!(e, Error::Rejected(_)), "got: {e}");
                assert!(e.to_string().contains("in use"), "got: {e}");
            }
        }
    }

    #[test]
    fn concurrent_readers_do_not_exclude_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.duckdb");
        let mut sink = DuckdbSink::open(&path, &meta()).unwrap();
        sink.record(&frames()).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let first = DuckdbSource::open(&path).unwrap();
        let second = DuckdbSource::open(&path).unwrap();
        assert_eq!(
            market_payloads(&first.read().unwrap().collect::<Vec<_>>()),
            market_payloads(&second.read().unwrap().collect::<Vec<_>>())
        );
    }
}
