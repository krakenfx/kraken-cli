//! The JSONL market-frame sink (the DuckDB counterpart is the feature-gated
//! `DuckdbSink`).
//!
//! Writes each captured frame as one JSONL line to `<tape>.jsonl`, plus a
//! `<tape>.jsonl.meta.json` sidecar: identity and schema version stamped
//! at open (so a crashed run stays version-checkable), the lag summary added
//! at finalize. Dependency-free, so it is always available and doubles as the
//! DuckDB sink's content oracle in tests. Reopen appends, matching DuckDB.
//!
//! Composes the shared [`Log`] engine and adds only what is market-specific:
//! the recordable-frame filter and the metadata sidecar. The domain-free
//! sibling is [`JsonlSink`](crate::sink::JsonlSink).

use std::path::{Path, PathBuf};

use kraken_core::ChannelMessage;

use crate::error::Result;
use crate::frames::CaptureSink;
use crate::frames::capture::{
    RecordingDeclaration, RecordingFilter, RecordingIntegrity, RecordingManifest, sidecar_path,
};
use crate::jsonl::Log;
use crate::schema;
use crate::sink::Sink;

#[derive(Debug)]
pub struct TapeSink {
    log: Log,
    meta_path: PathBuf,
    /// Carried to finalize: the open-time stamp (first session's version,
    /// absorbed identity) with `summary: None`.
    report: RecordingManifest,
    /// The shared per-frame capture gate ([`RecordingFilter`]).
    gate: RecordingFilter,
}

impl TapeSink {
    pub fn open(path: &Path, meta: &RecordingDeclaration) -> Result<Self> {
        // `open` takes the single-writer lock up front, so a second recorder
        // is rejected before it can interleave the tape — and holds it for
        // the sink's lifetime, which is what `playground` probes for liveness.
        let log = Log::open(path.to_path_buf(), format!("tape '{}'", path.display()))?;
        // Probed under the writer lock (a probe before it could go stale
        // against a concurrent bootstrap and silently rewrite the sidecar).
        // `open` is append-only — it never truncates — so the length still
        // reads the pre-open state. Zero committed bytes means zero recorded
        // frames, whatever the sidecar says: brand new, the artifact of a
        // crash before the first append, or a log deleted by hand next to a
        // surviving sidecar. Bootstrapping fresh in every one of those keeps
        // an orphaned sidecar's stale identity and version stamp from being
        // adopted by a new capture. (JSONL-only hazard: the sidecar and log
        // are separable files; DuckDB's `_meta` shares the data's file and
        // cannot be orphaned, so its structural is-new probe needs no twin.)
        let is_new = std::fs::metadata(path)?.len() == 0;
        let meta_path = sidecar_path(path);
        // Reopen mirrors the DuckDB `_meta` contract: keep the first session's
        // version stamp, absorb this session's identity.
        let report = if is_new {
            RecordingManifest {
                schema_version: schema::SCHEMA_VERSION.to_string(),
                meta: meta.clone(),
                summary: None,
            }
        } else {
            let mut existing = crate::frames::capture::load_sidecar(path, "append")?;
            existing.meta.absorb(meta);
            // A reopen restarts the capture: clear the old summary so a crash
            // mid-capture reads back as unfinalized, not as the old session's stamp.
            existing.summary = None;
            existing
        };
        crate::frames::capture::write_json_atomic(&meta_path, &report)?;
        Ok(Self {
            log,
            meta_path,
            report,
            gate: RecordingFilter::new(crate::frames::capture::declared_channels(meta)),
        })
    }
}

impl Sink for TapeSink {
    type Item = ChannelMessage;

    fn record(&mut self, batch: &[ChannelMessage]) -> Result<()> {
        let gate = &mut self.gate;
        let frames: Vec<&ChannelMessage> =
            batch.iter().filter(|frame| gate.admits(frame)).collect();
        self.log.append(&frames)
    }
}

