//! End-to-end tests over the crate's public surface: the full capture
//! lifecycle a real recorder drives — validate channels, resolve the tape
//! path, record batches, finalize, replay — plus the cross-cutting behaviours
//! no single module owns: crash-and-resume, torn-tail repair, writer/reader
//! exclusion, and (behind `duckdb`) backend equivalence across sessions.

// Integration tests compile with `--test` (so `cfg(test)` holds); same
// carve-out as lib.rs, extended to helpers outside `#[test]` functions.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use kraken_core::{
    ChannelData, ChannelMessage, OrderSide, OrderType, SubscribableChannel,
    subscribe::message::{
        BookData, ExtraFields, MessageType, OhlcData, PriceLevel, StatusData, SystemStatus,
        TickerData, TradeData,
    },
};
use kraken_recording::{
    CaptureSink, CaptureSource, Error, MarketEvent, RecordingDeclaration, RecordingIntegrity, Sink,
    Source, TapeSink, TapeSource, ensure_recordable, is_lock_held, resolve, sanitize_id, schema,
};
use std::{io::Write, path::Path, str::FromStr};

/// Parse a numeric fixture literal into the field's own inferred type —
/// `Decimal` at every call site, without a direct `rust_decimal`
/// dev-dependency.
fn dec<T: FromStr>(raw: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    raw.parse().expect("fixture literal parses")
}

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
        cli_version: "e2e".into(),
    }
}

/// The server pushes `status` unsolicited on every connection; a capture
/// must drop it without disturbing the frame numbering.
fn status_frame() -> ChannelMessage {
    ChannelMessage {
        body: ChannelData::Status(vec![StatusData {
            api_version: "v2".into(),
            connection_id: 1,
            system: SystemStatus::Online,
            version: "2.0".into(),
            extra: ExtraFields::new(),
        }]),
        message_type: MessageType::Update,
        sequence: None,
    }
}

/// One frame per recordable channel inside minute `minute` (single digit),
/// timestamps strictly increasing — including the shapes that stress the
/// read-back inverse: a two-entry trade frame and a two-sided book snapshot.
fn market_frames(minute: u8) -> Vec<ChannelMessage> {
    let ts = |sec: u8| format!("2026-01-01T00:0{minute}:0{sec}.000000Z");
    let id = u64::from(minute) * 10;
    let frame = |message_type: MessageType, body: ChannelData| ChannelMessage {
        body,
        message_type,
        sequence: None,
    };
    vec![
        frame(
            MessageType::Update,
            ChannelData::Ticker(vec![TickerData {
                symbol: "BTC/USD".into(),
                bid: dec("68000.1"),
                bid_qty: dec("0.5"),
                ask: dec("68000.2"),
                ask_qty: dec("0.25"),
                last: dec("68000.15"),
                volume: dec("1234.5"),
                vwap: dec("67990.4"),
                low: dec("67000.0"),
                high: dec("69000.0"),
                change: dec("-120.5"),
                change_pct: dec("-0.18"),
                timestamp: ts(1),
            }]),
        ),
        frame(
            MessageType::Update,
            ChannelData::Trade(vec![
                TradeData {
                    symbol: "BTC/USD".into(),
                    side: OrderSide::Buy,
                    price: dec("68000.1"),
                    qty: dec("0.5"),
                    ord_type: OrderType::Market,
                    trade_id: id + 1,
                    timestamp: ts(2),
                },
                TradeData {
                    symbol: "BTC/USD".into(),
                    side: OrderSide::Sell,
                    price: dec("68000.2"),
                    qty: dec("0.25"),
                    ord_type: OrderType::Limit,
                    trade_id: id + 2,
                    timestamp: ts(3),
                },
            ]),
        ),
        frame(
            MessageType::Snapshot,
            ChannelData::Book(vec![BookData {
                symbol: "BTC/USD".into(),
                bids: vec![
                    PriceLevel {
                        price: dec("68000.0"),
                        qty: dec("1.0"),
                    },
                    PriceLevel {
                        price: dec("67999.5"),
                        qty: dec("2.0"),
                    },
                ],
                asks: vec![PriceLevel {
                    price: dec("68000.5"),
                    qty: dec("3.0"),
                }],
                checksum: 4242,
                timestamp: ts(4),
            }]),
        ),
        frame(
            MessageType::Update,
            ChannelData::Ohlc(vec![OhlcData {
                symbol: "BTC/USD".into(),
                open: dec("68000.0"),
                high: dec("68001.0"),
                low: dec("67999.0"),
                close: dec("68000.5"),
                vwap: dec("68000.2"),
                trades: 7,
                volume: dec("123.0"),
                interval_begin: format!("2026-01-01T00:0{minute}:00.000000000Z"),
                interval: 1,
                timestamp: Some(ts(5)),
            }]),
        ),
    ]
}

