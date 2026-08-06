//! Both connection flavours over the one [`Transport`] seam, as one generic
//! [`Connection`] machine — the clients above never touch a socket directly.
//!
//! The shared mechanism is written once: dialing paced by [`Backoff`], token minting
//! through [`TokenCache`], data exchange over one events channel, drop-triggered
//! shutdown via [`CancellationToken`]. What differs is the session intent, carried by
//! the [`Mode`] value: [`Stream`] replays a fixed subscription set and reconnects
//! forever; [`OneShot`] sends one request and ends at the server's close — never
//! re-sending, since a redelivered order is a double-submit. A mode says what to put
//! on the wire at session start; what the replies *mean* stays with each client (the
//! stream types its frames only because rejection-driven routing is lifecycle policy).

use std::sync::Arc;
use std::time::Duration;

use kraken_core::endpoint::Endpoint;
use kraken_core::error::{Error, Result};
use kraken_core::{
    ChannelMessage, Inbound, Method, MethodReply, MethodResponse, PingParams, Request, Wire,
    WsSubscription,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::backoff::{Backoff, ReconnectPolicy};
use super::token::TokenCache;
use super::transport::{ConnEvent, ConnReader, ConnWriter, Transport};

/// A connection flavour: the session intent the shared machinery drives, and the
/// vocabulary it publishes. A mode carries what to say once the socket is up — never
/// how to interpret what comes back.
pub(crate) trait Mode: Send + Sync + 'static {
    /// What this flavour publishes on its events channel.
    type Event: Send;
}

/// The streaming intent: a fixed subscription set, replayed on every reconnect.
pub(crate) struct Stream {
    /// Reconnect pacing — caller intent here, unlike the one-shot's fixed dial schedule.
    pub(super) policy: ReconnectPolicy,
    /// Fixed at construction; re-sent (with a forced snapshot) on every reconnect.
    pub(super) subs: Vec<WsSubscription>,
    /// The caller's `--req-id` echoed on the initial subscribe ack; multiplexed streams
    /// pass `None`.
    pub(super) req_id: Option<std::num::NonZeroU64>,
}

impl Mode for Stream {
    type Event = Event;
}

/// The request intent: one method call, answered by one correlated reply batch.
pub(crate) struct OneShot<P> {
    pub(super) params: P,
    pub(super) req_id: u64,
}

impl<P: Request + Send + Sync + 'static> Mode for OneShot<P> {
    /// Raw frames, verbatim: parsing, correlation, and gathering are the request
    /// client's job.
    type Event = ConnEvent;
}

pub(crate) type StreamConnection<T> = Connection<T, Stream>;
pub(crate) type OneShotConnection<T, P> = Connection<T, OneShot<P>>;

/// One session machine over a [`Transport`], generic in its [`Mode`]. The shared
/// fields are the shared mechanism; each mode's `run` drives them to completion.
///
/// Built as a struct literal by its client (`StreamClient` / `RequestClient`), so the
/// fields are `pub(super)` rather than a six-argument constructor.
pub(crate) struct Connection<T: Transport, M: Mode> {
    pub(super) endpoint: Endpoint,
    pub(super) transport: T,
    /// `Some` exactly when this session authenticates; both flavours mint through it.
    pub(super) token_cache: Option<Arc<TokenCache>>,
    /// Fan-in sender for this mode's events; the send fails only once the client's
    /// receiving end is dropped.
    pub(super) events_tx: mpsc::UnboundedSender<M::Event>,
    /// Cancelled when the owning handle is dropped; every phase races it.
    pub(super) shutdown: CancellationToken,
    /// The session intent: what this connection says once the socket is up.
    pub(super) mode: M,
}

impl<T: Transport, M: Mode> Connection<T, M> {
    /// Close the socket on teardown. A failed close is traced and the writer dropped —
    /// an abrupt TCP close — never propagated: the session is ending either way.
    async fn close_quietly(&self, writer: &mut T::Writer) {
        if let Err(error) = writer.close().await {
            tracing::debug!(endpoint = %self.endpoint, %error, "close failed during teardown");
        }
    }

    /// Publish an event. A failed send after shutdown is the drained fan-out (traced);
    /// mid-session it means every consumer vanished — a socket feeding nobody has no
    /// reason to stay open, so it cancels this connection like a handle drop.
    fn emit(&self, event: M::Event) {
        if self.events_tx.send(event).is_ok() {
            return;
        }
        if self.shutdown.is_cancelled() {
            tracing::trace!(endpoint = %self.endpoint, "event dropped after shutdown; channel closed");
        } else {
            tracing::warn!(
                endpoint = %self.endpoint,
                "event channel dropped mid-session; shutting the connection down"
            );
            self.shutdown.cancel();
        }
    }

    /// This session's token, or `None` for an unauthenticated session — the cache is
    /// `Some` exactly when the session authenticates.
    async fn session_token(&self) -> Result<Option<String>> {
        match &self.token_cache {
            Some(cache) => cache.get().await.map(Some),
            None => Ok(None),
        }
    }

