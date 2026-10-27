//! Command adapter for [`kraken_replay::replay_events`].

use crate::{
    cli::AppContext,
    errors::Result,
    output::{self, OutputFormat, summary::Summarize},
    stream,
};
use chrono::Utc;
use kraken_contract::replay::ReplayCommand;
use kraken_recording::CaptureState;
use kraken_replay::{PlaybackClock, replay_events};
use kraken_session::timeline::{SessionTimeline, event::TimelineEvent};

pub(crate) async fn execute(cmd: &ReplayCommand, ctx: &AppContext) -> Result<()> {
    super::session::ensure_session_replayable(ctx, &cmd.session)?;
    let (session_id, session) = super::session::read_session_window(ctx, &cmd.session)?;
    eprintln!(
        "{}",
        started_line(&session_id.to_string(), cmd.speed, &session)
    );

    let mut emitted: u64 = 0;
    let result = match session.events.first() {
        // A provisioned session with nothing recorded yet replays as an
        // empty stream, not an error — started/summary still bracket it.
        None => Ok(()),
        Some(first) => {
            let clock = PlaybackClock::anchored(first.at, cmd.speed);
            // `Stdout`, not a held `StdoutLock`: the engine writes each line
            // (newline included) in one `write_all`, which takes the internal
            // lock for that call, so NDJSON cannot be corrupted mid-line
            // anyway. Holding the guard across the pacing sleeps would make
            // this future non-`Send`, and every command shares one future type
            // through `Execute` — including the MCP host's, which requires
            // `Send`.
            let mut out = std::io::stdout();
            let render = |event: &TimelineEvent| render_line(ctx.format, event);
            tokio::select! {
                result = replay_events(&session.events, clock, &mut out, render, &mut emitted) => {
                    result.map_err(Into::into)
                }
                () = stream::shutdown_signal() => Ok(()),
            }
        }
    };
    eprintln!("{}", summary_line(emitted));
    result
}

/// Render one event for `format`: JSON is the timeline's pinned wire shape
/// emitted verbatim; table one `stream_line` per event, mirroring
/// [`render_data`](crate::sink::stdout) for live streams. Pure — the
/// playback engine owns writing and flushing.
fn render_line(
    format: OutputFormat,
    event: &TimelineEvent,
) -> std::result::Result<String, serde_json::Error> {
    match format {
        OutputFormat::Json => serde_json::to_string(event),
        OutputFormat::Table => {
            let at = kraken_recording::format_instant(event.at);
            let label = event.track_label();
            Ok(output::table::stream_line(&[
                ("at", &at),
                ("track", &label),
                ("data", &event.payload.summary()),
            ]))
        }
    }
}

/// The stderr lifecycle notice announcing playback, mirroring `record`'s
/// started/summary convention (src/commands/record.rs) — stdout stays
/// reserved for the events themselves.
fn started_line(run: &str, speed: f64, session: &SessionTimeline) -> String {
    serde_json::json!({
        "event": "replay",
        "type": "started",
        "ts": Utc::now(),
        "session": run,
        "events": session.events.len(),
        "speed": speed,
        "capture": capture_field(session),
    })
    .to_string()
}

/// The started line's capture health: the replay must say which of the
/// three [`CaptureState`]s it plays back rather than pretend a hole-free
/// record.
fn capture_field(session: &SessionTimeline) -> serde_json::Value {
    match CaptureState::of(session.capture.as_ref()) {
        CaptureState::Absent => serde_json::json!({ "state": "none" }),
        CaptureState::Unfinalized(_) => serde_json::json!({ "state": "unfinalized" }),
        CaptureState::Finalized(report, summary) => serde_json::json!({
            "state": "finalized",
            "window_start": report.meta.window_start,
            "window_end": summary.window_end,
            "events_dropped": summary.events_dropped,
            "reconnect_count": summary.reconnect_count,
        }),
    }
}