fn payloads(events: &[MarketEvent]) -> Vec<ChannelMessage> {
    events.iter().map(|e| e.frame.clone()).collect()
}

/// Replay's ordering contract: the merge key `(at, seq)` is strictly
/// increasing, so a consumer's sort can never reorder replay against capture.
fn assert_replay_ordered(events: &[MarketEvent]) {
    assert!(
        events
            .windows(2)
            .all(|w| (w[0].at, w[0].seq) < (w[1].at, w[1].seq)),
        "replay must be strictly (at, seq)-ordered, got: {:?}",
        events.iter().map(|e| (e.at, e.seq)).collect::<Vec<_>>()
    );
}

/// `TapeSource` has no Debug impl (unwrap is unavailable): open or panic.
fn open_reader(path: &Path) -> TapeSource {
    match TapeSource::open(path) {
        Ok(source) => source,
        Err(e) => panic!("reader must open: {e}"),
    }
}

/// The lifecycle contract every backend must honour, driven the way the
/// production recorder drives it — generic over the concrete sink and source,
/// exactly as the crate's drivers are: record batches (the wire interleaves
/// an unsolicited status frame), finalize with a lossy summary, then replay
/// the events and the capture's self-description.
fn assert_capture_lifecycle<W, R>(
    path: &Path,
    open_sink: impl FnOnce(&Path, &RecordingDeclaration) -> kraken_recording::Result<W>,
    open_source: impl FnOnce(&Path) -> kraken_recording::Result<R>,
) where
    W: CaptureSink,
    R: CaptureSource,
{
    let written = market_frames(0);
    let mut sink = open_sink(path, &meta()).unwrap_or_else(|e| panic!("sink opens: {e}"));
    sink.record(&[vec![status_frame()], written[..2].to_vec()].concat())
        .unwrap();
    sink.record(&written[2..]).unwrap();
    let summary = RecordingIntegrity {
        window_end: "2026-01-01T00:01:00Z".parse().unwrap(),
        events_dropped: 1,
        frames_unparsed: 2,
        reconnect_count: 3,
    };
    sink.finalize(summary.clone()).unwrap();

    let source = open_source(path).unwrap_or_else(|e| panic!("source opens: {e}"));
    let report = source.capture().unwrap();
    assert_eq!(report.schema_version, schema::SCHEMA_VERSION);
    assert_eq!(report.meta, meta());
    assert_eq!(report.summary, Some(summary));

    let events: Vec<MarketEvent> = source.read().unwrap().collect();
    assert_eq!(
        payloads(&events),
        written,
        "the status frame never becomes an event"
    );
    assert_replay_ordered(&events);
}

#[test]
fn capture_lifecycle_round_trips_frames_and_report() {
    let dir = tempfile::tempdir().unwrap();
    // The production prelude: validate the requested channels, then derive
    // the tape path from the symbol.
    let channels: Vec<SubscribableChannel> = meta()
        .channels
        .iter()
        .map(|name| name.parse().expect("fixture channel parses"))
        .collect();
    ensure_recordable(&channels).unwrap();
    let path = resolve(dir.path(), &sanitize_id("BTC/USD"), "jsonl");

    assert_capture_lifecycle(&path, TapeSink::open, TapeSource::open);
}