    /// Like [`Self::session_token`], but mints fresh instead of trusting the cache:
    /// tokens are connection-scoped, and re-reading the shared cache could pick up a
    /// sibling's concurrent mint.
    async fn fresh_session_token(&self) -> Result<Option<String>> {
        match &self.token_cache {
            Some(cache) => cache.refresh().await.map(Some),
            None => Ok(None),
        }
    }

    /// Wait out the next backoff delay; `true` if shutdown fired while waiting.
    async fn backoff_or_stop(&self, backoff: &mut Backoff) -> bool {
        self.shutdown
            .run_until_cancelled(tokio::time::sleep(backoff.next_delay()))
            .await
            .is_none()
    }
}

// ── Streaming connections ───────────────────────────────────────────────────

const PING_INTERVAL: Duration = Duration::from_secs(30);

/// Maximum read-half silence before the socket is presumed dead. A healthy socket sees
/// frames continuously (heartbeats, pongs, data), so silence spanning several ping
/// rounds means a wedged read a ping write hasn't yet failed on.
pub(super) const READ_IDLE_TIMEOUT: Duration = PING_INTERVAL.saturating_mul(3);

/// `sub` minus every evicted symbol: `None` once nothing subscribable remains.
/// Eviction is symbol-scoped, not (channel, symbol)-scoped: the rejection ack names
/// only the offending pair, so an evicted symbol drops out of *every* channel on this
/// connection — a channel-specific rejection (say, an unsupported book depth) takes
/// the pair's healthy channels with it. Tightening this needs per-subscription
/// `req_id` correlation on the acks.
fn effective_subscription(sub: &WsSubscription, evicted: &[String]) -> Option<WsSubscription> {
    let mut params = sub.clone();
    params
        .retain_symbols(|symbol| !evicted.iter().any(|e| e == symbol))
        .then_some(params)
}

/// Open rejection incidents, scoped to what the rejection ack named: per symbol for
/// named rejections, per endpoint for anonymous ones. Each incident earns exactly one
/// fresh-token reconnect (a stale token is the one cause a retry cures); a repeat is
/// the second strike. An incident closes when its symbol's re-subscribe is acked
/// ([`cure`](Self::cure)) or when a session streams past the stability threshold;
/// recording happens in `run()` *after* the stability clear, so the strike that ended
/// a session always survives into the retry it pays for.
#[derive(Default)]
struct RejectionIncidents {
    /// Symbols whose rejection already spent its fresh-token retry.
    symbols: Vec<String>,
    /// An anonymous (symbol-less) rejection already spent the endpoint's retry.
    anonymous: bool,
}

impl RejectionIncidents {
    /// Whether this rejection repeats an incident that already spent its retry.
    fn is_repeat(&self, symbol: Option<&str>) -> bool {
        match symbol {
            Some(symbol) => self.symbols.iter().any(|s| s == symbol),
            None => self.anonymous,
        }
    }

    /// Open the incident: the fresh-token retry is now spent.
    fn record(&mut self, symbol: Option<&str>) {
        match symbol {
            Some(symbol) if !self.is_repeat(Some(symbol)) => self.symbols.push(symbol.to_owned()),
            Some(_) => {}
            None => self.anonymous = true,
        }
    }

    /// A successful re-subscribe ack for `symbol`: the fresh token cured it, so a
    /// later rejection is a new incident owed its own retry. Anonymous incidents have
    /// no ack to key on and close only via the stability clear.
    fn cure(&mut self, symbol: &str) {
        self.symbols.retain(|s| s != symbol);
    }

    fn clear(&mut self) {
        self.symbols.clear();
        self.anonymous = false;
    }
}

/// An event published by a streaming connection onto the client's event channel.
#[derive(Debug, Clone)]
pub enum Event {
    /// The socket connected (initial connect or a reconnect).
    Connected(Endpoint),
    /// The socket dropped; a reconnect attempt follows.
    Disconnected(Endpoint),
    /// The session never became live: the dial failed, or its setup (token mint,
    /// subscribe write) failed before anything streamed. Distinct from
    /// [`Disconnected`](Self::Disconnected) so the monitor can tell "never started"
    /// from a healthy-but-quiet stream.
    ConnectFailed(Endpoint),
    /// A decoded channel-data frame.
    Message(ChannelMessage),
    /// A successful subscribe acknowledgement — the only method success a stream socket
    /// surfaces (pong is dropped, anything else is drift), so readiness can key off it.
    Ack(MethodResponse),
    /// A rejected method acknowledgement with no awaiting caller.
    ApiError(MethodResponse),
    /// A server keepalive: the monitor's connection-level liveness signal, independent
    /// of market activity.
    Heartbeat,
    /// An inbound frame failed the pinned schema and was dropped before reaching any
    /// consumer. The capture-integrity signal: sinks and monitors count it, because
    /// the loss is invisible to them otherwise — the frame never becomes a `Message`.
    ParseFailure(Endpoint),
    /// The connection gave up for good; `cause` carries why, so consumers render the
    /// actor's decision instead of assuming one. The session must end as an error,
    /// not a clean capture.
    Aborted {
        endpoint: Endpoint,
        cause: AbortCause,
    },
}

