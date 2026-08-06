//! Chronological composition of market, account, and decision tracks.
//!
//! Shared locks reject live writers, while paired account and decision reads
//! share one cut so a trade cannot appear without its rationale.

use std::path::Path;

use kraken_paper::AccountEvent;
use kraken_paper::account::{AccountRecord, RECORD_VERSION};
use kraken_recording::FileLock;
use kraken_recording::JsonlSource;
use kraken_recording::RecordingManifest;
use kraken_recording::TapeSource;
use kraken_recording::{CaptureSource, line_seq};

use self::event::TimelineEvent;
use crate::decision::{Decision, DecisionKind};
use crate::manifest::{RecordingBackend, RecordingRef, SessionManifest};
use crate::{
    Result, SessionError, SessionName, decisions_path, dir, ensure_provisioned, events_path,
};

pub mod event;

/// A session read whole: every recorded event in `(at, track, seq)` order,
/// the market capture's self-description, and the manifest that named the
/// tracks (read once here — consumers must not reload it).
#[derive(Debug)]
pub struct SessionTimeline {
    pub events: Vec<TimelineEvent>,
    /// `None` when the session has no market recording. A report whose
    /// `summary` is `None` is a capture that never finalized (a crashed
    /// recorder) — completeness is then unknown, and consumers must say so
    /// rather than replay holes silently. Interrogate via
    /// [`CaptureState::of`](kraken_recording::CaptureState::of).
    pub capture: Option<RecordingManifest>,
    pub manifest: SessionManifest,
    /// Account journal records from a newer build, absent from `events`.
    /// Unknown event kinds are dropped too but not counted — the engine's
    /// strict fold ignores those, so their absence cannot skew a fold; a
    /// whole version it cannot read can. Display consumers may tolerate a
    /// nonzero count; consumers computing money from the account track
    /// must refuse.
    pub newer_records_skipped: usize,
}

/// Read a provisioned session's full timeline. `validation` when the session
/// doesn't exist, a writer still holds one of its logs, or its market
/// recording needs a backend this build lacks.
pub fn read(base: &Path, name: &SessionName) -> Result<SessionTimeline> {
    ensure_provisioned(base, name)?;
    let manifest = SessionManifest::load(&crate::manifest_path(base, name))?;

    let mut events = Vec::new();
    let capture = read_market_track_in(&dir(base, name), &manifest, &mut events)?;
    // Writer-order locks keep an account event and its decision in one snapshot.
    // Missing files are locked too, preventing first-write races.
    let journal = events_path(base, name);
    let decisions = decisions_path(base, name);
    let _journal_guard = read_guard(&journal, "paper account journal")?;
    let _decisions_guard = read_guard(&decisions, "session decision log")?;
    read_decisions_track(&decisions, &mut events)?;
    let newer_records_skipped = read_account_track(&journal, &mut events)?;
    events.sort_by_key(TimelineEvent::key);
    Ok(SessionTimeline {
        events,
        capture,
        manifest,
        newer_records_skipped,
    })
}

/// Reads one durable backend, preferring DuckDB and refusing damaged primary data.
pub(crate) fn read_market_track_in(
    recording_dir: &Path,
    manifest: &SessionManifest,
    events: &mut Vec<TimelineEvent>,
) -> Result<Option<RecordingManifest>> {
    let by_backend = |backend: RecordingBackend| -> Option<&RecordingRef> {
        manifest.recordings.iter().find(|r| r.backend == backend)
    };

    #[cfg(feature = "duckdb")]
    if let Some(recording) = by_backend(RecordingBackend::Duckdb) {
        ensure_readable_schema(recording)?;
        let source = kraken_recording::DuckdbSource::open(&recording_dir.join(&recording.file))?;
        return collect_market(source, events);
    }

    if let Some(recording) = by_backend(RecordingBackend::Jsonl) {
        ensure_readable_schema(recording)?;
        let source = TapeSource::open(&recording_dir.join(&recording.file))?;
        return collect_market(source, events);
    }

    #[cfg(not(feature = "duckdb"))]
    if by_backend(RecordingBackend::Duckdb).is_some() {
        return Err(SessionError::Rejected(
            "this session's market recording is DuckDB-only and this build has no DuckDB \
             support; rebuild with `--features record-duckdb`"
                .into(),
        ));
    }

    Ok(None)
}

