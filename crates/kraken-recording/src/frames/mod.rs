//! The market-frame domain: capture contracts and storage backends.
//!
//! Extends the domain-free core with what a market capture adds on top of a
//! plain log: the finalize stamp ([`CaptureSink`]), the self-description
//! handed back before any frame decodes ([`CaptureSource`]), and the merged
//! read unit ([`MarketEvent`]). Two backends implement the pair —
//! [`jsonl`] ([`TapeSink`]/[`TapeSource`]) and the feature-gated
//! `duckdb` (plain name: a link would break the doc build without the
//! feature) — and must yield identical event sequences for one capture
//! (pinned by the backend-equivalence tests).

use crate::{
    error::{Error, Result},
    frames::capture::{RecordingIntegrity, RecordingManifest},
    lock::FileLock,
    sink::Sink,
    source::Source,
    time::parse_instant,
};
use chrono::{DateTime, Utc};
use kraken_core::{ChannelMessage, endpoint::Endpoint};
use std::path::Path;

pub mod capture;
pub mod jsonl;

#[cfg(feature = "duckdb")]
pub mod duckdb;

#[cfg(feature = "duckdb")]
pub use duckdb::{DuckdbSink, DuckdbSource};
pub use jsonl::{TapeSink, TapeSource};

/// A market-stream destination: a frame [`Sink`] a driver can finalize with
/// the capture summary. Consumed by value — drivers are generic over the
/// concrete sink, never `Box<dyn CaptureSink>`.
pub trait CaptureSink: Sink<Item = ChannelMessage> {
    /// The stream reconnected on `endpoint` mid-capture; the venue re-sends
    /// full snapshots next. Default no-op: durable backends hold frames only
    /// (the reconnect count lands in the finalize summary), while a live echo
    /// overrides this to tell its consumer that accumulated state is stale.
    fn record_reconnect(&mut self, _endpoint: Endpoint) -> Result<()> {
        Ok(())
    }

    /// End the stream: stamp `summary` into the destination's metadata and
    /// release the destination. The default is for destinations with no
    /// metadata to stamp (a live echo): the summary is dropped and only the
    /// classical [`close`](Sink::close) runs.
    fn finalize(self, _summary: RecordingIntegrity) -> Result<()>
    where
        Self: Sized,
    {
        self.close()
    }
}

/// The read side of a market capture — the mirror of [`CaptureSink`]: where
/// the sink ends a stream by stamping the capture's self-description, the
/// source starts a read by handing the same description back. Callers check
/// it (version, summary) before consuming [`read`](Source::read), so no
/// frame is decoded under the wrong contract.
pub trait CaptureSource: Source<Item = MarketEvent> {
    /// The capture's self-description, exactly as stamped. `summary: None`
    /// means the last run never finalized (completeness unknown); a missing
    /// or incompatible stamp is an error, damage fails loud.
    fn capture(&self) -> Result<RecordingManifest>;
}

/// One market frame read back from a recording — the storage layer's read
/// unit, wrapped into its consumer's vocabulary (e.g. a session timeline).
/// `at` is the merge instant — the exchange `event_ts`, in every backend —
/// already clamped non-decreasing in capture order by the producing source
/// ([`clamp_non_decreasing`]); `seq` is the writer's frame order, the sort
/// and tiebreak within the track.
#[derive(Debug)]
pub struct MarketEvent {
    pub at: DateTime<Utc>,
    pub seq: i64,
    pub frame: ChannelMessage,
}

/// A market frame's merge instant — its exchange `event_ts`, the one rule
/// behind [`MarketEvent::at`]. Shared so no backend can drift to a different
/// timestamp field.
pub(crate) fn event_instant(frame: &ChannelMessage) -> Option<DateTime<Utc>> {
    frame.event_ts().and_then(parse_instant)
}

/// Clamp merge instants non-decreasing in capture order: wire `event_ts` is
/// not monotone (OHLC can report `interval_begin`, wall clocks step back),
/// and replay must never reorder against capture. Shared by both market
/// sources so the merged timeline orders identically per backend.
pub(crate) fn clamp_non_decreasing(events: &mut [MarketEvent]) {
    let mut last: Option<DateTime<Utc>> = None;
    for event in events {
        if let Some(prev) = last {
            event.at = event.at.max(prev);
        }
        last = Some(event.at);
    }
}

/// Take the reader posture on a tape: a *shared* lock, so a starting
/// recorder gets a clean "in use" rejection instead of an engine error,
/// concurrent readers coexist, and the exclusive-only recorder-liveness
/// probe never mistakes a long read for a live recorder.
pub(crate) fn shared_capture_lock(path: &Path) -> Result<FileLock> {
    FileLock::acquire_shared(path, "tape").map_err(|e| match e {
        Error::Rejected(_) => Error::Rejected(
            "the tape is in use — the recorder is still running; stop the session first".into(),
        ),
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_core::{ChannelData, subscribe::message::MessageType};
    use proptest::prelude::*;

    fn event_at(at: DateTime<Utc>, seq: i64) -> MarketEvent {
        MarketEvent {
            at,
            seq,
            frame: ChannelMessage {
                body: ChannelData::Trade(vec![]),
                message_type: MessageType::Update,
                sequence: None,
            },
        }
    }

    proptest! {
        /// The clamp is exactly the running maximum: `out[i] == max(in[..=i])`.
        /// This single postcondition subsumes non-decreasing, never-rewinding,
        /// and identity-on-ordered-input — and rules out an over-clamping
        /// implementation (e.g. `prev + ε`) the weaker properties would pass.
        #[test]
        fn clamp_is_the_running_maximum_of_its_input(
            offsets in proptest::collection::vec(0i64..=100_000, 0..32),
        ) {
            let base = DateTime::<Utc>::from_timestamp(1_760_000_000, 0).unwrap();
            let inputs: Vec<_> = offsets
                .iter()
                .map(|&s| base + chrono::Duration::seconds(s))
                .collect();
            let mut events: Vec<_> = inputs
                .iter()
                .enumerate()
                .map(|(i, &at)| event_at(at, i as i64))
                .collect();

            clamp_non_decreasing(&mut events);
            let mut running_max: Option<DateTime<Utc>> = None;
            for (event, &input) in events.iter().zip(&inputs) {
                running_max = Some(running_max.map_or(input, |m| m.max(input)));
                prop_assert_eq!(Some(event.at), running_max);
            }
        }
    }
}