/// Why a connection gave up. A new terminal give-up must name itself here — the
/// cause travels on [`Event::Aborted`] rather than living in a consumer's string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortCause {
    /// Every subscription was rejected on a fresh token. Carries the server's last
    /// rejection string (when the ack had one) so the binary edge classifies the
    /// abort — auth, rate limit — instead of flattening it to a generic category.
    AllSubscriptionsRejected { reason: Option<String> },
}

impl AbortCause {
    /// The server's error string behind the give-up, when one was sent.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::AllSubscriptionsRejected { reason } => reason.as_deref(),
        }
    }
}

impl std::fmt::Display for AbortCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AllSubscriptionsRejected { reason } => {
                f.write_str("every subscription was rejected on a fresh token")?;
                if let Some(reason) = reason {
                    write!(f, ": {reason}")?;
                }
                Ok(())
            }
        }
    }
}

/// Which attempt of the reconnect loop a session belongs to — decides the token (reuse
/// vs. re-mint) and the subscribe shape. An enum because only three of the four flag
/// combinations are legal: a replay always re-mints.
#[derive(Clone, Copy)]
enum Attempt {
    /// The very first attempt: subscribe with the token minted up front.
    First,
    /// A prior attempt failed before a session established: re-mint (the up-front token
    /// may have gone stale), but subscribe as a first send.
    Redial,
    /// A served session dropped: re-mint and replay with a forced snapshot, no `req_id`.
    Replay,
}

impl Attempt {
    /// Whether the session must re-mint rather than trust the up-front token.
    fn must_remint(self) -> bool {
        !matches!(self, Attempt::First)
    }

    /// Whether the subscribe replays with a forced snapshot and no echoed `req_id`.
    fn is_replay(self) -> bool {
        matches!(self, Attempt::Replay)
    }
}

/// What the event loop should do after one inbound frame.
enum FrameOutcome {
    Continue,
    /// A `subscribe` was rejected; carries the pair the ack named (for eviction) and
    /// the server's error string (for the abort cause).
    SubscribeRejected {
        symbol: Option<String>,
        reason: Option<String>,
    },
}

/// Why a serve loop ended.
enum ServeOutcome {
    /// Shutdown was signalled. Stop for good.
    Stop,
    /// An established session's socket dropped. Reconnect.
    Disconnected,
    /// A subscribe was rejected before the incident's fresh-token retry was spent:
    /// reconnect so the replay re-mints.
    SubscribeRejected {
        /// The pair the rejection named, if any — recorded in `run()` post-clear.
        symbol: Option<String>,
    },
    /// Every subscription rejected on a fresh token: stop instead of reconnect-flapping.
    /// Carries the last rejection's server error string for the abort cause.
    AllRejected { reason: Option<String> },
    /// The dial, token mint, or subscribe write failed before the event loop started.
    /// Retried on backoff, so the consumer never sees a `Connected` flap.
    SetupFailed,
}

impl<T: Transport> Connection<T, Stream> {
    /// Drive the connection until shutdown is signalled.
    pub(crate) async fn run(self) {
        let mut backoff = Backoff::new(self.mode.policy);
        let mut attempt = Attempt::First;
        let mut incidents = RejectionIncidents::default();
        let mut evicted: Vec<String> = Vec::new();
        loop {
            let outcome = match self
                .shutdown
                .run_until_cancelled(self.transport.connect())
                .await
            {
                None => break,
                // A failed dial is a failed setup: same recovery, one policy point.
                Some(Err(error)) => {
                    tracing::warn!(endpoint = %self.endpoint, %error, "connect failed");
                    ServeOutcome::SetupFailed
                }
                Some(Ok((writer, reader))) => {
                    // `tokio::time::Instant`, so the stability threshold is
                    // exercisable under paused virtual time (as token.rs does).
                    let session_start = tokio::time::Instant::now();
                    let outcome = self
                        .serve(writer, reader, attempt, &mut incidents, &mut evicted)
                        .await;
                    // A session that *streamed* past the stability threshold closes
                    // every open incident (a slow SetupFailed proves nothing — so it
                    // earns neither the clear nor the backoff reset below). Cleared
                    // before the outcome records its own strike, so the rejection that
                    // ended this session always survives into the retry it pays for.
                    let session = session_start.elapsed();
                    if !matches!(outcome, ServeOutcome::SetupFailed) {
                        if self.mode.policy.is_stable(session) {
                            incidents.clear();
                        }
                        // Only a *stable* session restarts the schedule, so an
                        // instantly-dropping socket escalates instead of hot-looping.
                        backoff.note_session(session);
                    }
                    outcome
                }
            };
            match outcome {
                ServeOutcome::Stop => break,
                ServeOutcome::AllRejected { reason } => {
                    tracing::error!(
                        endpoint = %self.endpoint,
                        reason = reason.as_deref().unwrap_or("none given"),
                        "every subscription was rejected on a fresh token; stopping the connection"
                    );
                    self.emit(Event::Aborted {
                        endpoint: self.endpoint,
                        cause: AbortCause::AllSubscriptionsRejected { reason },
                    });
                    break;
                }
                ServeOutcome::SubscribeRejected { ref symbol } => {
                    incidents.record(symbol.as_deref());
                    attempt = Attempt::Replay;
                    self.emit(Event::Disconnected(self.endpoint));
                    if self.backoff_or_stop(&mut backoff).await {
                        break;
                    }
                }
                ServeOutcome::Disconnected => {
                    // A session that reached the wire escalates to Replay.
                    attempt = Attempt::Replay;
                    self.emit(Event::Disconnected(self.endpoint));
                    if self.backoff_or_stop(&mut backoff).await {
                        break;
                    }
                }
                ServeOutcome::SetupFailed => {
                    // Retries as a first send (Redial re-mints) without downgrading
                    // a Replay.
                    if matches!(attempt, Attempt::First) {
                        attempt = Attempt::Redial;
                    }
                    self.emit(Event::ConnectFailed(self.endpoint));
                    if self.backoff_or_stop(&mut backoff).await {
                        break;
                    }
                }
            }
        }
    }