/// Gate on the manifest's version stamp — present even if the recording never
/// finalized — so an old-MAJOR session is rejected, not misread as damage.
fn ensure_readable_schema(recording: &RecordingRef) -> Result<()> {
    if kraken_recording::schema::is_compatible(&recording.schema_version) {
        return Ok(());
    }
    Err(SessionError::Rejected(format!(
        "session recording '{}' has schema version {}, which this build cannot read \
         (reads v{}); re-record the session",
        recording.file,
        recording.schema_version,
        kraken_recording::schema::SCHEMA_VERSION
    )))
}

/// Reads the version stamp before decoding frames under its contract.
fn collect_market(
    source: impl CaptureSource,
    events: &mut Vec<TimelineEvent>,
) -> Result<Option<RecordingManifest>> {
    let capture = source.capture()?;
    events.extend(source.read()?.map(TimelineEvent::from));
    Ok(Some(capture))
}

/// Caller contract: `read` holds this track's [`read_guard`] — acquired
/// there, not here, so the paired account/decision tracks share one cut.
pub(crate) fn read_decisions_track(path: &Path, events: &mut Vec<TimelineEvent>) -> Result<()> {
    let Some(source) = JsonlSource::<Decision>::open(path.to_path_buf()) else {
        return Ok(());
    };
    for (line, decision) in source.read_numbered()? {
        if matches!(decision.kind, DecisionKind::Unknown) {
            tracing::warn!(path = %path.display(), line, "skipping unknown decision kind");
            continue;
        }
        events.push(TimelineEvent::decision(
            decision.timestamp,
            line_seq(line),
            decision,
        ));
    }
    Ok(())
}

/// Caller contract: `read` holds this track's [`read_guard`] — acquired
/// there, not here, so the paired account/decision tracks share one cut.
/// Returns how many newer-version records were dropped, for
/// [`SessionTimeline::newer_records_skipped`].
fn read_account_track(path: &Path, events: &mut Vec<TimelineEvent>) -> Result<usize> {
    let Some(source) = JsonlSource::<AccountRecord>::open(path.to_path_buf()) else {
        return Ok(0);
    };
    let mut newer_skipped = 0usize;
    for (line, record) in source.read_numbered()? {
        // Display readers may skip future records; money readers inspect the count and refuse.
        if record.v > RECORD_VERSION {
            tracing::warn!(path = %path.display(), line, v = record.v, "skipping newer-version journal record");
            newer_skipped += 1;
            continue;
        }
        if matches!(record.event, AccountEvent::Unknown) {
            tracing::warn!(path = %path.display(), line, "skipping unknown account event kind");
            continue;
        }
        events.push(TimelineEvent::account(record.ts, line_seq(line), record));
    }
    Ok(newer_skipped)
}

/// Held shared for the whole read: a live writer's torn-tail repair must not
/// be observed mid-splice, and shared holders coexist without registering on
/// the exclusive-only recorder-liveness probe. Taken even when the tracked
/// file doesn't exist yet — a writer creating it mid-read would tear the
/// cross-track cut.
pub(crate) fn read_guard(path: &Path, purpose: &str) -> Result<FileLock> {
    Ok(FileLock::acquire_shared(path, purpose)?)
}

/// Shared fixture builders for timeline tests and downstream consumers' tests
/// (`kraken replay`, `kraken explain pnl`, the Lab): provision a session
/// under a temp base and write each track exactly as the real writers do.
/// Crossing crates only via the `fixtures` cargo feature, never bare
/// `cfg(test)`.
#[cfg(any(test, feature = "fixtures"))]
// Fixture misuse is a test bug, so unwrap/expect/panic are the right tool —
// but `--all-features` compiles this module outside cfg(test), where
// clippy.toml's allow-*-in-tests carve-outs do not apply. (`#[expect]` would
// be unfulfilled in cfg(test) compilations and trip `-D warnings`.)
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
pub mod fixtures {
    use std::collections::HashMap;
    use std::path::Path;

    use kraken_core::OrderSide;
    use kraken_paper::account::{AccountRecord, Origin, RECORD_VERSION};
    use kraken_paper::{AccountEvent, PaperConfig, PaperOrder, PaperState, PaperTrade};
    use kraken_recording::JsonlSink;
    use kraken_recording::RecordingDeclaration;
    use kraken_recording::Sink;
    use kraken_recording::TapeSink;
    use kraken_recording::parse_instant;
    use kraken_recording::schema;
    use kraken_recording::{CaptureSink, MarketEvent, RecordingIntegrity, RecordingManifest};
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    use super::SessionTimeline;
    use super::event::TimelineEvent;
    use crate::decision::{Decision, DecisionKind};
    use crate::manifest::{
        PaperRef, RecordingBackend, RecordingRef, SessionManifest, SessionState,
    };
    use crate::{SessionName, decisions_path, dir, events_path};

