//! Shared streaming lifecycle for subscriptions, sinks, monitoring, and shutdown.

use std::future::Future;
use std::num::NonZeroU64;
use std::time::Duration;

use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt};
use itertools::Itertools;
use kraken_core::error::Error as WsError;
use kraken_core::{SubscribableChannel, WsSubscription};
use kraken_recording::CaptureSink;
use kraken_ws::{AbortCause, Event, StreamClient};
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;

use crate::cli::AppContext;
use crate::errors::{KrakenError, Result};
use crate::sink::driver;

/// Independent cursors isolate slow consumers; lag drops old events instead of backpressure.
const EVENT_CAPACITY: usize = 1024;

/// Bounds teardown after the shutdown signal has already been consumed.
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// Subscribe `subs` and stream frames into `sink` until shutdown. `ready` is signalled
/// once, on the first subscribe acknowledgement the server sends back (the stream is
/// genuinely live); it is dropped unsignalled when the session ends without one — an
/// unreachable venue or a rejected subscribe never signals. The *effect* of readiness
/// (e.g. `run start` printing `session_started`) stays with the caller, which
/// races the receiver against this future. Callers that need no signal pass `None`.
pub(crate) async fn run<S>(
    subs: Vec<WsSubscription>,
    req_id: Option<NonZeroU64>,
    ready: Option<oneshot::Sender<()>>,
    sink: S,
    duration: Option<Duration>,
    ctx: &AppContext,
) -> Result<()>
where
    S: CaptureSink + Send + 'static,
{
    // Preserve channel classes so token failures can identify unaffected public data.
    let (private_channels, public_channels): (Vec<_>, Vec<_>) = subs
        .iter()
        .map(WsSubscription::channel)
        .partition(|channel| channel.endpoint().requires_token());

    let cfg = match ctx.ws_config() {
        Ok(cfg) => cfg,
        Err(e) => return fail_with_summary(e, ctx).await,
    };
    let policy = ctx.ws_reconnect_policy();

    let (client, stream) = match StreamClient::connect(subs, req_id, &cfg, policy).await {
        Ok(connected) => connected,
        Err(e) => {
            let stream_err = match e {
                WsError::TokenRejected(reason) => {
                    private_token_error(&private_channels, &public_channels, &reason)
                }
                other => other.into(),
            };
            return fail_with_summary(stream_err, ctx).await;
        }
    };

    // Report degraded starts distinctly; the connection planner owns skip policy.
    let skipped_private = client.skipped_for_credentials();
    tracing::info!(
        private = private_channels.len().saturating_sub(skipped_private),
        public = public_channels.len(),
        skipped_for_credentials = skipped_private,
        "stream started"
    );

    // Readiness waits for the first subscription acknowledgment, not socket creation.
    fan_out(
        bound_by_duration(stream, duration),
        |events| driver::run(sink, events),
        ctx.monitor.clone(),
        shutdown_signal(),
        ready,
        client,
    )
    .await
}

/// Bound a frame stream to a wall-clock `duration` (a `--duration` run),
/// ending it when the window elapses or the source finishes first.
///
/// The window bounds the *stream*, not the teardown: ending the stream drives
/// the same graceful finalize as a tape running out, a socket closing, or a
/// shutdown dropping the source, so the sinks flush the same way down every
/// path.
fn bound_by_duration<St>(stream: St, duration: Option<Duration>) -> BoxStream<'static, Event>
where
    St: Stream<Item = Event> + Send + 'static,
{
    match duration {
        Some(window) => stream.take_until(tokio::time::sleep(window)).boxed(),
        None => stream.boxed(),
    }
}