    async fn serve(
        &self,
        mut writer: T::Writer,
        mut reader: T::Reader,
        attempt: Attempt,
        incidents: &mut RejectionIncidents,
        evicted: &mut Vec<String>,
    ) -> ServeOutcome {
        if let Some(outcome) = self.establish_session(&mut writer, attempt, evicted).await {
            return outcome;
        }
        // Announced only once the subscriptions are on the wire, so a consumer never
        // sees `Connected` for a session that failed its setup.
        self.emit(Event::Connected(self.endpoint));
        self.run_event_loop(&mut writer, &mut reader, incidents, evicted)
            .await
    }

    /// Resolve the session's token and send the subscription set, raced against
    /// shutdown. `Some` short-circuits `serve`; `None` means the session is live.
    async fn establish_session(
        &self,
        writer: &mut T::Writer,
        attempt: Attempt,
        evicted: &[String],
    ) -> Option<ServeOutcome> {
        let setup = async {
            // A stale cached token can't serve this new socket, so a mint failure
            // tears down and backs off; the next attempt re-mints.
            let token = if attempt.must_remint() {
                self.fresh_session_token().await?
            } else {
                self.session_token().await?
            };
            self.send_subscriptions(&mut *writer, attempt, token.as_deref(), evicted)
                .await
        };
        // Bound before matching: the raced future borrows the writer, and a match
        // scrutinee's temporary would hold that borrow across the close arm.
        let outcome = self.shutdown.run_until_cancelled(setup).await;
        match outcome {
            None => {
                self.close_quietly(writer).await;
                Some(ServeOutcome::Stop)
            }
            Some(Ok(())) => None,
            Some(Err(error)) => {
                tracing::warn!(
                    endpoint = %self.endpoint,
                    %error,
                    "session setup failed; backing off to retry"
                );
                Some(ServeOutcome::SetupFailed)
            }
        }
    }

    /// Drive the live socket until it drops or shutdown fires.
    async fn run_event_loop(
        &self,
        writer: &mut T::Writer,
        reader: &mut T::Reader,
        incidents: &mut RejectionIncidents,
        evicted: &mut Vec<String>,
    ) -> ServeOutcome {
        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ping.tick().await; // consume the immediate first tick

        // Built from the domain `ping` request so the keepalive shares every other
        // method's wire framing: `{"method":"ping"}`, pinned by test.
        let ping_params = PingParams {};
        let ping_wire = Wire::new(PingParams::METHOD, None, &ping_params, None);

        // A deadline only a *read* resets: catches a half-open socket whose buffered
        // ping writes haven't failed yet.
        let idle = tokio::time::sleep(READ_IDLE_TIMEOUT);
        tokio::pin!(idle);

        // Fresh-token rejections whose ack names no pair, counted per session so a
        // socket where *everything* was rejected aborts instead of idling forever.
        let mut anonymous_rejections: usize = 0;

        loop {
            tokio::select! {
                // Shutdown first, so a stop is honoured even on a continuously-busy socket.
                biased;

                () = self.shutdown.cancelled() => {
                    self.close_quietly(writer).await;
                    return ServeOutcome::Stop;
                }

                event = reader.recv() => match event {
                    // The actor parses its own frames: routing (rejection-driven
                    // reconnect) is lifecycle policy, so the data must be typed here.
                    ConnEvent::Frame(text) => match text.parse::<Inbound>() {
                        Ok(frame) => {
                            // A parsed frame proves the read half is alive. A drifted one
                            // is never fatal to a stream, but it is not liveness either —
                            // no idle reset, so an all-junk feed still trips the reconnect.
                            idle.as_mut()
                                .reset(tokio::time::Instant::now() + READ_IDLE_TIMEOUT);
                            if let FrameOutcome::SubscribeRejected { symbol, reason } =
                                self.handle_frame(frame, &text, incidents)
                                && let Some(outcome) = self.on_subscribe_rejected(
                                    symbol,
                                    reason,
                                    incidents,
                                    evicted,
                                    &mut anonymous_rejections,
                                )
                            {
                                return outcome;
                            }
                        }
                        Err(error) => {
                            tracing::warn!(endpoint = %self.endpoint, %error, "ignoring unparseable inbound frame");
                            self.emit(Event::ParseFailure(self.endpoint));
                        }
                    },
                    ConnEvent::Closed => return ServeOutcome::Disconnected,
                    ConnEvent::Error(reason) => {
                        tracing::warn!(endpoint = %self.endpoint, %reason, "stream error");
                        return ServeOutcome::Disconnected;
                    }
                },

                _ = ping.tick() => {
                    if let Err(error) = writer.send_frame(&ping_wire).await {
                        tracing::warn!(endpoint = %self.endpoint, %error, "keepalive send failed; reconnecting");
                        return ServeOutcome::Disconnected;
                    }
                }

                () = &mut idle => {
                    tracing::warn!(
                        endpoint = %self.endpoint,
                        idle_ms = READ_IDLE_TIMEOUT.as_millis(),
                        "no inbound frame; presuming the socket dead and reconnecting"
                    );
                    return ServeOutcome::Disconnected;
                }
            }
        }
    }