fn summary_line(emitted: u64) -> String {
    serde_json::json!({
        "event": "replay",
        "type": "summary",
        "ts": Utc::now(),
        "emitted": emitted,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{
        decision::{Decision, DecisionKind},
        timeline::fixtures,
    };
    use clap::Parser;
    use kraken_recording::{
        RecordingDeclaration, RecordingIntegrity, RecordingManifest, parse_instant,
    };
    use kraken_replay::{MAX_SPEED, MIN_SPEED};

    /// Parse wrapper: `ReplayCommand` is `clap::Args`, so tests flatten it
    /// into a throwaway parser the way `lib.rs` flattens it into the CLI.
    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        cmd: ReplayCommand,
    }

    fn speed_of(args: &[&str]) -> std::result::Result<f64, clap::Error> {
        TestCli::try_parse_from(args).map(|cli| cli.cmd.speed)
    }

    fn decision_event(ts: &str, seq: i64) -> TimelineEvent {
        TimelineEvent::decision(
            parse_instant(ts).unwrap(),
            seq,
            Decision {
                timestamp: ts.parse().unwrap(),
                kind: DecisionKind::Skip,
                symbol: None,
                reason: "pin".to_string(),
                order_id: None,
            },
        )
    }

    #[test]
    fn speed_accepts_the_bounded_range_and_defaults_to_real_time() {
        assert_eq!(speed_of(&["t"]).unwrap(), 1.0);
        assert_eq!(speed_of(&["t", "--speed", "0.01"]).unwrap(), MIN_SPEED);
        assert_eq!(speed_of(&["t", "--speed", "1"]).unwrap(), 1.0);
        assert_eq!(speed_of(&["t", "--speed", "1000"]).unwrap(), MAX_SPEED);
    }

    #[test]
    fn speed_rejects_zero_negative_nonfinite_and_out_of_range() {
        for bad in ["0", "-1", "inf", "nan", "0.009", "1001", "abc"] {
            assert!(speed_of(&["t", "--speed", bad]).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn json_line_matches_the_timeline_wire_shape() {
        let event = decision_event("2026-01-01T00:00:00Z", 3);
        let line = render_line(OutputFormat::Json, &event).unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["seq"], 3);
        assert_eq!(value["kind"], "skip");
        assert_eq!(value["reason"], "pin");
        assert!(value.get("payload").is_none(), "no discriminant wrapper");
    }

    #[test]
    fn table_line_shows_at_track_and_summary() {
        let event = decision_event("2026-01-01T00:00:00Z", 1);
        let line = render_line(OutputFormat::Table, &event).unwrap();
        for needle in ["at: 2026-01-01T00:00:00", "track: decision", "kind:skip"] {
            assert!(line.contains(needle), "{needle} missing in {line}");
        }
    }

    #[test]
    fn started_line_reports_the_three_capture_states() {
        let meta = RecordingDeclaration {
            source: "test".into(),
            symbols: vec!["BTC/USD".into()],
            channels: vec!["ticker".into()],
            window_start: "2026-01-01T00:00:00Z".parse().unwrap(),
            cli_version: "test".into(),
        };
        let report = |summary| RecordingManifest {
            schema_version: "3.0".into(),
            meta: meta.clone(),
            summary,
        };
        let line_for = |capture| {
            let session = SessionTimeline {
                events: Vec::new(),
                capture,
                manifest: fixtures::manifest(vec![]),
                newer_records_skipped: 0,
            };
            serde_json::from_str::<serde_json::Value>(&started_line("s1", 2.0, &session)).unwrap()
        };

        let none = line_for(None);
        assert_eq!(none["capture"]["state"], "none");
        assert_eq!(none["session"], "s1");
        assert_eq!(none["speed"], 2.0);

        let unfinalized = line_for(Some(report(None)));
        assert_eq!(unfinalized["capture"]["state"], "unfinalized");

        let finalized = line_for(Some(report(Some(RecordingIntegrity {
            window_end: "2026-01-01T01:00:00Z".parse().unwrap(),
            events_dropped: 7,
            reconnect_count: 1,
            ..RecordingIntegrity::now()
        }))));
        assert_eq!(finalized["capture"]["state"], "finalized");
        assert_eq!(finalized["capture"]["events_dropped"], 7);
        assert_eq!(finalized["capture"]["window_start"], "2026-01-01T00:00:00Z");
    }
}

#[cfg(test)]
mod bounds {
    /// `--speed` is validated at parse time by `kraken_contract`, which cannot
    /// depend on the replay engine. This is the tripwire for the two drifting:
    /// a bound changed in one place and not the other fails here rather than
    /// letting the CLI accept a speed the clock refuses.
    #[test]
    fn the_argument_bounds_match_the_playback_clocks() {
        let (min, max) = kraken_contract::replay::SPEED_BOUNDS;
        assert_eq!(min, kraken_replay::MIN_SPEED);
        assert_eq!(max, kraken_replay::MAX_SPEED);
    }
}