/// Drive a recorded tape through the same fan-out pipeline as a live
/// session: frames are paced by the tape's own timestamps at the
/// configured speed, re-stamped to session wall time at emission, and
/// re-recorded into the session's sinks. The session is self-contained
/// afterwards — timeline, score, and explain read it with zero changes, and
/// deleting the source tape can never orphan a verdict.
///
/// `source_guard` owns the source tape's shared read lock; it is dropped
/// at teardown like the live client, releasing the reader posture.
pub(crate) async fn run_replay<S>(
    frames: Vec<kraken_recording::MarketEvent>,
    map: kraken_session::manifest::SessionSource,
    ready: Option<oneshot::Sender<()>>,
    sink: S,
    duration: Option<Duration>,
    source_guard: impl Send + 'static,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()>
where
    S: CaptureSink + Send + 'static,
{
    let clock = kraken_replay::PlaybackClock::anchored(map.anchor, map.speed);
    let paced = futures_util::stream::iter(frames).then(move |event| {
        let map = map.clone();
        async move {
            clock.wait_until_due(event.at).await;
            Event::Message(restamp(event.frame, &map))
        }
    });
    // A replayed session is "subscribed" the moment its tape opens: the
    // synthetic ack fires the same readiness signal a live subscribe would.
    let stream = futures_util::stream::iter([synthetic_ack()?]).chain(paced);
    // ONE shutdown mechanism, shared with the live path: `fan_out` ends a
    // session by dropping its `source`. Live, that source is the WS client and
    // dropping it closes the socket, ending the stream. A replay's stream is
    // paced in memory, so dropping the read-lock guard alone can't end it — the
    // pump would sleep out the tape's remaining wall time and wedge the 30s
    // teardown (a Ctrl-C hanging with "the tape may need a manual check"). Tie
    // the paced stream's life to the source instead: end it when a stop signal
    // drops, and hand that signal in AS PART of `source`. Now `drop(source)`
    // ends the stream for live and replay alike, through the identical path.
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let stream = bound_by_duration(stream, duration)
        .take_until(async move {
            let _ = stop_rx.await;
        })
        .boxed();
    fan_out(
        stream,
        |events| driver::run(sink, events),
        None,
        shutdown,
        ready,
        (source_guard, stop_tx),
    )
    .await
}

/// Rewrite a frame's exchange timestamps onto the session clock. Both read
/// backends derive a frame's merge instant from these payload stamps, so
/// re-stamping at emission is what keeps the re-recorded tape interleaving
/// with the account journal under the ordinary `(at, track, seq)` order.
fn restamp(
    mut frame: kraken_core::ChannelMessage,
    map: &kraken_session::manifest::SessionSource,
) -> kraken_core::ChannelMessage {
    use kraken_core::subscribe::ChannelData;
    let retime = |ts: &mut String| {
        if let Ok(parsed) = ts.parse::<chrono::DateTime<chrono::Utc>>() {
            *ts = map
                .session_time(parsed)
                .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        }
    };
    match &mut frame.body {
        ChannelData::Ticker(entries) => entries.iter_mut().for_each(|e| retime(&mut e.timestamp)),
        ChannelData::Trade(entries) => entries.iter_mut().for_each(|e| retime(&mut e.timestamp)),
        ChannelData::Book(entries) => entries.iter_mut().for_each(|e| retime(&mut e.timestamp)),
        ChannelData::Ohlc(entries) => entries
            .iter_mut()
            .filter_map(|e| e.timestamp.as_mut())
            .for_each(retime),
        // Channels without a per-entry stamp keep their payload verbatim.
        _ => {}
    }
    frame
}

/// The readiness ack ([`pump_events`] keys on the first `Event::Ack`),
/// built through serde because `MethodReply` has no public constructor.
fn synthetic_ack() -> Result<Event> {
    let response = serde_json::from_value(serde_json::json!({
        "method": "subscribe",
        "success": true
    }))
    .map_err(|e| KrakenError::Parse(format!("static replay ack shape: {e}")))?;
    Ok(Event::Ack(response))
}

/// Resolve when the OS asks the process to stop: Ctrl-C (SIGINT) everywhere, plus
/// SIGTERM and SIGHUP on unix. SIGTERM is what `timeout(1)`, `docker stop`, systemd,
/// and Kubernetes send; SIGHUP is a closed controlling terminal. Catching them (not
/// only SIGINT) lets [`run`] take its graceful path — drop the client so the sink
/// finalizes (final flush / `CHECKPOINT`) and releases its advisory lock, and the session
/// window seals — instead of being terminated mid-session, which would leave an
/// un-checkpointed WAL, a stale `.lock`, and an unsealed (aborted) window behind. A
/// signal whose handler cannot be installed is dropped best-effort; Ctrl-C always
/// covers the session, so the capture is never aborted for want of a handler.
pub(crate) async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate())
            .inspect_err(|error| tracing::warn!(%error, "SIGTERM handler install failed"))
            .ok();
        let mut hup = signal(SignalKind::hangup())
            .inspect_err(|error| tracing::warn!(%error, "SIGHUP handler install failed"))
            .ok();
        // A dropped signal source parks forever so it never wins the select;
        // Ctrl-C still covers the session either way.
        let terminate = async {
            match term.as_mut() {
                Some(sig) => sig.recv().await,
                None => std::future::pending().await,
            }
        };
        let hangup = async {
            match hup.as_mut() {
                Some(sig) => sig.recv().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = ctrl_c() => {}
            // `None` (a signal stream closed) must not read as a shutdown.
            Some(()) = terminate => {}
            Some(()) = hangup => {}
        }
    }
    #[cfg(not(unix))]
    ctrl_c().await;
}