    /// Send the subscription set, skipping anything `evicted` names. A first send keeps
    /// each subscription's own snapshot preference and echoes the mode's `req_id`; a
    /// replay forces a snapshot (local state rebuilds cleanly) and echoes no id.
    async fn send_subscriptions(
        &self,
        writer: &mut T::Writer,
        attempt: Attempt,
        token: Option<&str>,
        evicted: &[String],
    ) -> Result<()> {
        for sub in &self.mode.subs {
            let Some(mut params) = effective_subscription(sub, evicted) else {
                continue;
            };
            let req_id = if attempt.is_replay() {
                params.force_snapshot();
                None
            } else {
                self.mode.req_id.map(std::num::NonZeroU64::get)
            };
            writer
                .send_frame(&Wire::new(WsSubscription::METHOD, token, &params, req_id))
                .await?;
        }
        Ok(())
    }

    fn handle_frame(
        &self,
        frame: Inbound,
        raw: &str,
        incidents: &mut RejectionIncidents,
    ) -> FrameOutcome {
        match frame {
            Inbound::Channel(message) => {
                // Tolerated vocabulary growth is still logged, never silent: the
                // fields decoded as `Unknown` and the frame flows on. The warn
                // carries no payload; the original token survives nowhere else
                // (re-serialization emits "unknown"), so the raw frame is the one
                // diagnostic — debug-gated per the raw-payload logging rule, and
                // never for an endpoint whose frames carry the account's balances
                // and order flow.
                if message.body.has_unknown_vocabulary() {
                    tracing::warn!(
                        endpoint = %self.endpoint,
                        channel = %message.channel(),
                        "frame carries vocabulary outside the pinned catalogue; decoded as \"unknown\""
                    );
                    if !self.endpoint.carries_account_data() {
                        tracing::debug!(endpoint = %self.endpoint, frame = %raw, "unknown-vocabulary frame");
                    }
                }
                self.emit(Event::Message(message));
            }
            Inbound::Method(resp) => return self.route_method(resp, incidents),
            Inbound::Heartbeat => {
                self.emit(Event::Heartbeat);
            }
        }
        FrameOutcome::Continue
    }

    /// Route a method reply by (method, success) — the streaming twin of the one-shot
    /// client's `correlated`. Every [`Method`] is enumerated — no catch-all — so a new
    /// variant fails to compile until it picks a row here.
    fn route_method(
        &self,
        resp: MethodResponse,
        incidents: &mut RejectionIncidents,
    ) -> FrameOutcome {
        match (resp.method(), resp.is_success()) {
            // The reply to our own keepalive: no awaiting caller, not consumer data.
            (Method::Pong, _) => FrameOutcome::Continue,
            (Method::Subscribe, true) => {
                // The fresh token demonstrably works for this pair: its incident is
                // cured, so a later rejection is a new problem owed its own retry.
                if let MethodReply::Subscribe(Some(ack)) = &resp.reply
                    && let Some(symbol) = ack.symbol.as_deref()
                {
                    incidents.cure(symbol);
                }
                self.emit(Event::Ack(resp));
                FrameOutcome::Continue
            }
            // A rejected subscribe drives the reconnect/evict policy.
            (Method::Subscribe, false) => {
                let symbol = resp.symbol.clone();
                let reason = resp.error.clone();
                self.emit(Event::ApiError(resp));
                FrameOutcome::SubscribeRejected { symbol, reason }
            }
            // Not a catch-all: every remaining method is consciously routed here.
            (
                Method::AddOrder
                | Method::AmendOrder
                | Method::BatchAdd
                | Method::BatchCancel
                | Method::CancelAll
                | Method::CancelAllOrdersAfter
                | Method::CancelOrder
                | Method::Ping
                | Method::Unsubscribe,
                success,
            ) => {
                if success {
                    // The actor sends only subscribe + ping, so no other success can
                    // answer on this socket: protocol drift, surfaced — never ack'd
                    // (see [`Event::Ack`]).
                    tracing::warn!(
                        endpoint = %self.endpoint,
                        method = %resp.method(),
                        "unexpected method success on a stream socket"
                    );
                } else {
                    // A rejection is consumer-visible error data (e.g. a refused
                    // keepalive).
                    self.emit(Event::ApiError(resp));
                }
                FrameOutcome::Continue
            }
        }
    }

