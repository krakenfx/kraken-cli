//! [`Fanout`] — a [`Sink`] that writes one recorded stream to several
//! destinations.
//!
//! Built from the `--to` targets: any of the durable backends (duckdb/jsonl)
//! plus, best-effort, stdout. Durable write errors are fatal; a stdout failure
//! (e.g. a closed `| head` pipe) is logged and the echo dropped so the durable
//! recording continues — unless stdout is the only sink, where its failure ends
//! the session instead of recording into nothing.

use crate::{
    output::OutputFormat,
    record::{RecordFormat, SinkPlan},
    sink::stdout::StdoutSink,
};
use kraken_core::{ChannelMessage, endpoint::Endpoint};
use kraken_recording::{
    CaptureSink, Error as RecordingError, RecordingDeclaration, RecordingIntegrity, Result, Sink,
    TapeSink,
};
use std::path::{Path, PathBuf};

/// The recorder's composite sink: writes each batch to every configured
/// destination.
#[derive(Default)]
pub(crate) struct Fanout {
    #[cfg(feature = "record-duckdb")]
    duckdb: Option<kraken_recording::DuckdbSink>,
    jsonl: Option<TapeSink>,
    stdout: Option<StdoutSink>,
}

impl Fanout {
    /// Open the sinks named by `plan`: each durable backend at
    /// `path_for(backend)`, plus stdout if requested. `path_for` lets each
    /// command place the tape (`recordings/` vs a session dir).
    ///
    /// # Errors
    /// Propagates a sink open failure (busy tape, incompatible schema), or
    /// reports that a `duckdb` target was requested on a build without the
    /// `record-duckdb` feature.
    pub(crate) fn build(
        plan: &SinkPlan,
        meta: &RecordingDeclaration,
        format: OutputFormat,
        path_for: impl Fn(RecordFormat) -> PathBuf,
    ) -> Result<Self> {
        let mut fanout = Self::default();
        for &backend in &plan.durables {
            fanout.open_durable(backend, &path_for(backend), meta)?;
        }
        if plan.echo {
            fanout.stdout = Some(StdoutSink::new(format));
        }
        Ok(fanout)
    }

    /// Open one durable backend at `path` and hold it.
    ///
    /// # Errors
    /// Propagates the sink open failure, or a validation error if `Duckdb` is
    /// requested on a build compiled without the `record-duckdb` feature.
    fn open_durable(
        &mut self,
        backend: RecordFormat,
        path: &Path,
        meta: &RecordingDeclaration,
    ) -> Result<()> {
        match backend {
            RecordFormat::Jsonl => self.jsonl = Some(TapeSink::open(path, meta)?),
            #[cfg(feature = "record-duckdb")]
            RecordFormat::Duckdb => {
                self.duckdb = Some(kraken_recording::DuckdbSink::open(path, meta)?);
            }
            #[cfg(not(feature = "record-duckdb"))]
            RecordFormat::Duckdb => {
                return Err(RecordingError::Rejected(
                    "this build has no DuckDB support; rebuild with `--features record-duckdb` or \
                     record with `--to jsonl`"
                        .into(),
                ));
            }
        }
        Ok(())
    }

    /// Drop the failed stdout tee and record on — unless it is the only sink,
    /// where its failure is the session's failure (matching `ws`/`streamd`)
    /// rather than a session that silently streams into nothing.
    fn drop_failed_tee(&mut self, err: RecordingError) -> Result<()> {
        if !self.has_durable() {
            return Err(err);
        }
        tracing::warn!(%err, "stdout tee failed; continuing to record");
        self.stdout = None; // e.g. the reader closed a `| head` pipe
        Ok(())
    }

    fn has_durable(&self) -> bool {
        #[cfg(feature = "record-duckdb")]
        if self.duckdb.is_some() {
            return true;
        }
        self.jsonl.is_some()
    }
}

