//! Paced emission of timeline events in recorded order.
//!
//! Playback reproduces recorded gaps; analytics remain outside this module.

use crate::Result;
use chrono::{DateTime, Utc};
use kraken_session::timeline::event::TimelineEvent;
use std::io::Write;
use tokio::time::Instant;

/// Supported playback range: 100× slow motion through near-I/O-speed replay.
pub const MIN_SPEED: f64 = 0.01;
pub const MAX_SPEED: f64 = 1000.0;

/// Prevents damaged timestamps from parking playback for more than a year.
const MAX_STALL: std::time::Duration = std::time::Duration::from_secs(366 * 24 * 60 * 60);

/// An anchored virtual clock with absolute deadlines.
///
/// Absolute timing lets delayed sinks catch up instead of compounding drift.
#[derive(Debug, Clone, Copy)]
pub struct PlaybackClock {
    /// Wall-clock instant playback started (w₀).
    start: Instant,
    /// The first event's recorded instant (t₀).
    anchor: DateTime<Utc>,
    speed: f64,
}

impl PlaybackClock {
    pub fn anchored(anchor: DateTime<Utc>, speed: f64) -> Self {
        Self {
            start: Instant::now(),
            anchor,
            speed,
        }
    }

    /// Computes `(at − anchor)/speed`, saturating malformed timestamps.
    fn offset(&self, at: DateTime<Utc>) -> std::time::Duration {
        let recorded_us = (at - self.anchor).num_microseconds().unwrap_or(i64::MAX);
        if recorded_us <= 0 {
            return std::time::Duration::ZERO;
        }
        let scaled = recorded_us as f64 / 1_000_000.0 / self.speed;
        std::time::Duration::try_from_secs_f64(scaled).unwrap_or(std::time::Duration::MAX)
    }

    /// Waits until due; unsafe deadlines preserve order but skip pacing.
    ///
    /// Public for the playground's replay pump, which paces recorded frames
    /// through the live capture pipeline with this same clock.
    pub async fn wait_until_due(&self, at: DateTime<Utc>) {
        let offset = self.offset(at);
        if offset > MAX_STALL {
            tracing::warn!(%at, "replay deadline is over a year of wall clock away; emitting immediately");
            return;
        }
        match self.start.checked_add(offset) {
            Some(deadline) => tokio::time::sleep_until(deadline).await,
            None => {
                tracing::warn!(%at, "replay deadline overflows the clock; emitting immediately")
            }
        }
    }
}

