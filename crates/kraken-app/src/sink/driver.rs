//! The market-stream driver: feeds the WS broadcast into a frame sink.
//!
//! The glue between the async client and the synchronous sink layer: it owns
//! the blocking thread, batch coalescing, lag accounting, and non-data event
//! routing. Deliberately outside the [`Sink`](kraken_recording::Sink) trait —
//! the trait is generic over the entry type; this driver is specific to
//! [`ChannelMessage`](kraken_core::ChannelMessage) streams
//! and the [`Event`] lifecycle around them.

use crate::errors::{KrakenError, Result};
use chrono::Utc;
use kraken_core::{MethodReply, endpoint::Endpoint};
use kraken_recording::{CaptureSink, RecordingIntegrity};
use kraken_ws::Event;
use tokio::{sync::broadcast, task::JoinHandle};

/// Data frames coalesced into one [`kraken_recording::Sink::record`] call — one
/// stdout flush, one DB transaction. A quiet feed still yields a one-frame
/// batch immediately.
const SINK_BATCH_SIZE: usize = 256;

/// Drive the client's event stream into `sink` on a dedicated blocking thread:
/// coalesce data frames into [`record`](kraken_recording::Sink::record), render
/// acks/lifecycle to stderr, deliver reconnects to the sink via
/// [`record_reconnect`](CaptureSink::record_reconnect), end when the broadcast
/// closes. The handle resolves to the first sink error, else the
/// [`finalize`](CaptureSink::finalize) result.
pub(crate) fn run<S>(mut sink: S, mut events: broadcast::Receiver<Event>) -> JoinHandle<Result<()>>
where
    S: CaptureSink + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let mut fatal: Option<KrakenError> = None;
        // Stamped into the tape at finalize, so loss and resets are observable.
        let mut stats = CaptureStats::default();
        // Reused across batches: `record` only borrows it, so one allocation
        // serves the whole session.
        let mut batch = Vec::with_capacity(SINK_BATCH_SIZE);
        loop {
            match events.blocking_recv() {
                Ok(Event::Message(data)) => {
                    // Coalesce this and any already-queued data into one record
                    // call; a non-data event pulled mid-coalesce is held and
                    // routed after the record so its wire order is preserved.
                    batch.clear();
                    batch.push(data);
                    let mut deferred = None;
                    while batch.len() < SINK_BATCH_SIZE {
                        match events.try_recv() {
                            Ok(Event::Message(more)) => batch.push(more),
                            // Hold a non-data event and stop coalescing.
                            Ok(other) => {
                                deferred = Some(other);
                                break;
                            }
                            // Nothing buffered right now — flush what we have.
                            Err(broadcast::error::TryRecvError::Empty) => break,
                            // All senders gone — flush; the outer
                            // blocking_recv() then drains to Closed and finalizes.
                            Err(broadcast::error::TryRecvError::Closed) => break,
                            // The ring overwrote frames while we coalesced;
                            // warn, then flush. tokio has reset our cursor to
                            // the oldest retained value, so the next
                            // blocking_recv() resumes with no extra loss.
                            Err(broadcast::error::TryRecvError::Lagged(dropped)) => {
                                warn_lagged(dropped);
                                stats.note_lag(dropped);
                                break;
                            }
                        }
                    }
                    // Every arm above breaks the inner loop only, so the batch
                    // is always recorded before the stream finalizes.
                    if let Err(e) = sink.record(&batch) {
                        // Route the held event first so a pending diagnostic
                        // is not lost on the error path. The record failure
                        // stays the fatal error; a marker failure on top of it
                        // is shadowed but must not vanish silently.
                        if let Some(event) = deferred
                            && let Err(error) = route_and_tally(event, &mut stats, &mut sink)
                        {
                            tracing::warn!(%error, "reconnect marker also failed after a record error");
                        }
                        fatal = Some(e.into());
                        break;
                    }
                    if let Some(event) = deferred
                        && let Err(e) = route_and_tally(event, &mut stats, &mut sink)
                    {
                        fatal = Some(e.into());
                        break;
                    }
                }
                Ok(other) => {
                    if let Err(e) = route_and_tally(other, &mut stats, &mut sink) {
                        fatal = Some(e.into());
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(dropped)) => {
                    warn_lagged(dropped);
                    stats.note_lag(dropped);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        let finalized = sink.finalize(stats.summary()).map_err(KrakenError::from);
        match fatal {
            // The record failure is the error the caller acts on; a finalize failure
            // on top of it is shadowed but must not vanish silently.
            Some(fatal) => {
                if let Err(error) = finalized {
                    tracing::warn!(%error, "sink finalize also failed after a record error");
                }
                Err(fatal)
            }
            None => finalized,
        }
    })
}

/// Warn that the broadcast dropped frames this receiver never read. Shared by
/// both receive paths so the two diagnostics can't drift.
fn warn_lagged(dropped: u64) {
    tracing::warn!(dropped, "event stream lagged");
}

/// Per-session capture statistics tallied by [`run`] and stamped into the
/// tape at finalize.
#[derive(Default)]
struct CaptureStats {
    /// Frames the broadcast ring overwrote before this recorder read them.
    events_dropped: u64,
    /// Inbound frames the connection dropped as unparseable.
    frames_unparsed: u64,
    /// Reconnects observed mid-session.
    reconnects: u64,
    /// Endpoints that have connected at least once, so only a *re*connect is
    /// counted. At most one entry per endpoint (≤3), so a linear `Vec`
    /// beats a hashing set here.
    seen: Vec<Endpoint>,
}

impl CaptureStats {
    /// Account for `dropped` frames the broadcast ring discarded (saturating).
    fn note_lag(&mut self, dropped: u64) {
        self.events_dropped = self.events_dropped.saturating_add(dropped);
    }

    /// A `Connected` for an endpoint already seen is a reconnect (the first
    /// connect per endpoint is not), returned so the caller can announce it
    /// to the sink; a `ParseFailure` is a capture hole to count.
    fn observe(&mut self, event: &Event) -> Option<Endpoint> {
        match event {
            Event::Connected(endpoint) => {
                if self.seen.contains(endpoint) {
                    self.reconnects = self.reconnects.saturating_add(1);
                    return Some(*endpoint);
                }
                self.seen.push(*endpoint);
                None
            }
            Event::ParseFailure(_) => {
                self.frames_unparsed = self.frames_unparsed.saturating_add(1);
                None
            }
            _ => None,
        }
    }

    /// The end-of-session summary, stamped at the current instant.
    fn summary(&self) -> RecordingIntegrity {
        RecordingIntegrity {
            window_end: Utc::now(),
            events_dropped: self.events_dropped,
            frames_unparsed: self.frames_unparsed,
            reconnect_count: self.reconnects,
        }
    }
}

/// Tally a non-data event's reconnect (if any), route it, then announce a
/// reconnect to the sink — one helper so every non-data path counts, routes,
/// and announces identically. A marker failure is the sink's failure, fatal
/// exactly like a record error.
fn route_and_tally<S: CaptureSink>(
    event: Event,
    stats: &mut CaptureStats,
    sink: &mut S,
) -> kraken_recording::Result<()> {
    let reconnected = stats.observe(&event);
    route_non_data(event);
    match reconnected {
        Some(endpoint) => sink.record_reconnect(endpoint),
        None => Ok(()),
    }
}

/// Route a non-data event: a failed ack and lifecycle go to stderr; a
/// successful ack is silent unless it carries warnings. (`Message` is handled
/// on the batch path.)
fn route_non_data(event: Event) {
    match event {
        Event::Message(_) => {}
        Event::Ack(resp) => {
            // Subscribe and unsubscribe acks carry the same echo body; both surface
            // their warnings.
            if let MethodReply::Subscribe(Some(ack)) | MethodReply::Unsubscribe(Some(ack)) =
                &resp.reply
                && let Some(warnings) = ack.warnings.as_ref().filter(|w| !w.is_empty())
            {
                let joined = warnings.join("; ");
                tracing::warn!(method = %resp.method(), warnings = %joined, "ack warning");
            }
        }
        Event::ApiError(resp) => {
            let error = resp.error.as_deref().unwrap_or("unknown error");
            tracing::warn!(method = %resp.method(), %error, "api error");
        }
        Event::Connected(endpoint) => {
            tracing::debug!(%endpoint, "socket connected");
        }
        Event::Disconnected(endpoint) => {
            tracing::debug!(%endpoint, "socket disconnected");
        }
        // The connection actor already warned and the monitor counts it.
        Event::ConnectFailed(endpoint) => {
            tracing::debug!(%endpoint, "socket connect attempt failed");
        }
        // Keepalive: liveness for the monitor, nothing to record or render here.
        Event::Heartbeat => {}
        // The connection actor already warned with the parse error; the count lands
        // in the capture summary via `CaptureStats`.
        Event::ParseFailure(endpoint) => {
            tracing::debug!(%endpoint, "unparseable frame dropped");
        }
        // Terminal give-up: this endpoint is done for the session while any siblings
        // keep streaming; the pump fails the session with the cause once the stream
        // ends, and the actor already warned with it — so lifecycle-level here.
        Event::Aborted { endpoint, .. } => {
            tracing::debug!(%endpoint, "connection aborted; endpoint is done for this session");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_core::ChannelMessage;
    use kraken_recording::Sink;
    use std::{
        cell::Cell,
        rc::Rc,
        sync::{Arc, Mutex},
    };

    struct Probe {
        closed: Rc<Cell<bool>>,
    }

    impl Sink for Probe {
        type Item = ChannelMessage;

        fn record(&mut self, _batch: &[ChannelMessage]) -> kraken_recording::Result<()> {
            Ok(())
        }

        fn close(self) -> kraken_recording::Result<()> {
            self.closed.set(true);
            Ok(())
        }
    }

    impl CaptureSink for Probe {}

    #[test]
    fn parse_failures_are_counted_into_the_summary() {
        let mut stats = CaptureStats::default();
        stats.observe(&Event::ParseFailure(Endpoint::Public));
        stats.observe(&Event::ParseFailure(Endpoint::Auth));
        assert_eq!(stats.summary().frames_unparsed, 2);
    }

    /// A `Send` sink that logs every reconnect marker it receives, so run-level
    /// tests can assert exactly when the driver announces a reconnect.
    struct MarkerProbe {
        markers: Arc<Mutex<Vec<Endpoint>>>,
        fail_marker: bool,
    }

    impl Sink for MarkerProbe {
        type Item = ChannelMessage;

        fn record(&mut self, _batch: &[ChannelMessage]) -> kraken_recording::Result<()> {
            Ok(())
        }
    }

    impl CaptureSink for MarkerProbe {
        fn record_reconnect(&mut self, endpoint: Endpoint) -> kraken_recording::Result<()> {
            if self.fail_marker {
                return Err(kraken_recording::Error::Rejected("marker sink down".into()));
            }
            self.markers.lock().unwrap().push(endpoint);
            Ok(())
        }
    }

    /// Queue `events` on a closed broadcast and drain them through [`run`]:
    /// deterministic delivery order, no live socket, no sleeping.
    async fn drain(sink: MarkerProbe, events: Vec<Event>) -> Result<()> {
        let (tx, rx) = broadcast::channel(events.len().max(1));
        for event in events {
            tx.send(event).unwrap();
        }
        drop(tx);
        run(sink, rx).await.unwrap()
    }

    fn trade_frame() -> ChannelMessage {
        ChannelMessage::parse(
            r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","side":"buy",
               "price":50000.1,"qty":0.5,"ord_type":"market","trade_id":1,
               "timestamp":"2026-01-01T00:00:00.000000Z"}]}"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_reconnect_delivers_the_marker_to_the_sink() {
        let markers = Arc::new(Mutex::new(Vec::new()));
        let sink = MarkerProbe {
            markers: Arc::clone(&markers),
            fail_marker: false,
        };
        drain(
            sink,
            vec![
                Event::Connected(Endpoint::Public),
                Event::Connected(Endpoint::Public),
            ],
        )
        .await
        .unwrap();
        assert_eq!(*markers.lock().unwrap(), vec![Endpoint::Public]);
    }

    #[tokio::test]
    async fn a_first_connect_emits_no_marker() {
        // Two endpoints connecting for the first time: neither is a reconnect.
        let markers = Arc::new(Mutex::new(Vec::new()));
        let sink = MarkerProbe {
            markers: Arc::clone(&markers),
            fail_marker: false,
        };
        drain(
            sink,
            vec![
                Event::Connected(Endpoint::Public),
                Event::Connected(Endpoint::Auth),
            ],
        )
        .await
        .unwrap();
        assert!(markers.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_reconnect_held_mid_batch_still_delivers_the_marker() {
        // The deferred-event path: the reconnect is pulled while coalescing a
        // data batch, held, and must still reach the sink after the record.
        let markers = Arc::new(Mutex::new(Vec::new()));
        let sink = MarkerProbe {
            markers: Arc::clone(&markers),
            fail_marker: false,
        };
        drain(
            sink,
            vec![
                Event::Connected(Endpoint::Public),
                Event::Message(trade_frame()),
                Event::Connected(Endpoint::Public),
            ],
        )
        .await
        .unwrap();
        assert_eq!(*markers.lock().unwrap(), vec![Endpoint::Public]);
    }

    #[tokio::test]
    async fn a_marker_failure_is_fatal_like_a_record_failure() {
        let sink = MarkerProbe {
            markers: Arc::new(Mutex::new(Vec::new())),
            fail_marker: true,
        };
        let result = drain(
            sink,
            vec![
                Event::Connected(Endpoint::Public),
                Event::Connected(Endpoint::Public),
            ],
        )
        .await;
        assert!(matches!(result, Err(KrakenError::Validation(_))));
    }

    #[test]
    fn default_finalize_still_closes_the_sink() {
        // A destination with no metadata to stamp must not skip the classical
        // close (and its flush) just because the summary has nowhere to go.
        let closed = Rc::new(Cell::new(false));
        let sink = Probe {
            closed: Rc::clone(&closed),
        };

        sink.finalize(RecordingIntegrity::now()).unwrap();
        assert!(closed.get());
    }
}