#[test]
fn crash_reads_unfinalized_then_resume_completes_the_capture() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("BTC-USD.jsonl");
    let first_run = market_frames(0);
    let mut sink = TapeSink::open(&path, &meta()).unwrap();
    sink.record(&first_run).unwrap();
    drop(sink); // crash: no finalize

    {
        let source = open_reader(&path);
        assert_eq!(
            source.capture().unwrap().summary,
            None,
            "completeness unknown after a crash"
        );
        assert_eq!(
            payloads(&source.read().unwrap().collect::<Vec<_>>()),
            first_run,
            "every committed frame survives the crash"
        );
    } // the reader's shared lock releases before the resume

    let second_run = market_frames(1);
    let mut resumed = TapeSink::open(&path, &meta()).unwrap();
    resumed.record(&second_run).unwrap();
    let summary = RecordingIntegrity {
        window_end: "2026-01-01T00:02:00Z".parse().unwrap(),
        reconnect_count: 1,
        ..RecordingIntegrity::now()
    };
    resumed.finalize(summary.clone()).unwrap();

    let source = open_reader(&path);
    assert_eq!(
        source.capture().unwrap().summary,
        Some(summary),
        "the stamp describes the resumed capture"
    );
    let events: Vec<MarketEvent> = source.read().unwrap().collect();
    assert_eq!(payloads(&events), [first_run, second_run].concat());
    assert_replay_ordered(&events);
}

#[test]
fn torn_tail_from_a_crash_never_becomes_a_frame_and_repair_resumes_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("BTC-USD.jsonl");
    let committed = market_frames(0);
    let mut sink = TapeSink::open(&path, &meta()).unwrap();
    sink.record(&committed).unwrap();
    drop(sink);
    // A crash mid-append: bytes after the last newline, no commit marker.
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(br#"{"channel":"trade","type":"upd"#)
        .unwrap();
    drop(file);

    {
        let source = open_reader(&path);
        assert_eq!(
            payloads(&source.read().unwrap().collect::<Vec<_>>()),
            committed,
            "the torn tail is a crash artifact, not damage"
        );
    }

    let tail = market_frames(1);
    let mut resumed = TapeSink::open(&path, &meta()).unwrap();
    resumed.record(&tail).unwrap(); // the append repairs the torn tail first
    resumed.finalize(RecordingIntegrity::now()).unwrap();

    let events: Vec<MarketEvent> = open_reader(&path).read().unwrap().collect();
    assert_eq!(payloads(&events), [committed, tail].concat());
    assert_eq!(
        events.iter().map(|e| e.seq).collect::<Vec<_>>(),
        (1i64..=8).collect::<Vec<_>>(),
        "the repaired tail leaves no phantom line behind"
    );
}

#[test]
fn live_recorder_excludes_writers_and_readers_until_it_releases() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("BTC-USD.jsonl");
    let mut sink = TapeSink::open(&path, &meta()).unwrap();
    sink.record(&market_frames(0)).unwrap();
    assert!(
        is_lock_held(&path),
        "a live recorder reads as held to the liveness probe"
    );

    let err = TapeSink::open(&path, &meta()).unwrap_err();
    assert!(matches!(err, Error::Rejected(_)), "got: {err}");
    let err = match TapeSource::open(&path) {
        Ok(_) => panic!("a live recorder must reject readers"),
        Err(e) => e,
    };
    assert!(matches!(err, Error::Rejected(_)), "got: {err}");
    assert!(err.to_string().contains("in use"), "got: {err}");

    sink.finalize(RecordingIntegrity::now()).unwrap();
    assert!(!is_lock_held(&path), "finalize releases the writer lock");

    let first_reader = open_reader(&path);
    let second_reader = open_reader(&path);
    assert!(
        !is_lock_held(&path),
        "readers stay invisible to the recorder-liveness probe"
    );
    let err = TapeSink::open(&path, &meta()).unwrap_err();
    assert!(
        matches!(err, Error::Rejected(_)),
        "a live reader rejects a starting recorder, got: {err}"
    );
    drop(first_reader);
    drop(second_reader);
    drop(TapeSink::open(&path, &meta()).expect("the released tape records again"));
}

