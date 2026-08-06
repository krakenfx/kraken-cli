//! A DuckDB-recorded session read whole through the public API — the
//! feature-gated sibling of `session_roundtrip.rs`.

use kraken_recording::{
    CaptureSink, DuckdbSink, RecordingDeclaration, RecordingIntegrity, Sink, schema,
};
use kraken_session::manifest::{RecordingBackend, RecordingRef};
use kraken_session::timeline::event::Track;
use kraken_session::timeline::{self, fixtures};

#[test]
fn duckdb_recording_reads_back_through_the_timeline() {
    let base = tempfile::tempdir().unwrap();
    let name = fixtures::provision(
        base.path(),
        vec![RecordingRef {
            backend: RecordingBackend::Duckdb,
            file: "market.duckdb".to_string(),
            schema_version: schema::SCHEMA_VERSION.to_string(),
            symbols: vec!["BTC/USD".to_string()],
            channels: vec!["trade".to_string()],
        }],
    );

    let frame = kraken_core::ChannelMessage::parse(
        r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","side":"buy",
           "price":1.0,"qty":1.0,"ord_type":"market","trade_id":0,
           "timestamp":"2026-01-01T00:00:01.000000Z"}]}"#,
    )
    .unwrap();
    let mut sink = DuckdbSink::open(
        &kraken_session::dir(base.path(), &name).join("market.duckdb"),
        &RecordingDeclaration {
            source: schema::SOURCE.to_string(),
            symbols: vec!["BTC/USD".into()],
            channels: vec!["trade".into()],
            window_start: "2026-01-01T00:00:00Z".parse().unwrap(),
            cli_version: "test".into(),
        },
    )
    .unwrap();
    sink.record(std::slice::from_ref(&frame)).unwrap();
    sink.finalize(RecordingIntegrity::now()).unwrap();

    let timeline = timeline::read(base.path(), &name).unwrap();
    assert_eq!(timeline.events.len(), 1);
    assert_eq!(timeline.events[0].track(), Track::Market);
    assert!(timeline.capture.is_some(), "capture manifest read back");
}