/// The playback loop: wait for each event's deadline, write one rendered
/// line, flush, count. The only await is the sleep, so cancellation (a
/// caller's `select!` shutdown branch) can never drop a half-written line.
///
/// `render` is a pure transform (event → line, no trailing newline); sink
/// construction and locking stay at the call site — stdout belongs to the
/// caller for the whole playback. `emitted` is an out-parameter, not a
/// return, so the count survives cancellation and errors.
pub async fn replay_events(
    events: &[TimelineEvent],
    clock: PlaybackClock,
    out: &mut impl Write,
    render: impl Fn(&TimelineEvent) -> std::result::Result<String, serde_json::Error>,
    emitted: &mut u64,
) -> Result<()> {
    for event in events {
        clock.wait_until_due(event.at).await;
        let mut line = render(event)?;
        line.push('\n');
        if let Err(err) = out.write_all(line.as_bytes()).and_then(|()| out.flush()) {
            // A closed pipe (`kraken replay … | head`) ends a *finite*
            // playback cleanly — deliberately unlike a live-stream sink,
            // which must surface it.
            if err.kind() == std::io::ErrorKind::BrokenPipe {
                return Ok(());
            }
            return Err(err.into());
        }
        *emitted += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReplayError;
    use kraken_recording::parse_instant;
    use kraken_session::decision::{Decision, DecisionKind};
    use std::time::Duration;

    fn json_render(event: &TimelineEvent) -> std::result::Result<String, serde_json::Error> {
        serde_json::to_string(event)
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

    fn three_events_with_2s_and_3s_gaps() -> Vec<TimelineEvent> {
        vec![
            decision_event("2026-01-01T00:00:00Z", 1),
            decision_event("2026-01-01T00:00:02Z", 2),
            decision_event("2026-01-01T00:00:05Z", 3),
        ]
    }

    /// Records the virtual instant of every line written, so pacing tests
    /// assert on the paused clock instead of real time.
    #[derive(Default)]
    struct InstantWriter {
        instants: Vec<Instant>,
        bytes: Vec<u8>,
    }

    impl Write for InstantWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.instants.push(Instant::now());
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Fails every write past the first `ok_writes` with the given kind.
    struct FailingWriter {
        ok_writes: usize,
        kind: std::io::ErrorKind,
    }

    impl Write for FailingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.ok_writes == 0 {
                return Err(std::io::Error::from(self.kind));
            }
            self.ok_writes -= 1;
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn offset_scales_gaps_by_speed() {
        let anchor = parse_instant("2026-01-01T00:00:00Z").unwrap();
        let at = parse_instant("2026-01-01T00:00:02Z").unwrap();
        for (speed, expected) in [
            (1.0, Duration::from_secs(2)),
            (10.0, Duration::from_millis(200)),
            (0.5, Duration::from_secs(4)),
        ] {
            let clock = PlaybackClock::anchored(anchor, speed);
            assert_eq!(clock.offset(at), expected, "speed {speed}");
        }
    }

    #[test]
    fn offset_keeps_submillisecond_precision() {
        // recv_ts and wire timestamps carry microseconds; pacing must not
        // floor them to whole milliseconds.
        let anchor = parse_instant("2026-01-01T00:00:00Z").unwrap();
        let at = anchor + chrono::Duration::microseconds(400);
        for (speed, expected) in [
            (1.0, Duration::from_micros(400)),
            (0.01, Duration::from_micros(40_000)),
        ] {
            let clock = PlaybackClock::anchored(anchor, speed);
            assert_eq!(clock.offset(at), expected, "speed {speed}");
        }
    }

    #[test]
    fn pre_anchor_stamps_clamp_to_zero_offset() {
        let anchor = parse_instant("2026-01-01T00:00:05Z").unwrap();
        let clock = PlaybackClock::anchored(anchor, 1.0);
        let earlier = parse_instant("2026-01-01T00:00:00Z").unwrap();
        assert_eq!(clock.offset(earlier), Duration::ZERO);
    }

    #[test]
    fn extreme_gap_offset_scales_without_panicking() {
        let anchor = parse_instant("2026-01-01T00:00:00Z").unwrap();
        let clock = PlaybackClock::anchored(anchor, MIN_SPEED);
        // ~100k years of recorded gap (near chrono's date ceiling) at 100×
        // slow motion: a finite, huge offset — the point is no panic.
        let far = anchor + chrono::Duration::days(36_500_000);
        assert!(clock.offset(far) > Duration::from_secs(100_000_000_000_000));
    }

    #[tokio::test(start_paused = true)]
    async fn astronomic_deadline_emits_immediately_instead_of_parking() {
        let anchor = parse_instant("2026-01-01T00:00:00Z").unwrap();
        let clock = PlaybackClock::anchored(anchor, MIN_SPEED);
        let far = anchor + chrono::Duration::days(36_500_000);
        let before = Instant::now();
        clock.wait_until_due(far).await;
        // Past MAX_STALL the wait degrades to immediate emission: the paused
        // clock must not have moved at all.
        assert_eq!(Instant::now(), before, "no virtual time consumed");
    }

    #[tokio::test(start_paused = true)]
    async fn replay_emits_on_the_recorded_gaps_at_speed_1() {
        let events = three_events_with_2s_and_3s_gaps();
        let clock = PlaybackClock::anchored(events[0].at, 1.0);
        let start = clock.start;
        let mut out = InstantWriter::default();
        let mut emitted = 0;

        replay_events(&events, clock, &mut out, json_render, &mut emitted)
            .await
            .unwrap();

        assert_eq!(emitted, 3);
        let offsets: Vec<Duration> = out.instants.iter().map(|i| *i - start).collect();
        assert_eq!(
            offsets,
            [
                Duration::ZERO,
                Duration::from_secs(2),
                Duration::from_secs(5)
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn faster_speed_compresses_the_gaps() {
        let events = three_events_with_2s_and_3s_gaps();
        let clock = PlaybackClock::anchored(events[0].at, 10.0);
        let start = clock.start;
        let mut out = InstantWriter::default();
        let mut emitted = 0;

        replay_events(&events, clock, &mut out, json_render, &mut emitted)
            .await
            .unwrap();

        let offsets: Vec<Duration> = out.instants.iter().map(|i| *i - start).collect();
        assert_eq!(
            offsets,
            [
                Duration::ZERO,
                Duration::from_millis(200),
                Duration::from_millis(500)
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn overdue_events_flush_back_to_back_after_a_stall() {
        // Deadlines are absolute (`start + offset`), so after a 10 s stall
        // every already-due event flushes at one instant — no re-pacing.
        let events = three_events_with_2s_and_3s_gaps();
        let clock = PlaybackClock::anchored(events[0].at, 1.0);
        let start = clock.start;
        tokio::time::advance(Duration::from_secs(10)).await;
        let mut out = InstantWriter::default();
        let mut emitted = 0;

        replay_events(&events, clock, &mut out, json_render, &mut emitted)
            .await
            .unwrap();

        let stalled_until = start + Duration::from_secs(10);
        assert!(
            out.instants.iter().all(|at| *at == stalled_until),
            "expected one shared instant, got {:?}",
            out.instants
        );
    }

    #[tokio::test(start_paused = true)]
    async fn write_error_stops_the_replay() {
        let events = three_events_with_2s_and_3s_gaps();
        let clock = PlaybackClock::anchored(events[0].at, MAX_SPEED);
        let mut out = FailingWriter {
            ok_writes: 1,
            kind: std::io::ErrorKind::StorageFull,
        };
        let mut emitted = 0;

        let err = replay_events(&events, clock, &mut out, json_render, &mut emitted)
            .await
            .unwrap_err();

        assert!(matches!(err, ReplayError::Emit(_)), "got: {err:?}");
        assert_eq!(emitted, 1, "no further emissions after the failure");
    }

    #[tokio::test(start_paused = true)]
    async fn broken_pipe_is_a_clean_end_not_an_error() {
        let events = three_events_with_2s_and_3s_gaps();
        let clock = PlaybackClock::anchored(events[0].at, MAX_SPEED);
        let mut out = FailingWriter {
            ok_writes: 1,
            kind: std::io::ErrorKind::BrokenPipe,
        };
        let mut emitted = 0;

        replay_events(&events, clock, &mut out, json_render, &mut emitted)
            .await
            .unwrap();

        assert_eq!(emitted, 1, "the pipe closed after the first line");
    }
}