/// Resolve on Ctrl-C. A handler failure waits forever instead of resolving: resolving
/// would tear a healthy capture down as if an operator had interrupted it, while the
/// OS default disposition still lets SIGINT kill the process.
async fn ctrl_c() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "Ctrl-C handler failed; graceful shutdown via SIGINT is unavailable");
        std::future::pending::<()>().await;
    }
}

/// Build the auth error for a failed private-endpoint token mint (e.g. an invalid API
/// key). It names the private channels that need credentials and — when the stream also
/// has a public half — the public channels that don't, so the caller can either fix the
/// key or drop the private channels and keep streaming the rest.
///
/// `reason` is the raw upstream message (e.g. `EAPI:Invalid key`). The returned
/// [`KrakenError::Auth`] prepends "Authentication failed: ", so the message is phrased to
/// read as its continuation.
fn private_token_error(
    private: &[SubscribableChannel],
    public: &[SubscribableChannel],
    reason: &str,
) -> KrakenError {
    let join = |channels: &[SubscribableChannel]| channels.iter().join(", ");
    let mut message = format!(
        "the private channels ({}) require valid API credentials — minting the WebSocket \
         token failed: {reason}. Check KRAKEN_API_KEY/KRAKEN_API_SECRET",
        join(private),
    );
    if public.is_empty() {
        message.push('.');
    } else {
        message.push_str(&format!(
            ", or drop them to stream the public channels ({}), which need no authentication.",
            join(public),
        ));
    }
    KrakenError::Auth(message)
}

/// Fail the session while honoring the monitored-session contract: every monitored run ends
/// with the monitor's final `summary` line, even one that failed before a socket
/// opened — tooling tails stderr for it on every session. The session's own error stays in
/// front of any monitor failure.
async fn fail_with_summary(error: KrakenError, ctx: &AppContext) -> Result<()> {
    let monitor_result = close_out_monitor(ctx.monitor.clone()).await;
    keep_first(Err(error), monitor_result, "monitor")
}

/// Run the `--monitor` task against an already-closed broadcast, so it emits its final
/// zero-count `summary` line and exits at once. The path for a session that failed before
/// any socket opened: the monitored-session contract — every session ends with a summary — must
/// hold on the failure path too. No-op without `--monitor`.
async fn close_out_monitor(monitor_cfg: Option<crate::monitor::MonitorConfig>) -> Result<()> {
    let Some(cfg) = monitor_cfg else {
        return Ok(());
    };
    let (closed_tx, closed_rx) = broadcast::channel(1);
    drop(closed_tx);
    let handle = tokio::spawn(crate::monitor::run_monitor(closed_rx, cfg));
    await_monitor(Some(handle)).await
}

/// Await the `--monitor` task's final `summary` line and its exit status, if the monitor is
/// running. The caller must already have dropped the client so the broadcast is closed and
/// the task is winding down. A join failure (the task panicked or was cancelled) is
/// returned rather than discarded, so a crashed monitor never passes silently. `Ok(())`
/// when `--monitor` is off.
async fn await_monitor(monitor: Option<JoinHandle<()>>) -> Result<()> {
    let Some(handle) = monitor else {
        return Ok(());
    };
    handle.await.map_err(KrakenError::from)
}

/// Keep `primary` when it failed, folding `secondary` in when it is clean — the session's
/// own error always outranks a diagnostic side-channel's. Each error surfaces exactly once:
/// the survivor through the returned `Result` (rendered as the envelope), a masked
/// secondary through one log line here, at the point it is dropped.
fn keep_first(primary: Result<()>, secondary: Result<()>, secondary_stage: &str) -> Result<()> {
    if let (Err(_), Err(masked)) = (&primary, &secondary) {
        tracing::warn!(
            error = %masked,
            stage = secondary_stage,
            "teardown error masked by the session error"
        );
        return primary;
    }
    primary.and(secondary)
}