    /// Decide what a rejected subscribe means. First rejection of an incident:
    /// reconnect, since a stale token is the one cause a retry cures — one retry per
    /// incident, scoped to the pair the ack named (an unrelated later rejection is its
    /// own incident with its own retry). The same pair rejected again on its fresh
    /// token: evict it (the remainder keeps streaming), stop once nothing is left. No
    /// pair named: count it — one dead channel among live ones is left dead rather
    /// than flapping the endpoint, but once every live subscription has been rejected
    /// the socket streams nothing and must abort (its own keepalive pongs would
    /// otherwise defeat the idle watchdog forever).
    fn on_subscribe_rejected(
        &self,
        symbol: Option<String>,
        reason: Option<String>,
        incidents: &mut RejectionIncidents,
        evicted: &mut Vec<String>,
        anonymous_rejections: &mut usize,
    ) -> Option<ServeOutcome> {
        if !incidents.is_repeat(symbol.as_deref()) {
            tracing::warn!(
                endpoint = %self.endpoint,
                symbol = symbol.as_deref().unwrap_or("<unnamed>"),
                "a subscribe was rejected; reconnecting once to re-subscribe with a fresh token"
            );
            return Some(ServeOutcome::SubscribeRejected { symbol });
        }
        let Some(symbol) = symbol else {
            *anonymous_rejections += 1;
            if *anonymous_rejections >= self.live_subscriptions(evicted) {
                return Some(ServeOutcome::AllRejected { reason });
            }
            tracing::warn!(
                endpoint = %self.endpoint,
                "subscribe rejected again on a fresh token and the ack named no pair; \
                 leaving that channel dead on this socket"
            );
            return None;
        };
        tracing::warn!(
            endpoint = %self.endpoint,
            %symbol,
            "subscribe rejected again on a fresh token; dropping the pair from the replay set"
        );
        evicted.push(symbol);
        (self.live_subscriptions(evicted) == 0).then_some(ServeOutcome::AllRejected { reason })
    }

    /// How many subscriptions still have anything to subscribe once `evicted` is
    /// applied.
    fn live_subscriptions(&self, evicted: &[String]) -> usize {
        self.mode
            .subs
            .iter()
            .filter(|sub| effective_subscription(sub, evicted).is_some())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_frame_matches_the_v2_keepalive_bytes() {
        let params = PingParams {};
        let wire = Wire::new(PingParams::METHOD, None, &params, None);
        assert_eq!(
            serde_json::to_string(&wire).unwrap(),
            r#"{"method":"ping"}"#
        );
    }
}

// ── One-shot connections ────────────────────────────────────────────────────

/// The one-shot dial's pacing: the same [`Backoff`] the streaming actor uses, with
/// jitter disabled — a single process probing one host has no herd to de-correlate.
/// The schedule is 250ms, 500ms, 1s, 2s (~3.75s of sleeps inside [`DIAL_TIMEOUT`]).
/// Machinery, not caller intent, so it lives here rather than in the [`OneShot`] mode.
const DIAL_RETRY: ReconnectPolicy = ReconnectPolicy {
    initial: Duration::from_millis(250),
    max: Duration::from_secs(2),
    jitter_max: Duration::ZERO,
    // Never consulted: the dial loop notes no sessions.
    stable_after: Duration::ZERO,
};

/// Dial attempts before giving up. Bounds the retry *count* independently of the
/// dial budget — a fast-failing host would otherwise burst through the window — and
/// is what makes the dial surface the concrete last error instead of retrying
/// forever like the streaming flavour deliberately does.
const DIAL_MAX_ATTEMPTS: u32 = 5;

/// Overall dial budget across every retry. A miss is the retry-safe
/// [`Error::DialTimeout`]: nothing has been sent yet.
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Reply budget once the request is on the wire, for a server that never closes the
/// socket. An elapse surfaces as [`Error::Unacknowledged`] — never a retry-safe
/// timeout, since the frame may have reached the exchange.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// Brand a post-send stop reason. Only the connection knows which side of the send a
/// failure happened on: dial and mint failures keep their own class (retry-safe, nothing
/// reached the wire), while a failure at or after the send must not read as blindly
/// retryable — the request may have reached the exchange.
fn verify_first(stop: Error) -> Error {
    if stop.is_verify_first() {
        stop
    } else {
        Error::ReplyLost(Box::new(stop))
    }
}

impl<T: Transport, P: Request + Send + Sync + 'static> Connection<T, OneShot<P>> {
    /// Drive the session to completion: dial, send the one request, pump raw replies
    /// onto the events channel. Every terminal path emits `Closed` or `Error` last, so
    /// the gathering client always learns how the exchange ended.
    pub(crate) async fn run(self) {
        let Some(dialed) = self.dial().await else {
            return; // shutdown while dialing: the caller is gone
        };
        let (mut writer, mut reader) = match dialed {
            Ok(socket) => socket,
            Err(error) => {
                self.emit(ConnEvent::Error(error));
                return;
            }
        };
        // Bound before matching: the raced future borrows the writer, and a match
        // scrutinee's temporary would hold that borrow across the close arm.
        let established = self
            .shutdown
            .run_until_cancelled(self.establish(&mut writer))
            .await;
        let Some(established) = established else {
            self.close_quietly(&mut writer).await;
            return;
        };
        match established {
            Ok(()) => {
                self.pump(&mut reader).await;
                // Close only now, after the batch ended (client done, server close,
                // or the reply budget): a Close sent before the reply licenses the
                // server to drop it — RFC 6455 forbids data frames after its echo.
                self.close_quietly(&mut writer).await;
            }
            Err(error) => self.emit(ConnEvent::Error(error)),
        }
    }