// Inherits the default `flush`/`close`, correct while every member is
// durable-per-batch; a member that starts buffering needs both forwarded.
impl Sink for Fanout {
    type Item = ChannelMessage;

    fn record(&mut self, batch: &[ChannelMessage]) -> Result<()> {
        #[cfg(feature = "record-duckdb")]
        if let Some(sink) = self.duckdb.as_mut() {
            sink.record(batch)?;
        }
        if let Some(sink) = self.jsonl.as_mut() {
            sink.record(batch)?;
        }
        if let Some(sink) = self.stdout.as_mut()
            && let Err(err) = sink.record(batch)
        {
            self.drop_failed_tee(err)?;
        }
        Ok(())
    }
}

impl CaptureSink for Fanout {
    /// Same routing and tee policy as [`record`](Sink::record): the durable
    /// backends take their no-op default (tapes hold frames only), the stdout
    /// echo emits the marker.
    fn record_reconnect(&mut self, endpoint: Endpoint) -> Result<()> {
        #[cfg(feature = "record-duckdb")]
        if let Some(sink) = self.duckdb.as_mut() {
            sink.record_reconnect(endpoint)?;
        }
        if let Some(sink) = self.jsonl.as_mut() {
            sink.record_reconnect(endpoint)?;
        }
        if let Some(sink) = self.stdout.as_mut()
            && let Err(err) = sink.record_reconnect(endpoint)
        {
            self.drop_failed_tee(err)?;
        }
        Ok(())
    }

    fn finalize(self, summary: RecordingIntegrity) -> Result<()> {
        if let Some(sink) = self.stdout
            && let Err(err) = sink.finalize(summary.clone())
        {
            tracing::warn!(%err, "stdout tee finalize failed");
        }
        // Finalize every durable backend even if one fails; report the first error.
        let mut result = Ok(());
        #[cfg(feature = "record-duckdb")]
        if let Some(sink) = self.duckdb {
            result = keep_first(result, sink.finalize(summary.clone()), "duckdb");
        }
        if let Some(sink) = self.jsonl {
            result = keep_first(result, sink.finalize(summary), "jsonl");
        }
        result
    }
}