impl CaptureSink for TapeSink {
    /// Written atomically: the summary's presence is the "finalized" marker,
    /// so a torn half-write must never be observable.
    fn finalize(self, summary: RecordingIntegrity) -> Result<()> {
        let report = RecordingManifest {
            summary: Some(summary),
            ..self.report
        };
        crate::frames::capture::write_json_atomic(&self.meta_path, &report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    fn meta() -> crate::frames::capture::RecordingDeclaration {
        crate::frames::capture::RecordingDeclaration {
            source: schema::SOURCE.to_string(),
            symbols: vec!["BTC/USD".into()],
            channels: vec!["trades".into()],
            window_start: "2026-01-01T00:00:00Z".parse().unwrap(),
            cli_version: "test".into(),
        }
    }

    #[test]
    fn second_open_of_same_tape_is_rejected() {
        // While one sink holds the tape, a second open fails with
        // `validation` rather than letting two writers interleave one file.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.jsonl");
        let _first = TapeSink::open(&path, &meta()).unwrap();
        let err = TapeSink::open(&path, &meta()).unwrap_err();
        assert!(matches!(err, crate::error::Error::Rejected(_)));
    }

    fn trade_frame() -> ChannelMessage {
        ChannelMessage::parse(
            r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","side":"buy",
               "price":1.0,"qty":1.0,"ord_type":"market","trade_id":1,"timestamp":"TS"}]}"#,
        )
        .expect("trade frame parses")
    }

    #[test]
    fn records_frames_and_writes_meta_sidecar() {
        // Capture-and-read for the jsonl backend (the DuckDB content oracle): a frame
        // round-trips via ChannelMessage, and finalize writes the metadata sidecar.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.jsonl");
        let frame = ChannelMessage::parse(
            r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","side":"buy",
               "price":50000.1,"qty":0.5,"ord_type":"market","trade_id":1,
               "timestamp":"2026-01-01T00:00:00.000000Z"}]}"#,
        )
        .unwrap();

        let mut sink = TapeSink::open(&path, &meta()).unwrap();
        sink.record(std::slice::from_ref(&frame)).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 1);
        assert_eq!(ChannelMessage::parse(lines[0]).unwrap(), frame);

        let sidecar = std::fs::read_to_string(sidecar_path(&path)).unwrap();
        assert!(sidecar.contains("\"schema_version\""));
        assert!(sidecar.contains("BTC/USD"));
        // The completeness markers are always present (0 when clean).
        assert!(sidecar.contains("\"events_dropped\""));
        assert!(sidecar.contains("\"frames_unparsed\""));
        assert!(sidecar.contains("\"reconnect_count\""));
    }

    #[test]
    fn skips_non_recordable_status_frames() {
        // The server pushes `status` unsolicited; the jsonl backend must drop it just
        // like duckdb, so both backends capture the same channels.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.jsonl");
        let status = ChannelMessage::parse(
            r#"{"channel":"status","type":"update","data":[{"system":"online",
               "api_version":"v2","connection_id":1,"version":"2.0"}]}"#,
        )
        .unwrap();
        let trade = trade_frame();

        let mut sink = TapeSink::open(&path, &meta()).unwrap();
        sink.record(&[status, trade]).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body.lines().count(), 1, "status skipped, trade kept");
        assert!(body.contains("\"trade\""));
        assert!(
            !body.contains("connection_id"),
            "status frame must not be written"
        );
    }

    #[test]
    fn reopen_keeps_first_identity_and_absorbs_new_channels() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.jsonl");
        let mut sink = TapeSink::open(&path, &meta()).unwrap();
        sink.record(&[trade_frame()]).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let changed = RecordingDeclaration {
            window_start: "2027-01-01T00:00:00Z".parse().unwrap(),
            channels: vec!["book".into()],
            ..meta()
        };
        TapeSink::open(&path, &changed)
            .unwrap()
            .finalize(RecordingIntegrity::now())
            .unwrap();

        let report: RecordingManifest =
            serde_json::from_str(&std::fs::read_to_string(sidecar_path(&path)).unwrap()).unwrap();
        assert_eq!(
            report.meta.window_start,
            "2026-01-01T00:00:00Z"
                .parse::<chrono::DateTime<chrono::Utc>>()
                .unwrap(),
            "window_start stays from the first open"
        );
        assert_eq!(
            report.meta.channels,
            ["trades", "book"],
            "channels absorb the reopen's additions, so the report covers every frame"
        );
    }

    #[test]
    fn open_stamps_an_unfinalized_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.jsonl");
        let _sink = TapeSink::open(&path, &meta()).unwrap();

        let report: RecordingManifest =
            serde_json::from_str(&std::fs::read_to_string(sidecar_path(&path)).unwrap()).unwrap();
        assert_eq!(report.schema_version, schema::SCHEMA_VERSION);
        assert_eq!(report.meta, meta());
        assert_eq!(report.summary, None, "no summary until finalize");
    }

    #[test]
    fn reopen_resets_the_summary_until_this_runs_finalize() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.jsonl");
        let mut sink = TapeSink::open(&path, &meta()).unwrap();
        sink.record(&[trade_frame()]).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let _reopened = TapeSink::open(&path, &meta()).unwrap();
        let report: RecordingManifest =
            serde_json::from_str(&std::fs::read_to_string(sidecar_path(&path)).unwrap()).unwrap();
        assert_eq!(report.summary, None, "reopen clears the finalize stamp");
    }

    #[test]
    fn existing_tape_without_a_sidecar_is_refused() {
        // Removing the sidecar simulates an unstamped or foreign tape. The
        // recorded frame matters: only an *empty* sidecar-less file is the
        // known crash artifact — one with content stays refused.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.jsonl");
        let mut sink = TapeSink::open(&path, &meta()).unwrap();
        sink.record(&[trade_frame()]).unwrap();
        drop(sink);
        std::fs::remove_file(sidecar_path(&path)).unwrap();

        let err = TapeSink::open(&path, &meta()).unwrap_err();
        assert!(matches!(err, Error::Rejected(_)));
        assert!(
            err.to_string().contains("no metadata sidecar"),
            "got: {err}"
        );
    }

    #[test]
    fn empty_sidecar_less_file_is_rescued_as_new() {
        // The crash artifact: `Log::create` touched the tape file, then the
        // process died before the sidecar's first write. An empty file holds
        // no frames, so a later open bootstraps it instead of refusing forever.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.jsonl");
        std::fs::write(&path, "").unwrap();

        let sink = TapeSink::open(&path, &meta()).unwrap();
        drop(sink);
        let report: RecordingManifest =
            serde_json::from_str(&std::fs::read_to_string(sidecar_path(&path)).unwrap()).unwrap();
        assert_eq!(report.schema_version, schema::SCHEMA_VERSION);
        assert_eq!(report.meta, meta());
    }

    #[test]
    fn orphaned_sidecar_next_to_an_empty_log_is_not_adopted() {
        // A log deleted by hand leaves its sidecar behind (nothing removes
        // the sidecar). The next capture at that path records new content:
        // adopting the orphan would stamp the old session's window_start — and a
        // foreign version — onto frames it never described.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.jsonl");
        let mut sink = TapeSink::open(&path, &meta()).unwrap();
        sink.record(&[trade_frame()]).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();
        std::fs::remove_file(&path).unwrap();
        let doctored = std::fs::read_to_string(sidecar_path(&path))
            .unwrap()
            .replace(schema::SCHEMA_VERSION, "1.7");
        std::fs::write(sidecar_path(&path), doctored).unwrap();

        let fresh = RecordingDeclaration {
            window_start: "2027-01-01T00:00:00Z".parse().unwrap(),
            ..meta()
        };
        drop(TapeSink::open(&path, &fresh).unwrap());
        let report: RecordingManifest =
            serde_json::from_str(&std::fs::read_to_string(sidecar_path(&path)).unwrap()).unwrap();
        assert_eq!(report.schema_version, schema::SCHEMA_VERSION, "fresh stamp");
        assert_eq!(
            report.meta.window_start,
            "2027-01-01T00:00:00Z"
                .parse::<chrono::DateTime<chrono::Utc>>()
                .unwrap()
        );
        assert_eq!(report.summary, None, "the orphan's finalize stamp is gone");
    }

    #[test]
    fn incompatible_orphan_sidecar_is_restamped_not_rejected() {
        // Deliberate divergence from the non-empty path (which rejects a 2.0
        // stamp): with zero committed bytes there is no content the foreign
        // stamp could describe, so refusing would only force a manual delete.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.jsonl");
        let mut sink = TapeSink::open(&path, &meta()).unwrap();
        sink.record(&[trade_frame()]).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();
        std::fs::remove_file(&path).unwrap();
        let doctored = std::fs::read_to_string(sidecar_path(&path))
            .unwrap()
            .replace(schema::SCHEMA_VERSION, "2.0");
        std::fs::write(sidecar_path(&path), doctored).unwrap();

        drop(TapeSink::open(&path, &meta()).expect("empty log bootstraps fresh"));
        let report: RecordingManifest =
            serde_json::from_str(&std::fs::read_to_string(sidecar_path(&path)).unwrap()).unwrap();
        assert_eq!(report.schema_version, schema::SCHEMA_VERSION);
    }

    #[test]
    fn undeclared_recordable_frame_is_skipped() {
        // Mirrors the DuckDB sink's declared-channel filter: a session that
        // declared only `trades` must not persist a stray ticker frame.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.jsonl");
        let ticker = ChannelMessage::parse(
            r#"{"channel":"ticker","type":"update","data":[{"symbol":"BTC/USD",
               "bid":1.0,"bid_qty":2.0,"ask":3.0,"ask_qty":4.0,"last":5.0,
               "volume":5.0,"vwap":6.0,"low":0.5,"high":9.0,"change":-1.0,"change_pct":-2.5,
               "timestamp":"TS"}]}"#,
        )
        .unwrap();
        let trade = trade_frame();

        let mut sink = TapeSink::open(&path, &meta()).unwrap();
        sink.record(&[ticker, trade]).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body.lines().count(), 1, "ticker skipped, trade kept");
        assert!(body.contains("\"trade\""));
        assert!(!body.contains("\"ticker\""));
    }

    #[test]
    fn finalize_keeps_the_first_runs_version_stamp() {
        // The stamp names the content's vocabulary: a 1.7 tape finalized
        // by a 1.0 build must still say 1.7, not downgrade.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.jsonl");
        let mut sink = TapeSink::open(&path, &meta()).unwrap();
        sink.record(&[trade_frame()]).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();
        let sidecar = sidecar_path(&path);
        let doctored = std::fs::read_to_string(&sidecar)
            .unwrap()
            .replace(schema::SCHEMA_VERSION, "1.7");
        std::fs::write(&sidecar, doctored).unwrap();

        TapeSink::open(&path, &meta())
            .unwrap()
            .finalize(RecordingIntegrity::now())
            .unwrap();
        let report: RecordingManifest =
            serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
        assert_eq!(report.schema_version, "1.7", "stamp survives the reopen");
    }

    #[test]
    fn reopen_refuses_an_incompatible_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BTC-USD.jsonl");
        let mut sink = TapeSink::open(&path, &meta()).unwrap();
        sink.record(&[trade_frame()]).unwrap();
        sink.finalize(RecordingIntegrity::now()).unwrap();
        let sidecar = sidecar_path(&path);
        let doctored = std::fs::read_to_string(&sidecar)
            .unwrap()
            .replace(schema::SCHEMA_VERSION, "2.0");
        std::fs::write(&sidecar, doctored).unwrap();

        let err = TapeSink::open(&path, &meta()).unwrap_err();
        assert!(matches!(err, Error::Rejected(_)));
        assert!(err.to_string().contains("refusing to append"), "got: {err}");
    }
}