    /// Dial with the capped retry, the whole schedule inside one [`DIAL_TIMEOUT`]
    /// budget; `None` on shutdown.
    async fn dial(&self) -> Option<Result<(T::Writer, T::Reader)>> {
        let attempts = async {
            let mut backoff = Backoff::new(DIAL_RETRY);
            let mut attempt = 1;
            loop {
                match self
                    .shutdown
                    .run_until_cancelled(self.transport.connect())
                    .await?
                {
                    Ok(socket) => return Some(Ok(socket)),
                    Err(error) => {
                        if attempt >= DIAL_MAX_ATTEMPTS {
                            return Some(Err(error));
                        }
                        tracing::warn!(%error, attempt, max = DIAL_MAX_ATTEMPTS, "dial failed; retrying");
                        if self.backoff_or_stop(&mut backoff).await {
                            return None;
                        }
                        attempt += 1;
                    }
                }
            }
        };
        match tokio::time::timeout(DIAL_TIMEOUT, attempts).await {
            Ok(outcome) => outcome,
            Err(_elapsed) => Some(Err(Error::DialTimeout)),
        }
    }

    /// Mint (when the session authenticates), frame, send. The socket stays open —
    /// the client's gather ends the batch at its expected reply count, and the close
    /// handshake runs afterwards. A mint failure propagates with its own class — it
    /// is pre-send, so it must never read as verify-first; the send is where the
    /// verify-first branding starts.
    async fn establish(&self, writer: &mut T::Writer) -> Result<()> {
        // Never a re-mint: a one-shot has no retry incident a fresh token could cure.
        let token = self.session_token().await?;
        let wire = Wire::new(
            P::METHOD,
            token.as_deref(),
            &self.mode.params,
            Some(self.mode.req_id),
        );
        writer.send_frame(&wire).await.map_err(verify_first)
    }