#[test]
fn an_empty_capture_finalizes_and_replays_to_zero_events() {
    // A session can end before its first market frame; the tape must
    // still self-describe as complete and replay as an empty stream.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("BTC-USD.jsonl");
    let sink = TapeSink::open(&path, &meta()).unwrap();
    sink.finalize(RecordingIntegrity::now()).unwrap();

    let source = open_reader(&path);
    assert!(source.capture().unwrap().summary.is_some());
    assert_eq!(source.read().unwrap().count(), 0);
}

#[cfg(feature = "duckdb")]
mod duckdb_backend {
    use super::*;
    use kraken_recording::{DuckdbSink, DuckdbSource};

    /// `DuckdbSource` has no Debug impl (unwrap is unavailable): open or panic.
    fn open_duckdb_reader(path: &Path) -> DuckdbSource {
        match DuckdbSource::open(path) {
            Ok(source) => source,
            Err(e) => panic!("reader must open: {e}"),
        }
    }

    #[test]
    fn capture_lifecycle_round_trips_frames_and_report() {
        // The jsonl lifecycle's columnar mirror, through the public surface
        // only — no raw SQL.
        let dir = tempfile::tempdir().unwrap();
        let path = resolve(dir.path(), &sanitize_id("BTC/USD"), "duckdb");

        assert_capture_lifecycle(&path, DuckdbSink::open, DuckdbSource::open);
    }

    #[test]
    fn backends_agree_across_a_crash_and_resume() {
        // Frame numbering continues differently per backend (physical line
        // numbers vs `_meta.last_seq`), so their equivalence across a crashed
        // and resumed capture is an invariant neither backend can pin alone.
        let dir = tempfile::tempdir().unwrap();
        let duck_path = dir.path().join("market.duckdb");
        let jsonl_path = dir.path().join("market.jsonl");
        let first_run: Vec<ChannelMessage> = [vec![status_frame()], market_frames(0)].concat();
        let second_run = market_frames(1);
        let summary = RecordingIntegrity {
            window_end: "2026-01-01T00:02:00Z".parse().unwrap(),
            events_dropped: 2,
            frames_unparsed: 1,
            reconnect_count: 1,
        };

        let mut duck = DuckdbSink::open(&duck_path, &meta()).unwrap();
        duck.record(&first_run).unwrap();
        drop(duck); // crash: no finalize
        let mut duck = DuckdbSink::open(&duck_path, &meta()).unwrap();
        duck.record(&second_run).unwrap();
        duck.finalize(summary.clone()).unwrap();

        let mut jsonl = TapeSink::open(&jsonl_path, &meta()).unwrap();
        jsonl.record(&first_run).unwrap();
        drop(jsonl); // crash: no finalize
        let mut jsonl = TapeSink::open(&jsonl_path, &meta()).unwrap();
        jsonl.record(&second_run).unwrap();
        jsonl.finalize(summary).unwrap();

        let duck_source = open_duckdb_reader(&duck_path);
        let jsonl_source = open_reader(&jsonl_path);
        let from_duck: Vec<MarketEvent> = duck_source.read().unwrap().collect();
        let from_jsonl: Vec<MarketEvent> = jsonl_source.read().unwrap().collect();
        assert_eq!(payloads(&from_duck), payloads(&from_jsonl));
        assert_eq!(
            from_duck.iter().map(|e| (e.at, e.seq)).collect::<Vec<_>>(),
            from_jsonl.iter().map(|e| (e.at, e.seq)).collect::<Vec<_>>(),
            "one merge-key sequence regardless of backend, across the crash"
        );
        assert_eq!(
            duck_source.capture().unwrap(),
            jsonl_source.capture().unwrap(),
            "one capture self-description regardless of backend"
        );
    }
}