    pub const NAME: &str = "btc-dip";

    pub const T0: &str = "2026-01-01T00:00:00Z";

    /// A journal-accurate in-memory session builder: a scratch [`PaperState`]
    /// mirrors every event so `decide_*` produces exactly what the engine
    /// would, while the events vec is what a composer actually consumes.
    pub struct SessionBuilder {
        pub events: Vec<TimelineEvent>,
        pub state: PaperState,
        seq: i64,
    }

    impl SessionBuilder {
        pub fn with_rates(balance: Decimal, fee_rate: Decimal, slippage_rate: Decimal) -> Self {
            let mut builder = Self {
                events: Vec::new(),
                state: PaperState::default(),
                seq: 0,
            };
            builder.account(
                T0,
                Origin::Cli,
                AccountEvent::Initialized(PaperConfig {
                    balance,
                    currency: "USD".to_string(),
                    fee_rate,
                    slippage_rate,
                }),
            );
            builder
        }

        fn next_seq(&mut self) -> i64 {
            self.seq += 1;
            self.seq
        }

        pub fn account(&mut self, ts: &str, origin: Origin, event: AccountEvent) {
            let at = parse_instant(ts).unwrap();
            self.state.apply(at, &event);
            let seq = self.next_seq();
            self.events.push(TimelineEvent::account(
                at,
                seq,
                AccountRecord {
                    v: RECORD_VERSION,
                    ts: at,
                    origin,
                    event,
                },
            ));
        }

        pub fn market_fill(
            &mut self,
            ts: &str,
            origin: Origin,
            side: OrderSide,
            pair: &str,
            volume: Decimal,
            (ask, bid): (Decimal, Decimal),
        ) -> PaperTrade {
            let mut trade = self
                .state
                .decide_market_order(side, pair, volume, ask, bid)
                .unwrap();
            trade.filled_at = parse_instant(ts).unwrap();
            self.account(
                ts,
                origin,
                AccountEvent::OrderFilled {
                    trade: trade.clone(),
                },
            );
            trade
        }

        pub fn submit_limit(
            &mut self,
            ts: &str,
            origin: Origin,
            side: OrderSide,
            pair: &str,
            volume: Decimal,
            price: Decimal,
        ) -> PaperOrder {
            let mut order = self
                .state
                .decide_limit_order(side, pair, volume, price)
                .unwrap();
            order.created_at = parse_instant(ts).unwrap();
            self.account(
                ts,
                origin,
                AccountEvent::OrderSubmitted {
                    order: order.clone(),
                },
            );
            order
        }

        pub fn fill_pending(
            &mut self,
            ts: &str,
            origin: Origin,
            pair: &str,
            ask: Decimal,
            bid: Decimal,
        ) {
            let prices = HashMap::from([(pair.to_string(), (ask, bid))]);
            let fills = self.state.decide_pending_fills(&prices);
            for mut trade in fills {
                trade.filled_at = parse_instant(ts).unwrap();
                self.account(ts, origin, AccountEvent::OrderFilled { trade });
            }
        }

        pub fn cancel(&mut self, ts: &str, origin: Origin, order_id: &str) {
            let order = self.state.decide_cancel(order_id).unwrap();
            self.account(ts, origin, AccountEvent::OrderCancelled { order });
        }