    /// Forward raw frames until the server closes, the reply budget elapses, or
    /// shutdown fires. The budget is total, not per-frame: one exchange has no
    /// business outliving it, however slowly the server drips.
    async fn pump(&self, reader: &mut T::Reader) {
        let deadline = tokio::time::sleep(REPLY_TIMEOUT);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                biased;
                () = self.shutdown.cancelled() => return,
                () = &mut deadline => {
                    // The frame is on the wire, so silence must not read as retry-safe.
                    self.emit(ConnEvent::Error(Error::Unacknowledged));
                    return;
                }
                event = reader.recv() => {
                    // Everything the pump sees happens after the send: a socket loss
                    // here loses the reply, not the request.
                    let event = match event {
                        ConnEvent::Error(cause) => ConnEvent::Error(verify_first(cause)),
                        other => other,
                    };
                    let terminal = matches!(event, ConnEvent::Closed | ConnEvent::Error(_));
                    self.emit(event);
                    if terminal {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod one_shot_tests {
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::mock_transport::{MockTransport, wait_until};
    use crate::token::{BoxFuture, Minted, TokenSource};

    /// A one-shot `ping` session over `transport`, plus its gather-side receiver.
    fn one_shot(
        transport: MockTransport,
    ) -> (
        OneShotConnection<MockTransport, PingParams>,
        mpsc::UnboundedReceiver<ConnEvent>,
    ) {
        one_shot_with_cache(transport, None)
    }

    fn one_shot_with_cache(
        transport: MockTransport,
        token_cache: Option<Arc<TokenCache>>,
    ) -> (
        OneShotConnection<MockTransport, PingParams>,
        mpsc::UnboundedReceiver<ConnEvent>,
    ) {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let connection = OneShotConnection {
            endpoint: Endpoint::Public,
            transport,
            token_cache,
            events_tx,
            shutdown: CancellationToken::new(),
            mode: OneShot {
                params: PingParams {},
                req_id: 1,
            },
        };
        (connection, events_rx)
    }

    /// A token source whose mint always fails, for the pre-send phase test.
    struct RejectingSource;

    impl TokenSource for RejectingSource {
        fn fetch(&self) -> BoxFuture<Result<Minted>> {
            Box::pin(async { Err(Error::TokenRejected("EAPI:Invalid key".into())) })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_transient_dial_failure_is_retried_within_the_budget() {
        let transport = MockTransport::new();
        transport.fail_first.store(2, Ordering::SeqCst);
        let (connection, _events) = one_shot(transport.clone());
        let dialed = connection.dial().await.expect("no shutdown was signalled");
        let _socket = dialed.expect("connects after the transient failures");
        assert_eq!(transport.connect_count(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn a_dead_endpoint_gives_up_after_the_attempt_limit() {
        let mut transport = MockTransport::new();
        transport.fail = true;
        let (connection, _events) = one_shot(transport.clone());
        let Err(err) = connection.dial().await.expect("no shutdown was signalled") else {
            panic!("a permanently dead endpoint must not connect");
        };
        assert!(matches!(err, Error::Transport(reason) if reason == "mock connect failure"));
        assert_eq!(transport.connect_count(), DIAL_MAX_ATTEMPTS as usize);
    }

    #[tokio::test(start_paused = true)]
    async fn a_dial_slower_than_its_budget_is_the_retry_safe_timeout() {
        let mut transport = MockTransport::new();
        transport.hang = true;
        let (connection, _events) = one_shot(transport);
        let Err(err) = connection.dial().await.expect("no shutdown was signalled") else {
            panic!("a hung handshake must not connect");
        };
        assert!(matches!(err, Error::DialTimeout));
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_server_after_the_send_is_unacknowledged() {
        // The mock's reader is fed by a channel nothing writes to, so only the reply
        // deadline can end the session.
        let (connection, mut events) = one_shot(MockTransport::new());
        connection.run().await;
        let event = events.recv().await.expect("a terminal event");
        assert!(matches!(event, ConnEvent::Error(Error::Unacknowledged)));
    }

    #[tokio::test(start_paused = true)]
    async fn a_one_shot_sends_exactly_once_and_never_redials_after_the_send() {
        // The double-submit guard: whatever happens once the frame is on the wire —
        // here a reply-less server that trips the deadline — the machine must not
        // reconnect and must not re-send.
        let transport = MockTransport::new();
        let (connection, _events) = one_shot(transport.clone());
        connection.run().await;
        assert_eq!(transport.connect_count(), 1, "one dial, never a re-dial");
        assert_eq!(transport.sent().len(), 1, "exactly one frame sent");
    }

    #[tokio::test]
    async fn the_socket_closes_only_after_the_batch_ends() {
        // The double-submit-safe ordering: no Close frame may precede the reply.
        let transport = MockTransport::new();
        let (connection, mut events) = one_shot(transport.clone());
        let session = tokio::spawn(connection.run());
        wait_until(|| !transport.sent().is_empty()).await;
        assert_eq!(
            transport.close_count(),
            0,
            "the request must not race a close"
        );
        transport
            .reader(0)
            .send(ConnEvent::Frame(r#"{"method":"pong","req_id":1}"#.into()))
            .expect("the session is reading");
        let reply = events.recv().await.expect("the reply is forwarded");
        assert!(matches!(reply, ConnEvent::Frame(_)));
        assert_eq!(
            transport.close_count(),
            0,
            "still open while the batch may grow"
        );
        transport
            .reader(0)
            .send(ConnEvent::Closed)
            .expect("the session is reading");
        session.await.expect("the session task must not panic");
        assert_eq!(transport.close_count(), 1, "teardown closes the socket");
    }

    #[tokio::test]
    async fn a_post_send_socket_error_ends_the_session_without_a_retry() {
        let transport = MockTransport::new();
        let (connection, mut events) = one_shot(transport.clone());
        let session = tokio::spawn(connection.run());
        wait_until(|| !transport.sent().is_empty()).await;
        transport
            .reader(0)
            .send(ConnEvent::Error(Error::Transport("reset".into())))
            .expect("the session is reading");
        session.await.expect("the session task must not panic");
        assert_eq!(transport.connect_count(), 1, "one dial, never a re-dial");
        assert_eq!(transport.sent().len(), 1, "exactly one frame sent");
        let mut saw_branded_error = false;
        while let Ok(event) = events.try_recv() {
            // Post-send, so the stop reason must carry the verify-first brand.
            saw_branded_error |= matches!(event, ConnEvent::Error(Error::ReplyLost(_)));
        }
        assert!(
            saw_branded_error,
            "the branded stop reason must reach the gather"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_post_dial_mint_failure_keeps_its_class_and_sends_nothing() {
        // Pre-send: a rejected mint must surface as the credentials problem it is,
        // never as "the request may have reached the exchange".
        let transport = MockTransport::new();
        let cache = Arc::new(TokenCache::new(Arc::new(RejectingSource)));
        let (connection, mut events) = one_shot_with_cache(transport.clone(), Some(cache));
        connection.run().await;
        assert!(transport.sent().is_empty(), "nothing reached the wire");
        let event = events.recv().await.expect("a terminal event");
        let ConnEvent::Error(err) = event else {
            panic!("expected the mint failure, got {event:?}");
        };
        assert!(matches!(err, Error::TokenRejected(_)), "got: {err:?}");
        assert!(
            !err.to_string().contains("verify its state"),
            "a pre-send failure must not carry the verify-first marker: {err}"
        );
    }
}
