//! The JSONL market-frame source — read pendant of
//! [`TapeSink`](crate::frames::jsonl::TapeSink). Lines carry no
//! `recv_ts`/`seq`, so the merge instant is the exchange `event_ts`, clamped
//! non-decreasing in line (capture) order. Lock-free; the composer holds the
//! file's [`FileLock`] for writer quiescence.

use crate::{
    error::{Error, Result},
    frames::{
        CaptureSource, MarketEvent, capture::RecordingManifest, clamp_non_decreasing,
        event_instant, shared_capture_lock,
    },
    jsonl::{committed_lines, damaged, line_seq},
    lock::FileLock,
    schema,
    source::Source,
};
use kraken_core::ChannelMessage;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct TapeSource {
    path: PathBuf,
    /// Loaded and version-gated at open, so no frame is ever decoded under
    /// the wrong contract; [`CaptureSource::capture`] hands it back.
    report: RecordingManifest,
    /// Held shared for the source's lifetime — the reader posture
    /// ([`shared_capture_lock`]), mirroring the DuckDB source.
    _lock: FileLock,
}

impl TapeSource {
    /// Fails with [`Rejected`](crate::Error::Rejected) when the tape is
    /// missing, a writer still holds it, or its sidecar stamp is absent or
    /// a different schema MAJOR — the same up-front contract as the
    /// feature-gated `DuckdbSource::open`.
    pub fn open(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(Error::Rejected(format!(
                "tape '{}' not found",
                path.display()
            )));
        }
        let lock = shared_capture_lock(path)?;
        let report = crate::frames::capture::load_sidecar(path, "read")?;
        Ok(Self {
            path: path.to_path_buf(),
            report,
            _lock: lock,
        })
    }

    /// The damage-vs-drift decision for a committed line that failed to
    /// decode. `Ok(())` guarantees the line is tolerable drift — the
    /// capture's stamp is a *newer minor* than this build, the one case
    /// where the content can be a newer writer's vocabulary — warned here
    /// and safe for the caller to skip. Anything else is damage: `Err`,
    /// fails loud.
    fn ensure_tolerable_drift(
        &self,
        line_no: usize,
        raw: &[u8],
        err: &serde_json::Error,
    ) -> Result<()> {
        let Ok(value) = serde_json::from_slice::<Value>(raw) else {
            return Err(damaged(
                &self.path,
                line_no,
                &format!("malformed json: {err}"),
            ));
        };
        if value.get("channel").and_then(Value::as_str).is_none() {
            // Valid JSON without a channel key is nothing any sink ever
            // writes — damage, not a newer writer's vocabulary.
            return Err(damaged(&self.path, line_no, "no channel key"));
        }
        if !schema::is_newer_minor(&self.report.schema_version) {
            return Err(damaged(&self.path, line_no, "undecodable frame"));
        }
        tracing::warn!(
            path = %self.path.display(),
            line = line_no,
            "skipping a newer-minor writer's frame outside this build's channel catalogue"
        );
        Ok(())
    }
}

impl Source for TapeSource {
    type Item = MarketEvent;

    /// A committed line that fails to decode is damage and fails loud (the
    /// DuckDB backend's contract too) — unless the capture's stamp is a
    /// *newer minor* than this build, the one case where the content can be
    /// a newer writer's vocabulary, warned and skipped. The merge instant is
    /// the exchange `event_ts`, clamped non-decreasing in line (capture)
    /// order; `seq` is the physical line number.
    fn read(&self) -> Result<impl Iterator<Item = MarketEvent>> {
        let mut events = Vec::new();
        for line in committed_lines(&self.path)? {
            let (line_no, raw) = line?;
            let frame = match serde_json::from_slice::<ChannelMessage>(&raw) {
                Ok(frame) => frame,
                // The happy path decoded straight to the frame; only a failure
                // pays for the probe that classifies damage vs drift.
                Err(err) => {
                    self.ensure_tolerable_drift(line_no, &raw, &err)?;
                    continue;
                }
            };
            // A decodable frame without a per-entry event time (a reference or
            // account channel) is equally nothing the sink ever writes.
            let Some(at) = event_instant(&frame) else {
                return Err(damaged(&self.path, line_no, "no parseable event timestamp"));
            };
            events.push(MarketEvent {
                at,
                seq: line_seq(line_no),
                frame,
            });
        }
        clamp_non_decreasing(&mut events);
        Ok(events.into_iter())
    }
}