/// Fan the client's event `stream` out to the sink task and the optional monitor, drive
/// them until `shutdown` (or the sink resolving on its own), then tear the pipeline down.
///
/// One broadcast feeds two independent consumers, each on its own cursor — a slow one lags on
/// its receiver rather than back-pressuring the other. The pump is handed the *only* sender
/// (moved in, not cloned), so when the source stream ends it drops that sender and the
/// broadcast closes; both consumers then observe `Closed` and finalize. Retaining a second
/// sender in this scope would wedge teardown — the sink and monitor would await a close that
/// can never arrive.
///
/// `start_sink` is a factory ([`driver::run`] in production), not the sink itself, so the
/// subscribe-before-pump ordering stays in this scope and a test can wedge the sink task
/// without a blocking-pool thread (which would inhibit tokio's paused virtual time).
///
/// `ready` is signalled from the pump on the first subscribe ack — see [`pump_events`].
/// `source` is whatever owns the stream's producers (the client handle in production);
/// dropping it at teardown is what ends the stream, so the ordering is literal code here
/// rather than a hook whose timing would need documenting.
async fn fan_out<St>(
    stream: St,
    start_sink: impl FnOnce(broadcast::Receiver<Event>) -> JoinHandle<Result<()>>,
    monitor_cfg: Option<crate::monitor::MonitorConfig>,
    shutdown: impl Future<Output = ()>,
    ready: Option<oneshot::Sender<()>>,
    source: impl Send,
) -> Result<()>
where
    St: futures_util::Stream<Item = Event> + Unpin + Send + 'static,
{
    let (fanout_tx, _) = broadcast::channel(EVENT_CAPACITY);

    let monitor_task = monitor_cfg.map(|cfg| {
        let monitor_rx = fanout_tx.subscribe();
        tokio::spawn(crate::monitor::run_monitor(monitor_rx, cfg))
    });

    let mut sink_task = start_sink(fanout_tx.subscribe());

    // Spawn the pump LAST — broadcast receivers miss anything sent before they subscribe, so
    // every subscribe() above must precede this — and hand it the sole sender by moving
    // `fanout_tx` in rather than cloning. As the only sender, the pump closes the broadcast
    // when the stream ends, which is what lets the sink and monitor see `Closed` and finalize.
    let pump_task = tokio::spawn(pump_events(stream, fanout_tx, ready));

    tokio::pin!(shutdown);
    // Run until the first of: an OS shutdown signal, or the sink finishing first (a fatal write
    // error, or a source stream that ends on its own). Awaiting `&mut sink_task` is cancel-safe —
    // if shutdown wins, the spawned sink task keeps running and we resume the same handle below;
    // if the sink wins, we capture its result *here*, because a `JoinHandle` panics if polled
    // again after it has completed.
    let sink_done = tokio::select! {
        _ = &mut shutdown => None,
        result = &mut sink_task => Some(result),
    };

    // Halt the source: dropping the client makes the connection actors close their sockets
    // and the stream ends; the pump forwards any buffered tail, then drops the sole broadcast
    // sender — closing it so the sink and monitor drain and finalize.
    drop(source);

    // Drain the three stages concurrently, each bounded by TEARDOWN_TIMEOUT — the signal
    // listener is consumed by now, so the window an operator's repeat Ctrl-C falls on deaf
    // ears is one timeout, never their sum. Causality flows through the broadcast, not the
    // await order: the pump ends when the stream ends (the source was just dropped) and its
    // sole sender drops, which is what lets the sink and monitor observe `Closed` and
    // finalize. The sink reuses its result if it won the race above; a timeout abandons the
    // task, it cannot cancel a blocking close — `main` bounds the runtime's own shutdown for
    // that. A pump that died (panicked or was aborted) ended the broadcast *early* — the
    // consumers then finalized on a silently truncated stream — so its join failure is a
    // session error, not a log line.
    let (stream_result, pump_result, monitor_result) = tokio::join!(
        async {
            match sink_done {
                Some(join) => join.map_err(KrakenError::from).and_then(|result| result),
                None => drain("sink finalization", flat_join(sink_task)).await,
            }
        },
        drain("event pump", flat_join(pump_task)),
        drain("monitor", await_monitor(monitor_task)),
    );
    keep_first(
        keep_first(stream_result, pump_result, "event pump"),
        monitor_result,
        "monitor",
    )
}

/// Run one teardown drain, bounded by [`TEARDOWN_TIMEOUT`]; expiry becomes the named
/// stage's [`teardown_timeout`] error.
async fn drain(stage: &str, task: impl Future<Output = Result<()>>) -> Result<()> {
    tokio::time::timeout(TEARDOWN_TIMEOUT, task)
        .await
        .unwrap_or_else(|_| Err(teardown_timeout(stage)))
}

/// Flatten a spawned drain's join outcome into its task result.
async fn flat_join(handle: JoinHandle<Result<()>>) -> Result<()> {
    handle
        .await
        .map_err(KrakenError::from)
        .and_then(|result| result)
}

/// The error for a teardown stage that outlived [`TEARDOWN_TIMEOUT`]. The stage is named so
/// the operator knows which finalizer wedged — and that the tape may need a manual check,
/// since a sink that never closed stamped no summary metadata.
fn teardown_timeout(stage: &str) -> KrakenError {
    KrakenError::Task(format!(
        "{stage} did not finish within {}s of shutdown; a wedged finalizer was abandoned and \
         the tape may need a manual check",
        TEARDOWN_TIMEOUT.as_secs()
    ))
}