/// Keep `primary` when it failed, folding `secondary` in when it is clean — the
/// recording-error twin of `stream::keep_first`. Each error surfaces exactly
/// once: the survivor through the returned `Result`, a masked one through one
/// log line here, at the point it is dropped.
fn keep_first(primary: Result<()>, secondary: Result<()>, backend: &str) -> Result<()> {
    if let (Err(_), Err(masked)) = (&primary, &secondary) {
        tracing::warn!(
            error = %masked,
            backend,
            "finalize error masked by an earlier backend's error"
        );
        return primary;
    }
    primary.and(secondary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_recording::schema;

    fn meta() -> RecordingDeclaration {
        RecordingDeclaration {
            source: schema::SOURCE.to_string(),
            symbols: vec!["BTC/USD".into()],
            channels: vec!["trade".into()],
            window_start: "2026-01-01T00:00:00Z".parse().unwrap(),
            cli_version: "test".into(),
        }
    }

    #[test]
    fn record_and_finalize_reach_every_durable_backend() {
        // The fan-out's reason to exist: a single recorded batch is written to every
        // durable backend in the plan, and finalize stamps each. Assert the jsonl
        // tape round-trips the frame (and, with the feature, the DuckDB one too).
        let dir = tempfile::tempdir().unwrap();
        let jsonl_path = dir.path().join("market.jsonl");
        #[cfg(feature = "record-duckdb")]
        let duckdb_path = dir.path().join("market.duckdb");

        #[cfg(feature = "record-duckdb")]
        let durables = vec![RecordFormat::Jsonl, RecordFormat::Duckdb];
        #[cfg(not(feature = "record-duckdb"))]
        let durables = vec![RecordFormat::Jsonl];
        let plan = SinkPlan {
            durables,
            echo: false,
        };

        let frame = ChannelMessage::parse(
            r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","side":"buy",
               "price":50000.1,"qty":0.5,"ord_type":"market","trade_id":1,
               "timestamp":"2026-01-01T00:00:00.000000Z"}]}"#,
        )
        .unwrap();

        let mut fanout = Fanout::build(&plan, &meta(), OutputFormat::Json, |backend| {
            dir.path().join(format!("market.{}", backend.extension()))
        })
        .unwrap();
        fanout.record(std::slice::from_ref(&frame)).unwrap();
        fanout
            .finalize(RecordingIntegrity {
                window_end: "2026-01-01T00:01:00Z".parse().unwrap(),
                ..RecordingIntegrity::now()
            })
            .unwrap();

        let jsonl = std::fs::read_to_string(&jsonl_path).unwrap();
        let lines: Vec<&str> = jsonl.lines().collect();
        assert_eq!(lines.len(), 1);
        assert_eq!(ChannelMessage::parse(lines[0]).unwrap(), frame);
        let sidecar = std::fs::read_to_string(dir.path().join("market.jsonl.meta.json")).unwrap();
        assert!(
            sidecar.contains("2026-01-01T00:01:00Z"),
            "finalize must stamp the jsonl sidecar"
        );

        #[cfg(feature = "record-duckdb")]
        {
            let db = kraken_recording::frames::duckdb::Db::open(&duckdb_path).unwrap();
            assert_eq!(db.scalar_u64("SELECT count(*) FROM trades").unwrap(), 1);
            assert_eq!(
                db.meta_get("window_end").unwrap().as_deref(),
                Some("2026-01-01T00:01:00Z"),
                "finalize must stamp the duckdb _meta"
            );
        }
    }

    fn broken_pipe() -> RecordingError {
        RecordingError::Io(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "pipe closed",
        ))
    }

    #[test]
    fn a_stdout_only_session_dies_with_its_pipe() {
        // With no durable backend the tee IS the session: dropping it would leave the
        // recorder consuming the venue feed into nothing, forever, and exiting 0.
        let plan = SinkPlan {
            durables: vec![],
            echo: true,
        };
        let mut fanout =
            Fanout::build(&plan, &meta(), OutputFormat::Json, |_| unreachable!()).unwrap();
        let err = fanout.drop_failed_tee(broken_pipe()).unwrap_err();
        assert!(matches!(err, RecordingError::Io(_)));
    }

    #[test]
    fn a_broken_tee_is_dropped_while_a_durable_records() {
        let dir = tempfile::tempdir().unwrap();
        let plan = SinkPlan {
            durables: vec![RecordFormat::Jsonl],
            echo: true,
        };
        let mut fanout = Fanout::build(&plan, &meta(), OutputFormat::Json, |backend| {
            dir.path().join(format!("market.{}", backend.extension()))
        })
        .unwrap();
        fanout.drop_failed_tee(broken_pipe()).unwrap();
        assert!(fanout.stdout.is_none(), "the tee is gone");
        assert!(fanout.jsonl.is_some(), "the durable backend records on");
    }

    // With `record-duckdb` off by default, this arm is what a `--to duckdb`
    // recorder hits on a stock build: it must reject, not record into nothing.
    #[cfg(not(feature = "record-duckdb"))]
    #[test]
    fn a_duckdb_target_without_the_feature_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let plan = SinkPlan {
            durables: vec![RecordFormat::Duckdb],
            echo: false,
        };

        let build = Fanout::build(&plan, &meta(), OutputFormat::Json, |backend| {
            dir.path().join(format!("market.{}", backend.extension()))
        });

        let Err(err) = build else {
            panic!("a duckdb target must not open on a build without the feature")
        };
        assert!(matches!(err, RecordingError::Rejected(_)));
        assert!(err.to_string().contains("record-duckdb"), "got: {err}");
    }
}