        pub fn ticker(&mut self, ts: &str, symbol: &str, bid: Decimal, ask: Decimal) {
            let frame = kraken_core::ChannelMessage::parse(&format!(
                r#"{{"channel":"ticker","type":"update","data":[{{"symbol":"{symbol}",
                   "bid":{bid},"bid_qty":1.0,"ask":{ask},"ask_qty":1.0,"last":{bid},
                   "volume":10.0,"vwap":{bid},"low":{bid},"high":{ask},"change":0.0,
                   "change_pct":0.0,"timestamp":"{ts}"}}]}}"#
            ))
            .unwrap();
            let seq = self.next_seq();
            self.events.push(TimelineEvent::from(MarketEvent {
                at: parse_instant(ts).unwrap(),
                seq,
                frame,
            }));
        }

        pub fn decision(&mut self, ts: &str, order_id: &str, reason: &str) {
            let seq = self.next_seq();
            self.events.push(TimelineEvent::decision(
                parse_instant(ts).unwrap(),
                seq,
                Decision {
                    timestamp: parse_instant(ts).unwrap(),
                    kind: DecisionKind::Buy,
                    symbol: Some("BTCUSD".to_string()),
                    reason: reason.to_string(),
                    order_id: Some(order_id.to_string()),
                },
            ));
        }

        pub fn session(
            mut self,
            capture: Option<RecordingManifest>,
            manifest: SessionManifest,
        ) -> SessionTimeline {
            // `timeline::read` delivers events sorted by the merge key; the
            // builder honors the same contract so same-instant frame/account
            // pairs order Market-first here too.
            self.events.sort_by_key(TimelineEvent::key);
            SessionTimeline {
                events: self.events,
                capture,
                manifest,
                newer_records_skipped: 0,
            }
        }
    }

    pub fn session_name() -> SessionName {
        NAME.parse().expect("valid test session name")
    }

    pub fn manifest(recordings: Vec<RecordingRef>) -> SessionManifest {
        SessionManifest {
            manifest_version: crate::MANIFEST_VERSION.to_string(),
            cli_version: "test".to_string(),
            id: NAME.to_string(),
            pid: 1,
            recordings,
            paper: PaperRef {
                starting_balance: dec!(10_000.0),
                currency: "USD".to_string(),
            },
            strategy: None,
            experiment: None,
            source: None,
            status: SessionState::Stopped,
            summary: None,
            created_at: "2026-01-01T00:00:00Z".parse().unwrap(),
            label: None,
            window: crate::manifest::SessionWindow {
                session: "r1".to_string(),
                started_at: "2026-01-01T00:00:00Z".parse().unwrap(),
                opening_equity: dec!(10_000.0),
                opening_complete: true,
                ended_at: None,
            },
        }
    }

    pub fn jsonl_recording() -> RecordingRef {
        RecordingRef {
            backend: RecordingBackend::Jsonl,
            file: "market.jsonl".to_string(),
            schema_version: schema::SCHEMA_VERSION.to_string(),
            symbols: vec!["BTC/USD".to_string()],
            channels: vec!["trade".to_string()],
        }
    }

    pub fn provision(base: &Path, recordings: Vec<RecordingRef>) -> SessionName {
        let name = session_name();
        manifest(recordings)
            .save(&crate::manifest_path(base, &name))
            .unwrap();
        name
    }

    pub fn write_market(base: &Path, name: &SessionName, timestamps: &[&str]) {
        write_frames_at(&dir(base, name).join("market.jsonl"), timestamps);
    }

    /// A session's tape: the same frames as [`write_market`], at
    /// `<session_dir>/tape.jsonl`.
    pub fn write_run_tape(session_dir: &Path, timestamps: &[&str]) {
        write_frames_at(&session_dir.join("tape.jsonl"), timestamps);
    }

    fn write_frames_at(path: &Path, timestamps: &[&str]) {
        let mut sink = TapeSink::open(
            path,
            &RecordingDeclaration {
                source: schema::SOURCE.to_string(),
                symbols: vec!["BTC/USD".into()],
                channels: vec!["trade".into()],
                window_start: "2026-01-01T00:00:00Z".parse().unwrap(),
                cli_version: "test".into(),
            },
        )
        .unwrap();
        let frames: Vec<_> = timestamps
            .iter()
            .enumerate()
            .map(|(i, ts)| {
                kraken_core::ChannelMessage::parse(&format!(
                    r#"{{"channel":"trade","type":"update","data":[{{"symbol":"BTC/USD",
                       "side":"buy","price":1.0,"qty":1.0,"ord_type":"market","trade_id":{i},
                       "timestamp":"{ts}"}}]}}"#
                ))
                .unwrap()
            })
            .collect();
        sink.record(&frames).unwrap();
        sink.finalize(RecordingIntegrity {
            window_end: "2026-01-01T01:00:00Z".parse().unwrap(),
            ..RecordingIntegrity::now()
        })
        .unwrap();
    }

    pub fn write_decision(base: &Path, name: &SessionName, ts: &str) {
        let mut sink: JsonlSink<Decision> =
            JsonlSink::create(decisions_path(base, name), "test decision log").unwrap();
        sink.record(&[Decision {
            timestamp: ts.parse().unwrap(),
            kind: DecisionKind::Buy,
            symbol: Some("BTCUSD".to_string()),
            reason: "dip".to_string(),
            order_id: Some("O1".to_string()),
        }])
        .unwrap();
    }

    pub fn account_record(ts: &str) -> AccountRecord {
        AccountRecord {
            v: RECORD_VERSION,
            ts: ts.parse().unwrap(),
            origin: Origin::Cli,
            event: AccountEvent::Initialized(PaperConfig {
                balance: dec!(10_000.0),
                currency: "USD".to_string(),
                fee_rate: dec!(0.0026),
                slippage_rate: dec!(0.0),
            }),
        }
    }

    pub fn write_account(base: &Path, name: &SessionName, records: &[AccountRecord]) {
        let mut sink: JsonlSink<AccountRecord> =
            JsonlSink::create(events_path(base, name), "test journal").unwrap();
        sink.record(records).unwrap();
    }

    /// A journal-shaped legacy fill (`reference_quote: None`): buy 1 BTC at
    /// 1.0 — the price [`write_market`]'s trade frames mark at.
    pub fn fill_record(ts: &str) -> AccountRecord {
        AccountRecord {
            v: RECORD_VERSION,
            ts: ts.parse().unwrap(),
            origin: Origin::Cli,
            event: AccountEvent::OrderFilled {
                trade: PaperTrade {
                    id: "PAPER-00001".to_string(),
                    order_id: "PAPER-00001".to_string(),
                    pair: "BTCUSD".to_string(),
                    base: "BTC".to_string(),
                    quote: "USD".to_string(),
                    side: OrderSide::Buy,
                    volume: dec!(1.0),
                    price: dec!(1.0),
                    fee: dec!(0.0026),
                    cost: dec!(1.0),
                    filled_at: ts.parse().unwrap(),
                    reference_quote: None,
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "duckdb")]
    use kraken_recording::{CaptureSink, RecordingDeclaration, RecordingIntegrity, Sink};
    use kraken_recording::{Source, schema};

    #[cfg(feature = "duckdb")]
    use super::event::EventPayload;
    use super::event::Track;
    use super::fixtures::*;
    use super::*;

    #[test]
    fn merges_three_tracks_chronologically_with_track_tiebreak() {
        // The tied instant is written in two offset styles (`Z` vs `+00:00`),
        // so the track tiebreak must hold across timestamp formats.
        let base = tempfile::tempdir().unwrap();
        let name = provision(base.path(), vec![jsonl_recording()]);
        write_market(
            base.path(),
            &name,
            &["2026-01-01T00:00:01.000000Z", "2026-01-01T00:00:03.000000Z"],
        );
        write_account(
            base.path(),
            &name,
            &[account_record("2026-01-01T00:00:02+00:00")],
        );
        write_decision(base.path(), &name, "2026-01-01T00:00:03+00:00");

        let timeline = read(base.path(), &name).unwrap();
        let tracks: Vec<Track> = timeline.events.iter().map(TimelineEvent::track).collect();
        assert_eq!(
            tracks,
            [
                Track::Market,
                Track::Account,
                Track::Market,
                Track::Decision
            ]
        );
        assert!(
            timeline.events.windows(2).all(|w| w[0].key() <= w[1].key()),
            "events are sorted by the merge key"
        );
        let capture = timeline.capture.expect("session has a market recording");
        let summary = capture.summary.expect("finalized capture");
        assert_eq!(
            summary.window_end,
            "2026-01-01T01:00:00Z"
                .parse::<chrono::DateTime<chrono::Utc>>()
                .unwrap()
        );
    }

    #[test]
    fn market_only_session_reads_without_other_tracks() {
        let base = tempfile::tempdir().unwrap();
        let name = provision(base.path(), vec![jsonl_recording()]);
        write_market(base.path(), &name, &["2026-01-01T00:00:01.000000Z"]);

        let timeline = read(base.path(), &name).unwrap();
        assert_eq!(timeline.events.len(), 1);
        assert_eq!(timeline.events[0].track(), Track::Market);
    }

    #[test]
    fn session_without_recordings_still_merges_account_and_decisions() {
        let base = tempfile::tempdir().unwrap();
        let name = provision(base.path(), vec![]);
        write_account(
            base.path(),
            &name,
            &[account_record("2026-01-01T00:00:01+00:00")],
        );
        write_decision(base.path(), &name, "2026-01-01T00:00:02+00:00");

        let timeline = read(base.path(), &name).unwrap();
        assert_eq!(timeline.events.len(), 2);
        assert!(timeline.capture.is_none(), "no market capture to report");
    }

    #[test]
    fn unprovisioned_session_is_a_validation_error() {
        let base = tempfile::tempdir().unwrap();
        let err = read(base.path(), &session_name()).unwrap_err();
        assert!(matches!(err, SessionError::Rejected(_)));
    }

    #[test]
    fn live_journal_writer_is_rejected() {
        // Like the real writer (`Log::create`), the holder touches the
        // file eagerly.
        let base = tempfile::tempdir().unwrap();
        let name = provision(base.path(), vec![]);
        let path = events_path(base.path(), &name);
        std::fs::write(&path, "").unwrap();
        let _held = kraken_recording::FileLock::acquire(&path, "test holder").unwrap();

        let err = read(base.path(), &name).unwrap_err();
        assert!(matches!(
            err,
            SessionError::Recording(kraken_recording::Error::Rejected(_))
        ));
        assert!(err.to_string().contains("in use"), "got: {err}");
    }

    #[test]
    fn journal_guard_is_taken_before_the_decision_log_is_read() {
        // The paired tracks are read under one cut, journal-first (the
        // writer's lock order). With a writer holding the journal, the read
        // must be rejected before decoding decisions — the damaged decision
        // log proves the ordering by *not* surfacing as a parse error.
        let base = tempfile::tempdir().unwrap();
        let name = provision(base.path(), vec![]);
        std::fs::write(
            decisions_path(base.path(), &name),
            "{\"timestamp\":\"not-a-timestamp\",\"kind\":\"skip\",\"reason\":\"x\"}\n",
        )
        .unwrap();
        let journal = events_path(base.path(), &name);
        std::fs::write(&journal, "").unwrap();
        let _writer = kraken_recording::FileLock::acquire(&journal, "test writer").unwrap();

        let err = read(base.path(), &name).unwrap_err();
        assert!(
            matches!(
                err,
                SessionError::Recording(kraken_recording::Error::Rejected(_))
            ),
            "got: {err}"
        );
        assert!(err.to_string().contains("in use"), "got: {err}");
    }

    #[test]
    fn unknown_account_event_is_skipped_not_fatal() {
        let base = tempfile::tempdir().unwrap();
        let name = provision(base.path(), vec![]);
        write_account(
            base.path(),
            &name,
            &[account_record("2026-01-01T00:00:01+00:00")],
        );
        // The strict reader proves the line still parses (as Unknown): we are
        // testing the timeline's *skip*, not a parse failure.
        let path = events_path(base.path(), &name);
        let raw = format!(
            "{}\n",
            r#"{"v":1,"ts":"2026-01-01T00:00:02+00:00","origin":"cli","event":"margin_called"}"#
        );
        std::fs::write(
            &path,
            [std::fs::read_to_string(&path).unwrap(), raw].concat(),
        )
        .unwrap();
        let parsed: Vec<AccountRecord> = JsonlSource::open(path).unwrap().read().unwrap().collect();
        assert!(matches!(parsed[1].event, AccountEvent::Unknown));

        let timeline = read(base.path(), &name).unwrap();
        assert_eq!(timeline.events.len(), 1, "unknown event skipped");
        assert_eq!(
            timeline.newer_records_skipped, 0,
            "an unknown kind is benign (the strict fold ignores it too), not a version drop"
        );
    }

    #[test]
    fn newer_version_journal_record_is_skipped_not_fatal() {
        let base = tempfile::tempdir().unwrap();
        let name = provision(base.path(), vec![]);
        write_account(
            base.path(),
            &name,
            &[account_record("2026-01-01T00:00:01+00:00")],
        );
        let path = events_path(base.path(), &name);
        let raw = format!(
            "{}\n",
            r#"{"v":99,"ts":"2026-01-01T00:00:02+00:00","origin":"cli","event":"initialized","balance":"1.0","currency":"USD","fee_rate":"0.0","slippage_rate":"0.0"}"#
        );
        std::fs::write(
            &path,
            [std::fs::read_to_string(&path).unwrap(), raw].concat(),
        )
        .unwrap();

        let timeline = read(base.path(), &name).unwrap();
        assert_eq!(timeline.events.len(), 1, "newer-version record skipped");
        assert_eq!(
            timeline.newer_records_skipped, 1,
            "the drop is disclosed so money consumers can refuse"
        );
    }

    #[test]
    fn manifest_referencing_a_missing_recording_is_a_validation_error() {
        let base = tempfile::tempdir().unwrap();
        let name = provision(base.path(), vec![jsonl_recording()]); // no market.jsonl on disk

        let err = read(base.path(), &name).unwrap_err();
        assert!(matches!(
            err,
            SessionError::Recording(kraken_recording::Error::Rejected(_))
        ));
        assert!(err.to_string().contains("not found"), "got: {err}");
    }

    #[test]
    fn different_major_recording_is_rejected_via_the_manifest_stamp() {
        let base = tempfile::tempdir().unwrap();
        let name = provision(
            base.path(),
            vec![RecordingRef {
                schema_version: "2.0".to_string(),
                ..jsonl_recording()
            }],
        );
        std::fs::write(dir(base.path(), &name).join("market.jsonl"), "").unwrap();

        let err = read(base.path(), &name).unwrap_err();
        assert!(matches!(err, SessionError::Rejected(_)));
        assert!(err.to_string().contains("re-record"), "got: {err}");
    }

    #[test]
    fn unknown_decision_kind_is_skipped_not_fatal() {
        let base = tempfile::tempdir().unwrap();
        let name = provision(base.path(), vec![]);
        write_decision(base.path(), &name, "2026-01-01T00:00:01+00:00");
        let path = decisions_path(base.path(), &name);
        let raw = format!(
            "{}\n",
            r#"{"timestamp":"2026-01-01T00:00:02+00:00","kind":"hold","reason":"wait"}"#
        );
        std::fs::write(
            &path,
            [std::fs::read_to_string(&path).unwrap(), raw].concat(),
        )
        .unwrap();
        // The strict reader proves the line still parses (as Unknown): we are
        // testing the timeline's *skip*, not a parse failure.
        let parsed: Vec<Decision> = JsonlSource::open(path).unwrap().read().unwrap().collect();
        assert_eq!(parsed[1].kind, DecisionKind::Unknown);

        let timeline = read(base.path(), &name).unwrap();
        assert_eq!(timeline.events.len(), 1, "unknown kind skipped");
    }

    #[test]
    fn decision_with_unparseable_timestamp_is_damage() {
        let base = tempfile::tempdir().unwrap();
        let name = provision(base.path(), vec![]);
        std::fs::write(
            decisions_path(base.path(), &name),
            "{\"timestamp\":\"not-a-timestamp\",\"kind\":\"skip\",\"reason\":\"x\"}\n",
        )
        .unwrap();

        let err = read(base.path(), &name).unwrap_err();
        assert!(matches!(
            err,
            SessionError::Recording(kraken_recording::Error::Damaged(_))
        ));
        // Typed `DateTime` fields reject a bad stamp at decode, so the line
        // surfaces as malformed rather than reaching the reader.
        assert!(err.to_string().contains("malformed log line"), "got: {err}");
    }

    #[test]
    fn account_seq_counts_physical_lines_across_blanks() {
        // A hand-edited journal with a blank line: `seq` must keep naming
        // the on-disk line, in agreement with the market track's `line_seq`
        // and the jsonl reader's damage diagnostics.
        let base = tempfile::tempdir().unwrap();
        let name = provision(base.path(), vec![]);
        write_account(
            base.path(),
            &name,
            &[account_record("2026-01-01T00:00:01+00:00")],
        );
        let path = events_path(base.path(), &name);
        let raw = format!(
            "\n{}\n",
            serde_json::to_string(&account_record("2026-01-01T00:00:02+00:00")).unwrap()
        );
        std::fs::write(
            &path,
            [std::fs::read_to_string(&path).unwrap(), raw].concat(),
        )
        .unwrap();

        let timeline = read(base.path(), &name).unwrap();
        let seqs: Vec<i64> = timeline.events.iter().map(|event| event.seq).collect();
        assert_eq!(seqs, [1, 3], "the blank line is counted, not renumbered");
    }

    #[test]
    fn decision_damage_names_the_physical_line_across_blanks() {
        let base = tempfile::tempdir().unwrap();
        let name = provision(base.path(), vec![]);
        std::fs::write(
            decisions_path(base.path(), &name),
            "\n{\"timestamp\":\"not-a-timestamp\",\"kind\":\"skip\",\"reason\":\"x\"}\n",
        )
        .unwrap();

        let err = read(base.path(), &name).unwrap_err();
        assert!(matches!(
            err,
            SessionError::Recording(kraken_recording::Error::Damaged(_))
        ));
        assert!(err.to_string().contains("line 2"), "got: {err}");
    }

    #[test]
    fn account_record_with_unparseable_timestamp_is_damage() {
        let base = tempfile::tempdir().unwrap();
        let name = provision(base.path(), vec![]);
        std::fs::write(
            events_path(base.path(), &name),
            r#"{"v":1,"ts":"not-a-timestamp","origin":"cli","event":"initialized","balance":"1.0","currency":"USD","fee_rate":"0.0","slippage_rate":"0.0"}"#.to_string() + "\n",
        )
        .unwrap();

        let err = read(base.path(), &name).unwrap_err();
        assert!(matches!(
            err,
            SessionError::Recording(kraken_recording::Error::Damaged(_))
        ));
        // Typed `DateTime` fields reject a bad stamp at decode, so the line
        // surfaces as malformed rather than reaching the reader.
        assert!(err.to_string().contains("malformed log line"), "got: {err}");
    }

    #[test]
    fn concurrent_reader_does_not_block_the_timeline_read() {
        let base = tempfile::tempdir().unwrap();
        let name = provision(base.path(), vec![jsonl_recording()]);
        write_market(base.path(), &name, &["2026-01-01T00:00:01.000000Z"]);
        let market = dir(base.path(), &name).join("market.jsonl");
        let _other_reader =
            kraken_recording::FileLock::acquire_shared(&market, "another reader").unwrap();

        let timeline = read(base.path(), &name).expect("shared holders coexist");
        assert_eq!(timeline.events.len(), 1);
    }

    #[test]
    fn a_sessions_first_note_is_rejected_while_the_read_guard_is_held() {
        // The decision log doesn't exist yet, but the guard must still cover
        // its track: a first `playground note` landing between the decision
        // and account reads would tear the cross-track cut.
        let base = tempfile::tempdir().unwrap();
        let name = provision(base.path(), vec![]);
        let decisions = decisions_path(base.path(), &name);
        let _cut = read_guard(&decisions, "session decision log").unwrap();

        let err = crate::decision::append(
            &decisions,
            DecisionKind::Buy,
            None,
            "mid-read note".to_string(),
            None,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SessionError::Recording(kraken_recording::Error::Rejected(_))
        ));
        assert!(err.to_string().contains("in use"), "got: {err}");
    }

    #[cfg(not(feature = "duckdb"))]
    #[test]
    fn duckdb_only_session_without_the_feature_is_a_validation_error() {
        let base = tempfile::tempdir().unwrap();
        let name = provision(
            base.path(),
            vec![RecordingRef {
                backend: RecordingBackend::Duckdb,
                file: "market.duckdb".to_string(),
                schema_version: schema::SCHEMA_VERSION.to_string(),
                symbols: vec!["BTC/USD".to_string()],
                channels: vec!["trade".to_string()],
            }],
        );

        let err = read(base.path(), &name).unwrap_err();
        assert!(matches!(err, SessionError::Rejected(_)));
        assert!(err.to_string().contains("record-duckdb"), "got: {err}");
    }

    #[cfg(feature = "duckdb")]
    #[test]
    fn dual_backend_session_prefers_duckdb_and_never_duplicates() {
        use kraken_recording::DuckdbSink;

        let base = tempfile::tempdir().unwrap();
        let name = provision(
            base.path(),
            vec![
                RecordingRef {
                    backend: RecordingBackend::Duckdb,
                    file: "market.duckdb".to_string(),
                    schema_version: schema::SCHEMA_VERSION.to_string(),
                    symbols: vec!["BTC/USD".to_string()],
                    channels: vec!["trade".to_string()],
                },
                jsonl_recording(),
            ],
        );
        write_market(base.path(), &name, &["2026-01-01T00:00:01.000000Z"]);
        let frame = kraken_core::ChannelMessage::parse(
            r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","side":"buy",
               "price":1.0,"qty":1.0,"ord_type":"market","trade_id":0,
               "timestamp":"2026-01-01T00:00:01.000000Z"}]}"#,
        )
        .unwrap();
        let duck_path = dir(base.path(), &name).join("market.duckdb");
        let mut duck = DuckdbSink::open(
            &duck_path,
            &RecordingDeclaration {
                source: schema::SOURCE.to_string(),
                symbols: vec!["BTC/USD".into()],
                channels: vec!["trade".into()],
                window_start: "2026-01-01T00:00:00Z".parse().unwrap(),
                cli_version: "test".into(),
            },
        )
        .unwrap();
        duck.record(std::slice::from_ref(&frame)).unwrap();
        duck.finalize(RecordingIntegrity::now()).unwrap();

        let timeline = read(base.path(), &name).unwrap();
        assert_eq!(timeline.events.len(), 1, "one copy, not one per backend");
        match &timeline.events[0].payload {
            EventPayload::Market(read_back) => assert_eq!(read_back, &frame),
            other => panic!("expected a market payload, got {other:?}"),
        }
    }
}