/// Fan the client's event stream out to every consumer by re-publishing each event on `tx`,
/// signalling `ready` once on the first subscribe ack — the earliest proof the stream is
/// live. The signal is a moment, not an effect: what readiness *does* (announce, log)
/// stays with the receiver's owner, which also decides what its failure means.
///
/// Runs until the stream ends (every connection actor has stopped, so the buffered tail has
/// been forwarded) or every receiver has dropped (no consumer left to feed). The latter lets a
/// stream nobody reads wind itself down: returning drops the stream, whose closure fails the
/// actors' next send. A broadcast send never blocks, so a slow consumer lags on its own receiver
/// rather than back-pressuring the feed.
///
/// An [`Event::Aborted`] degrades the session instead of truncating it: the event is
/// forwarded (so the sink and monitor see the endpoint die) and the pump keeps feeding
/// the surviving endpoints; the recorded abort fails the session once the stream ends,
/// so the session still exits non-zero rather than reading as a clean capture. A
/// single-endpoint session ends at the abort anyway — its only actor just stopped.
async fn pump_events<S>(
    mut stream: S,
    tx: broadcast::Sender<Event>,
    mut ready: Option<oneshot::Sender<()>>,
) -> Result<()>
where
    S: futures_util::Stream<Item = Event> + Unpin,
{
    let mut aborted: Option<KrakenError> = None;
    while let Some(event) = stream.next().await {
        match &event {
            Event::Ack(_) => {
                if let Some(signal) = ready.take()
                    && signal.send(()).is_err()
                {
                    // The caller stopped listening for readiness; the stream itself
                    // is unaffected.
                    tracing::debug!("readiness signal dropped: no listener");
                }
            }
            // A connection gave up for good. The first abort names the session error
            // (matching `keep_first`); a later one falls to the `_` arm — still
            // forwarded, and its actor already warned with the cause. The
            // per-rejection details were already published as `ApiError` events.
            Event::Aborted { endpoint, cause } if aborted.is_none() => {
                aborted = Some(abort_error(*endpoint, cause));
            }
            _ => {}
        }
        if tx.send(event).is_err() {
            break;
        }
    }
    aborted.map_or(Ok(()), Err)
}