impl CaptureSource for TapeSource {
    /// The report stamped at open and completed at finalize, already loaded
    /// and version-gated by [`TapeSource::open`].
    fn capture(&self) -> Result<RecordingManifest> {
        Ok(self.report.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        frames::{
            CaptureSink,
            capture::{RecordingDeclaration, RecordingIntegrity, sidecar_path},
            jsonl::TapeSink,
        },
        sink::Sink,
    };
    use kraken_core::ChannelData;

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

    /// Stamp a sidecar for a hand-written tape file, so `open`'s version
    /// gate passes (or observes `version`, for the drift tests).
    fn stamp(path: &Path, version: &str) {
        let report = RecordingManifest {
            schema_version: version.to_string(),
            meta: meta(),
            summary: None,
        };
        crate::frames::capture::write_json_atomic(&sidecar_path(path), &report).unwrap();
    }

    fn trade_frame(ts: &str, id: u64) -> ChannelMessage {
        ChannelMessage::parse(&format!(
            r#"{{"channel":"trade","type":"update","data":[{{"symbol":"BTC/USD","side":"buy",
               "price":50000.1,"qty":0.5,"ord_type":"market","trade_id":{id},
               "timestamp":"{ts}"}}]}}"#
        ))
        .expect("trade frame parses")
    }

    fn frames_of(events: &[MarketEvent]) -> Vec<&ChannelMessage> {
        events.iter().map(|e| &e.frame).collect()
    }

    #[test]
    fn all_channels_round_trip_in_event_ts_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.jsonl");
        let written: Vec<ChannelMessage> = [
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
               "bids":[{"price":100.0,"qty":1.0}],"asks":[{"price":100.5,"qty":3.0}],
               "checksum":4242,"timestamp":"2026-01-01T00:00:03.000000Z"}]}"#,
            r#"{"channel":"ohlc","type":"update","data":[{"symbol":"BTC/USD",
               "open":1.0,"high":2.0,"low":0.5,"close":1.5,"vwap":1.2,"trades":7,"volume":123.0,
               "interval_begin":"2026-01-01T00:00:00.000000000Z","interval":1,
               "timestamp":"2026-01-01T00:00:04.000000Z"}]}"#,
        ]
        .iter()
        .map(|raw| ChannelMessage::parse(raw).expect("fixture frame parses"))
        .collect();

        let mut sink = TapeSink::open(&path, &meta()).unwrap();
        sink.record(&written).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let events: Vec<_> = TapeSource::open(&path)
            .expect("tape exists")
            .read()
            .unwrap()
            .collect();
        assert_eq!(
            events.iter().map(|e| e.frame.clone()).collect::<Vec<_>>(),
            written
        );
        assert!(
            events
                .windows(2)
                .all(|w| (w[0].at, w[0].seq) < (w[1].at, w[1].seq))
        );
    }

    #[test]
    fn ohlc_interval_begin_fallback_does_not_reorder_the_capture() {
        // A candle without the wire `timestamp` reports interval_begin, up
        // to a whole interval before receipt; the clamp keeps line order.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.jsonl");
        let trade = trade_frame("2026-01-01T00:00:30.000000Z", 1);
        let candle = r#"{"channel":"ohlc","type":"update","data":[{"symbol":"BTC/USD",
           "open":1.0,"high":2.0,"low":0.5,"close":1.5,"vwap":1.2,"trades":7,"volume":123.0,
           "interval_begin":"2026-01-01T00:00:00.000000000Z","interval":1}]}"#;
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&trade).unwrap(),
                ChannelMessage::parse(candle)
                    .map(|f| serde_json::to_string(&f).unwrap())
                    .unwrap()
            ),
        )
        .unwrap();
        stamp(&path, schema::SCHEMA_VERSION);

        let events: Vec<_> = TapeSource::open(&path).unwrap().read().unwrap().collect();
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[1].frame.body, ChannelData::Ohlc(_)),
            "file order preserved"
        );
        assert!(
            (events[0].at, events[0].seq) < (events[1].at, events[1].seq),
            "the candle's clamped key sorts after the earlier trade"
        );
    }

    #[test]
    fn unknown_channel_line_under_a_newer_minor_stamp_is_skipped() {
        // Only a newer-minor writer's capture may carry vocabulary this build
        // lacks; there the skip is drift tolerance, not swallowed damage.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.jsonl");
        let keep = trade_frame("2026-01-01T00:00:01.000000Z", 1);
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                r#"{"channel":"margin_rates","type":"update","data":[{"rate":0.01}]}"#,
                serde_json::to_string(&keep).unwrap()
            ),
        )
        .unwrap();
        stamp(&path, "1.7");

        let events: Vec<_> = TapeSource::open(&path).unwrap().read().unwrap().collect();
        assert_eq!(frames_of(&events), [&keep]);
    }

    #[test]
    fn unknown_channel_line_under_this_builds_stamp_is_damage() {
        // The same line under this build's own stamp cannot be newer
        // vocabulary — it is a corrupted committed line and must fail loud,
        // exactly like the DuckDB backend.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                r#"{"channel":"margin_rates","type":"update","data":[{"rate":0.01}]}"#,
                serde_json::to_string(&trade_frame("2026-01-01T00:00:01.000000Z", 1)).unwrap()
            ),
        )
        .unwrap();
        stamp(&path, schema::SCHEMA_VERSION);

        let err = match TapeSource::open(&path).unwrap().read() {
            Ok(_) => panic!("a damaged recording must be rejected"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::Damaged(_)));
    }

    #[test]
    fn damaged_committed_line_fails_loud_not_skipped() {
        // The JSONL twin of the DuckDB damaged-row test: valid JSON whose
        // payload no longer decodes ("price":"oops") is damage under a
        // same-version stamp, never a skippable diagnostic.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","side":"buy","#,
                r#""price":"oops","qty":0.5,"ord_type":"market","trade_id":1,"#,
                r#""timestamp":"2026-01-01T00:00:01.000000Z"}]}"#,
                "\n"
            ),
        )
        .unwrap();
        stamp(&path, schema::SCHEMA_VERSION);

        let err = match TapeSource::open(&path).unwrap().read() {
            Ok(_) => panic!("a damaged recording must be rejected"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::Damaged(_)), "got: {err}");
    }

    #[test]
    fn damage_reports_the_physical_line_number_across_blank_lines() {
        // A blank line is skipped but still counted, so the report names the
        // on-disk line — the number a text editor or `sed -n` agrees with.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\n\nnot json\n",
                serde_json::to_string(&trade_frame("2026-01-01T00:00:01.000000Z", 1)).unwrap()
            ),
        )
        .unwrap();
        stamp(&path, schema::SCHEMA_VERSION);

        let err = match TapeSource::open(&path).unwrap().read() {
            Ok(_) => panic!("a damaged recording must be rejected"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("line 3"), "got: {err}");
    }

    #[test]
    fn seq_is_the_physical_line_number() {
        // Blank lines never come from the sink, but a hand-edited log must
        // still yield seqs that match the on-disk line numbers.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\n\n{}\n",
                serde_json::to_string(&trade_frame("2026-01-01T00:00:01.000000Z", 1)).unwrap(),
                serde_json::to_string(&trade_frame("2026-01-01T00:00:02.000000Z", 2)).unwrap()
            ),
        )
        .unwrap();
        stamp(&path, schema::SCHEMA_VERSION);

        let events: Vec<_> = TapeSource::open(&path).unwrap().read().unwrap().collect();
        assert_eq!(
            events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            [1, 3],
            "seq skips the blank physical line"
        );
    }

    #[test]
    fn frame_without_a_parseable_event_ts_fails_loud() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.jsonl");
        let damaged = trade_frame("not-a-timestamp", 1);
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string(&damaged).unwrap()),
        )
        .unwrap();
        stamp(&path, schema::SCHEMA_VERSION);

        let err = match TapeSource::open(&path).unwrap().read() {
            Ok(_) => panic!("a damaged recording must be rejected"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::Damaged(_)));
    }

    /// Verbatim on-disk line pinning the read/write contract; bump
    /// [`schema::SCHEMA_VERSION`] deliberately instead of editing the pin.
    const PINNED_MARKET_LINE: &str = r#"{"channel":"trade","data":[{"symbol":"BTC/USD","side":"buy","price":50000.1,"qty":0.5,"ord_type":"market","trade_id":1,"timestamp":"2026-01-01T00:00:00.000000Z"}],"type":"update"}"#;

    #[test]
    fn pinned_market_line_still_decodes() {
        let frame = ChannelMessage::parse(PINNED_MARKET_LINE).expect("pinned line decodes");
        assert!(
            matches!(
                &frame.body,
                ChannelData::Trade(data) if data[0].price == "50000.1".parse().unwrap()
            ),
            "pinned line must decode to its typed variant, got {frame:?}"
        );
    }

    #[test]
    fn writer_output_still_matches_the_pinned_market_line() {
        let frame = ChannelMessage::parse(PINNED_MARKET_LINE).expect("pinned line decodes");
        assert_eq!(serde_json::to_string(&frame).unwrap(), PINNED_MARKET_LINE);
    }

    #[test]
    fn capture_round_trips_the_report_the_sink_stamped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.jsonl");
        let summary = RecordingIntegrity {
            window_end: "2026-01-01T00:01:00Z".parse().unwrap(),
            events_dropped: 7,
            frames_unparsed: 3,
            reconnect_count: 2,
        };
        let sink = TapeSink::open(&path, &meta()).unwrap();
        sink.finalize(summary.clone()).unwrap();

        let report = TapeSource::open(&path).unwrap().capture().unwrap();
        assert_eq!(report.schema_version, schema::SCHEMA_VERSION);
        assert_eq!(report.meta, meta());
        assert_eq!(report.summary, Some(summary));
    }

    #[test]
    fn crashed_capture_reads_as_unfinalized_but_version_checked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.jsonl");
        drop(TapeSink::open(&path, &meta()).unwrap()); // no finalize

        let report = TapeSource::open(&path).unwrap().capture().unwrap();
        assert_eq!(report.summary, None, "unfinalized: completeness unknown");
        assert_eq!(report.schema_version, schema::SCHEMA_VERSION);
        assert_eq!(report.meta, meta());
    }

    #[test]
    fn missing_sidecar_is_rejected_at_open_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.jsonl");
        drop(TapeSink::open(&path, &meta()).unwrap());
        std::fs::remove_file(sidecar_path(&path)).unwrap();

        // The gate fires at open — before any read — like the DuckDB source.
        let err = match TapeSource::open(&path) {
            Ok(_) => panic!("a sidecar-less tape must be rejected"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::Rejected(_)));
        assert!(
            err.to_string().contains("no metadata sidecar"),
            "got: {err}"
        );
    }

    #[test]
    fn corrupt_sidecar_fails_loud_at_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.jsonl");
        drop(TapeSink::open(&path, &meta()).unwrap());
        std::fs::write(sidecar_path(&path), "{ torn").unwrap();

        let err = match TapeSource::open(&path) {
            Ok(_) => panic!("a corrupt sidecar must be rejected"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::Damaged(_)));
    }

    #[test]
    fn half_stamped_sidecar_is_damage_not_unfinalized() {
        // The DuckDB `_meta` twin: a sidecar carrying `window_end` without
        // its loss counts is a torn or hand-edited stamp. Reading it as
        // merely unfinalized would silently discard the completeness signal
        // — and classify the same state differently per backend.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.jsonl");
        let sink = TapeSink::open(&path, &meta()).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();
        let sidecar = sidecar_path(&path);
        let doctored: String = std::fs::read_to_string(&sidecar)
            .unwrap()
            .lines()
            .filter(|line| !line.contains("events_dropped"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&sidecar, doctored).unwrap();

        let err = match TapeSource::open(&path) {
            Ok(_) => panic!("a half-stamped sidecar must be rejected"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::Damaged(_)), "got: {err}");
        // The message pins WHICH rejection fired: were the doctoring to break
        // the JSON instead, the parse error would also be Damaged and the
        // test would pass for the wrong reason.
        assert!(
            err.to_string().contains("partial completeness stamp"),
            "got: {err}"
        );
    }

    #[test]
    fn incompatible_sidecar_schema_major_is_rejected_at_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("market.jsonl");
        let sink = TapeSink::open(&path, &meta()).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();
        let sidecar = sidecar_path(&path);
        let doctored = std::fs::read_to_string(&sidecar)
            .unwrap()
            .replace(schema::SCHEMA_VERSION, "2.0");
        std::fs::write(&sidecar, doctored).unwrap();

        let err = match TapeSource::open(&path) {
            Ok(_) => panic!("a different-major tape must be rejected"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::Rejected(_)));
    }
}