/// The session-failing error for a connection that gave up. The abort's server reason
/// (when the rejection carried one) is classified through the standard mapping and the
/// classified error kept whole — an auth rejection stays typed `Auth`, a rate-limit one
/// keeps its guidance fields (suggestion, retryable, docs_url) in the envelope — with
/// the "which endpoint gave up" context folded into its message.
fn abort_error(endpoint: kraken_core::endpoint::Endpoint, cause: &AbortCause) -> KrakenError {
    let message = format!("the {endpoint} connection gave up: {cause}");
    match cause.reason().map(KrakenError::from_kraken_error) {
        Some(KrakenError::RateLimit {
            suggestion,
            retryable,
            docs_url,
            ..
        }) => KrakenError::RateLimit {
            message,
            suggestion,
            retryable,
            docs_url,
        },
        // `Auth`'s Display prepends "Authentication failed: ", so the context is
        // appended rather than leading — never "Authentication failed: the auth
        // connection gave up: ...".
        Some(KrakenError::Auth(_)) => {
            KrakenError::Auth(format!("{cause} (the {endpoint} connection gave up)"))
        }
        // `from_kraken_error` yields RateLimit, Auth, or Api; a new mapping there
        // must pick its abort shape here rather than silently flattening.
        Some(classified) => KrakenError::Api {
            category: classified.category(),
            message,
        },
        None => KrakenError::Api {
            category: crate::errors::ErrorCategory::Api,
            message,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use kraken_core::endpoint::Endpoint;

    use super::*;
    use crate::errors::ErrorCategory;

    /// A subscribe ack event, as the server would send after a successful subscribe.
    fn ack() -> Event {
        let frame = r#"{"method":"subscribe","success":true}"#;
        match frame.parse().expect("a minimal subscribe ack classifies") {
            kraken_core::Inbound::Method(resp) => Event::Ack(resp),
            other => panic!("expected a method reply, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pump_forwards_every_event_then_closes_when_the_stream_ends() {
        // The fan-out contract: each event reaches a subscribed consumer in order, and the
        // broadcast closes once the source stream ends (so consumers see `Closed` and finalize).
        let (tx, mut rx) = broadcast::channel(16);
        let events = vec![Event::Connected(Endpoint::Public), Event::Heartbeat];
        pump_events(futures_util::stream::iter(events), tx, None)
            .await
            .unwrap();

        assert!(matches!(
            rx.try_recv(),
            Ok(Event::Connected(Endpoint::Public))
        ));
        assert!(matches!(rx.try_recv(), Ok(Event::Heartbeat)));
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Closed)
        ));
    }

    #[tokio::test]
    async fn pump_stops_once_every_receiver_is_gone() {
        // A stream nobody reads must wind down, not spin: with no receiver the first send fails
        // and the pump returns at once, even though the source could yield forever.
        let (tx, rx) = broadcast::channel(16);
        drop(rx);
        let forever = futures_util::stream::iter(std::iter::repeat_with(|| Event::Heartbeat));
        tokio::time::timeout(Duration::from_secs(1), pump_events(forever, tx, None))
            .await
            .expect("pump must stop promptly once no receiver remains")
            .unwrap();
    }

    #[tokio::test]
    async fn an_abort_fails_the_pump_classified_by_its_server_reason() {
        // The session must end as an error (the binary maps Err to a non-zero exit),
        // and a permission rejection routes on `auth` — not flattened to `api`.
        let (tx, _rx) = broadcast::channel(16);
        let events = vec![Event::Aborted {
            endpoint: Endpoint::Auth,
            cause: AbortCause::AllSubscriptionsRejected {
                reason: Some("EGeneral:Permission denied".into()),
            },
        }];
        let err = pump_events(futures_util::stream::iter(events), tx, None)
            .await
            .expect_err("an aborted connection must fail the session");
        assert_eq!(err.category(), ErrorCategory::Auth);
        assert!(
            err.to_string().contains("EGeneral:Permission denied"),
            "the server reason must survive into the envelope: {err}"
        );
    }

    #[tokio::test]
    async fn an_abort_degrades_the_stream_instead_of_truncating_it() {
        // One endpoint giving up must not tear down its siblings' capture: the abort
        // is forwarded (the consumers see the endpoint die), later events keep
        // flowing, and the recorded abort fails the session only once the stream ends.
        let (tx, mut rx) = broadcast::channel(16);
        let events = vec![
            Event::Aborted {
                endpoint: Endpoint::Auth,
                cause: AbortCause::AllSubscriptionsRejected {
                    reason: Some("EGeneral:Permission denied".into()),
                },
            },
            Event::Heartbeat,
        ];
        let err = pump_events(futures_util::stream::iter(events), tx, None)
            .await
            .expect_err("the session still ends as an error");
        assert!(matches!(rx.try_recv(), Ok(Event::Aborted { .. })));
        assert!(
            matches!(rx.try_recv(), Ok(Event::Heartbeat)),
            "events after the abort must keep flowing to consumers"
        );
        assert_eq!(err.category(), ErrorCategory::Auth);
    }

    #[test]
    fn a_rate_limited_abort_keeps_the_rate_limit_guidance() {
        // The classified reason crosses whole: the envelope keeps the RateLimit
        // guidance fields instead of a bare category rebuilt around the message.
        let err = abort_error(
            Endpoint::Auth,
            &AbortCause::AllSubscriptionsRejected {
                reason: Some("EOrder:Rate limit exceeded".into()),
            },
        );
        assert!(matches!(err, KrakenError::RateLimit { .. }), "got: {err:?}");
        let envelope = err.to_json_envelope();
        assert_eq!(envelope["error"], "rate_limit");
        assert!(
            envelope.get("suggestion").is_some(),
            "the guidance must survive the abort: {envelope}"
        );
        assert!(err.to_string().contains("connection gave up"), "{err}");
    }

    #[tokio::test]
    async fn an_abort_without_a_server_reason_stays_api() {
        let (tx, _rx) = broadcast::channel(16);
        let events = vec![Event::Aborted {
            endpoint: Endpoint::Auth,
            cause: AbortCause::AllSubscriptionsRejected { reason: None },
        }];
        let err = pump_events(futures_util::stream::iter(events), tx, None)
            .await
            .expect_err("an aborted connection must fail the session");
        assert_eq!(err.category(), ErrorCategory::Api);
    }

    #[tokio::test]
    async fn ready_signals_on_the_first_ack_and_never_without_one() {
        // Readiness is the first subscribe ack, not the connect: a session that connects but
        // never acks (unreachable venue, rejected subscribe) must not signal — its sender
        // drops unfired, so the receiver resolves Err. (Once-only is the oneshot's type.)
        let (ready_tx, ready_rx) = oneshot::channel();
        let (tx, _rx) = broadcast::channel(16);
        pump_events(
            futures_util::stream::iter(vec![Event::Connected(Endpoint::Public)]),
            tx,
            Some(ready_tx),
        )
        .await
        .unwrap();
        assert!(ready_rx.await.is_err(), "no ack, no readiness");

        let (ready_tx, ready_rx) = oneshot::channel();
        let (tx, _rx) = broadcast::channel(16);
        pump_events(
            futures_util::stream::iter(vec![
                Event::Connected(Endpoint::Public),
                ack(),
                ack(),
                Event::Heartbeat,
            ]),
            tx,
            Some(ready_tx),
        )
        .await
        .unwrap();
        assert!(ready_rx.await.is_ok(), "the first ack signals readiness");
    }

    #[tokio::test]
    async fn a_dropped_ready_receiver_does_not_disturb_the_stream() {
        // The caller may stop caring about readiness (its announce path is gone); the
        // pump must keep forwarding regardless.
        let (ready_tx, ready_rx) = oneshot::channel();
        drop(ready_rx);
        let (tx, mut rx) = broadcast::channel(16);
        pump_events(
            futures_util::stream::iter(vec![ack(), Event::Heartbeat]),
            tx,
            Some(ready_tx),
        )
        .await
        .unwrap();
        assert!(matches!(rx.try_recv(), Ok(Event::Ack(_))));
        assert!(matches!(rx.try_recv(), Ok(Event::Heartbeat)));
    }

    #[test]
    fn private_token_error_names_both_halves_with_a_single_prefix() {
        let err = private_token_error(
            &[
                SubscribableChannel::Executions,
                SubscribableChannel::Balances,
            ],
            &[SubscribableChannel::Ticker, SubscribableChannel::Book],
            "EAPI:Invalid key",
        );
        assert_eq!(err.category(), ErrorCategory::Auth);
        let message = err.to_string();
        // The "Authentication failed: " prefix appears exactly once — the doubling bug is gone.
        assert_eq!(
            message.matches("Authentication failed:").count(),
            1,
            "got: {message}"
        );
        // Both the private channels that need auth and the public ones that don't are named,
        // along with the raw upstream reason.
        assert!(message.contains("executions, balances"), "got: {message}");
        assert!(message.contains("ticker, book"), "got: {message}");
        assert!(message.contains("EAPI:Invalid key"), "got: {message}");
    }

    #[test]
    fn private_token_error_omits_the_public_clause_when_all_channels_are_private() {
        let err = private_token_error(&[SubscribableChannel::Executions], &[], "EAPI:Invalid key");
        let message = err.to_string();
        assert!(message.contains("executions"), "got: {message}");
        // No public half to fall back to, so no "drop them to stream..." suggestion.
        assert!(!message.contains("drop them"), "got: {message}");
    }

    #[test]
    fn stream_error_outranks_a_masked_teardown_error() {
        let stream_err = || KrakenError::websocket("stream boom");
        let monitor_err = || KrakenError::Task("monitor panicked".into());
        // A real stream error wins, even when the monitor also failed to join (the masked
        // error is logged, not returned).
        assert!(matches!(
            keep_first(Err(stream_err()), Err(monitor_err()), "monitor"),
            Err(KrakenError::WebSocket { .. })
        ));
        // A monitor join failure surfaces only once the stream itself succeeded.
        assert!(matches!(
            keep_first(Ok(()), Err(monitor_err()), "monitor"),
            Err(KrakenError::Task(_))
        ));
        assert!(keep_first(Ok(()), Ok(()), "monitor").is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn a_wedged_teardown_surfaces_as_an_error_instead_of_hanging() {
        // Shutdown fires immediately, but the sink's driver never finishes — a stand-in for
        // a finalizer wedged in a blocking `CHECKPOINT`. Async-pending rather than
        // `spawn_blocking`, because tokio's paused-time auto-advance is inhibited while
        // blocking-pool tasks are in flight; the timeout under test could otherwise never
        // fire in virtual time. The drains must give up after TEARDOWN_TIMEOUT (virtual
        // time) and name the wedged stage — not hang for SIGKILL.
        let result = tokio::time::timeout(
            TEARDOWN_TIMEOUT * 4,
            fan_out(
                futures_util::stream::pending(),
                |_events| tokio::spawn(std::future::pending()),
                None,
                std::future::ready(()),
                None,
                (),
            ),
        )
        .await
        .expect("fan_out must not outlive the teardown timeouts");
        let err = result.expect_err("a wedged teardown is an error");
        assert!(err.to_string().contains("did not finish"), "got: {err}");
    }

    #[tokio::test]
    async fn close_out_monitor_emits_promptly_without_a_stream() {
        // The failure-path contract: a monitored run that never opened a socket still runs
        // the monitor to its final summary — against an already-closed broadcast it must
        // finish at once (the summary is zero-count), never wait on stream activity.
        let cfg = crate::monitor::MonitorConfig {
            health_interval: Duration::from_secs(60),
            stale_after: Duration::from_secs(60),
            heartbeat_timeout: Duration::from_secs(60),
        };
        tokio::time::timeout(Duration::from_secs(1), close_out_monitor(Some(cfg)))
            .await
            .expect("a closed broadcast must finalize the monitor immediately")
            .unwrap();
        // Without --monitor it is a clean no-op.
        assert!(close_out_monitor(None).await.is_ok());
    }

    #[tokio::test]
    async fn await_monitor_surfaces_a_dead_task_and_no_ops_when_off() {
        // A monitor task that dies (aborted here) must surface as an error, not be swallowed.
        let handle = tokio::spawn(std::future::pending::<()>());
        handle.abort();
        assert!(matches!(
            await_monitor(Some(handle)).await,
            Err(KrakenError::Task(_))
        ));
        // No monitor running is a clean no-op.
        assert!(await_monitor(None).await.is_ok());
    }

    /// A [`CaptureSink`] that records nothing and flags when `finalize` runs, so a test can
    /// assert the fan-out drove it all the way to finalization.
    struct ClosingSink {
        closed: Arc<AtomicBool>,
    }

    impl kraken_recording::Sink for ClosingSink {
        type Item = kraken_core::ChannelMessage;

        fn record(
            &mut self,
            _batch: &[kraken_core::ChannelMessage],
        ) -> kraken_recording::Result<()> {
            Ok(())
        }
    }

    impl CaptureSink for ClosingSink {
        fn finalize(
            self,
            _summary: kraken_recording::RecordingIntegrity,
        ) -> kraken_recording::Result<()> {
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn fan_out_finalizes_when_the_source_stream_ends() {
        // The pump must own the only sender or the sink can wait forever for closure.
        let closed = Arc::new(AtomicBool::new(false));
        let sink = ClosingSink {
            closed: Arc::clone(&closed),
        };
        let events = vec![
            Event::Connected(Endpoint::Public),
            Event::Heartbeat,
            Event::Disconnected(Endpoint::Public),
        ];
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            fan_out(
                futures_util::stream::iter(events),
                |events| driver::run(sink, events),
                None,
                std::future::pending::<()>(),
                None,
                (),
            ),
        )
        .await
        .expect("fan_out must not hang once the source stream ends");

        assert!(result.is_ok(), "clean teardown expected, got: {result:?}");
        assert!(
            closed.load(Ordering::SeqCst),
            "the sink must be closed on teardown"
        );
    }

    /// Regression: a replay's stream is paced in memory, so shutdown must end
    /// the STREAM — dropping the source (a live socket's close) can't on its
    /// own. Without the fix the pump sleeps through the tape's remaining wall
    /// time and wedges the teardown drain, so a Ctrl-C hangs the full 30s
    /// instead of sealing. Frames an hour apart at speed 1 would pace for
    /// ~hours; a 100ms shutdown must end the stream and finalize the sink well
    /// inside a wall-clock second. Real time (not `start_paused`): the point is
    /// precisely that the paced sleep is abandoned promptly, and a wedged pump
    /// trips the outer `timeout` — a clean, non-flaky failure.
    #[tokio::test]
    async fn replay_shutdown_ends_the_paced_stream_without_pacing_the_whole_tape() {
        use kraken_core::ChannelData;
        use kraken_core::subscribe::message::MessageType;
        use kraken_recording::MarketEvent;

        let anchor = chrono::DateTime::<chrono::Utc>::from_timestamp(1_760_000_000, 0).unwrap();
        let frames: Vec<MarketEvent> = (0..3)
            .map(|i| MarketEvent {
                at: anchor + chrono::Duration::hours(i),
                seq: i,
                frame: kraken_core::ChannelMessage {
                    body: ChannelData::Trade(vec![]),
                    message_type: MessageType::Update,
                    sequence: None,
                },
            })
            .collect();
        let map = kraken_session::manifest::SessionSource {
            tape: "tape:t".to_string(),
            speed: 1.0,
            anchor,
            started_at: anchor,
            content_hash: None,
        };
        let closed = Arc::new(AtomicBool::new(false));
        let sink = ClosingSink {
            closed: Arc::clone(&closed),
        };

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            run_replay(
                frames,
                map,
                None,
                sink,
                None,
                (),
                tokio::time::sleep(Duration::from_millis(100)),
            ),
        )
        .await
        .expect("shutdown ends the paced replay promptly, not after the tape's hours-long span");

        assert!(
            result.is_ok(),
            "shutdown finalizes gracefully, got: {result:?}"
        );
        assert!(
            closed.load(Ordering::SeqCst),
            "the sink is finalized on a shutdown-ended replay, not abandoned"
        );
    }
}
