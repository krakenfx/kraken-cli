//! The one-shot request/response client — the request-side sibling of
//! [`StreamClient`](crate::StreamClient).
//!
//! The client owns the data: parsing each raw frame, correlating replies, gathering
//! the batch — complete at the request's expected reply count — and typing the
//! outcomes. Everything session-shaped (dial, token, deadlines, the teardown close
//! once the batch is done) belongs to the [`OneShotConnection`] it spawns per send,
//! answered over the events channel exactly as the stream actors answer their client.

use std::sync::Arc;

use kraken_core::endpoint::Endpoint;
use kraken_core::error::{Error, Result};
use kraken_core::{Inbound, MethodResponse, Request};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::config::WsConfig;
use super::connection::{OneShot, OneShotConnection};
use super::token::TokenCache;
use super::transport::ConnEvent;

/// One-shot request/response client. Holds the connection config and its one token
/// cache (shared across sends); every send spawns its own single-use connection.
pub struct RequestClient {
    cfg: WsConfig,
    token: Option<Arc<TokenCache>>,
}

/// Hand-written: the token cache holds a live credential, so only its presence is
/// shown.
impl std::fmt::Debug for RequestClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestClient")
            .field("cfg", &self.cfg)
            .field("token", &self.token.as_ref().map(|_| "<cache>"))
            .finish()
    }
}

impl RequestClient {
    pub fn new(cfg: WsConfig) -> Self {
        Self {
            token: cfg.new_token_cache(),
            cfg,
        }
    }

    /// Submit one request and collect every correlated reply, typed, in arrival order.
    /// Most methods answer once; a multi-id `cancel_order` answers once per id, and each
    /// reply projects into [`Request::Response`] individually, so a half-completed batch
    /// still reports per-id outcomes — with [`ReplyBatch::stopped_short`] carrying the
    /// typed reason the batch is incomplete.
    ///
    /// # Errors
    /// An `Err` means no reply arrived at all. The modes a caller must tell apart
    /// before retrying:
    /// - [`Error::DialTimeout`]: the dial missed its budget — nothing was sent, safe
    ///   to retry.
    /// - [`Error::SendTimeout`] / [`Error::Unacknowledged`] / [`Error::ReplyLost`]: the
    ///   frame may have reached the exchange — verify before retrying.
    /// - [`Error::NoTokenSource`] / [`Error::MintFailed`]: no token source, or the mint
    ///   failed — pre-send, nothing reached the exchange.
    /// - [`Error::Transport`]: the dial failed — nothing was sent.
    pub async fn send<P: Request + Send + Sync + 'static>(
        &self,
        params: P,
        req_id: std::num::NonZeroU64,
    ) -> Result<ReplyBatch<P::Response>> {
        let req_id = req_id.get();
        let endpoint = params.endpoint();
        let expected = params.expected_replies().get();

        // Fail closed before dialing: a token-requiring method with no source is a
        // configuration error, not a session outcome. The connection then holds a
        // cache exactly when its session authenticates — the same invariant the
        // stream client establishes when it plans its endpoints.
        let token_cache = if params.requires_token() {
            Some(self.token.clone().ok_or(Error::NoTokenSource(endpoint))?)
        } else {
            None
        };

        let (events_tx, mut events) = mpsc::unbounded_channel();
        let shutdown = CancellationToken::new();
        let connection = OneShotConnection {
            endpoint,
            transport: self.cfg.transport(endpoint),
            token_cache,
            events_tx,
            shutdown: shutdown.child_token(),
            mode: OneShot { params, req_id },
        };
        // Dropped when this future ends — completed, failed, or cancelled — so the
        // session task never outlives its one caller.
        let _guard = shutdown.drop_guard();
        tokio::spawn(connection.run());

        let (replies, stopped_short) =
            collect_replies(&mut events, endpoint, req_id, expected).await?;
        let outcomes: Vec<Result<P::Response>> = replies
            .into_iter()
            .map(MethodResponse::into_result::<P>)
            .collect();
        // A rejected session token is dead whatever its cache age; drop it so the
        // next send re-mints instead of failing identically until the TTL lapses.
        if let Some(cache) = &self.token
            && saw_token_rejection(&outcomes)
        {
            cache.invalidate().await;
        }
        Ok(ReplyBatch {
            outcomes,
            stopped_short,
        })
    }
}

/// What one send produced: each correlated reply, typed, in arrival order.
#[derive(Debug)]
pub struct ReplyBatch<R> {
    pub outcomes: Vec<Result<R>>,
    /// Why the session ended before every expected reply arrived, when it did — typed,
    /// so a caller classifies the shortfall (verify-first, category) instead of
    /// guessing from counts. `None` when the batch completed.
    pub stopped_short: Option<Error>,
}

/// Whether the venue rejected the session token itself — the cache-invalidation cue.
fn saw_token_rejection<T>(outcomes: &[Result<T>]) -> bool {
    outcomes
        .iter()
        .any(|outcome| matches!(outcome, Err(error) if error.is_token_rejection()))
}

/// A reply correlated to `req_id`, or `None` for anything else on the wire. An id-less
/// *error* reply is kept too: the server rejected before echoing the id, and only our
/// request is in flight on this single-use socket, so it is ours.
fn correlated(frame: Inbound, req_id: u64) -> Option<MethodResponse> {
    match frame {
        Inbound::Method(resp) if resp.req_id == Some(req_id) => Some(resp),
        Inbound::Method(resp) if resp.req_id.is_none() && !resp.is_success() => Some(resp),
        // Listed out (not `_`) so a new `Inbound` variant forces a decision here.
        Inbound::Method(_) | Inbound::Channel(_) | Inbound::Heartbeat => None,
    }
}

/// How a gather run ended.
enum GatherEnd {
    /// The batch completed: `expected` replies, or an id-less whole-frame rejection
    /// (the per-id replies it leaves unanswered were never owed).
    Complete,
    /// The server closed the socket before the batch completed.
    ServerClosed,
    /// The connection reported a terminal stop reason.
    Stopped(Error),
}

/// What one gather run produced.
struct Gathered {
    batch: Vec<MethodResponse>,
    /// The first frame that failed the pinned schema, when one was seen.
    malformed: Option<Error>,
    end: GatherEnd,
}

/// Gather every correlated reply until the batch completes — `expected` replies, or an
/// id-less whole-frame rejection — or the session ends.
///
/// Each raw frame is parsed here — this client's strictness, not the connection's. A
/// frame that fails the pinned schema is recorded (first kept), never fatal: later
/// replies of a batch may still parse.
async fn gather(
    events: &mut mpsc::UnboundedReceiver<ConnEvent>,
    endpoint: Endpoint,
    req_id: u64,
    expected: usize,
) -> Gathered {
    let mut batch = Vec::new();
    let mut malformed = None;
    let end = loop {
        match events.recv().await {
            Some(ConnEvent::Frame(text)) => match text.parse::<Inbound>() {
                Ok(frame) => {
                    if let Some(resp) = correlated(frame, req_id) {
                        // An id-less failure answers the whole frame: no further
                        // replies are owed, however many ids the request carried.
                        let whole_frame = resp.req_id.is_none() && !resp.is_success();
                        batch.push(resp);
                        if batch.len() >= expected || whole_frame {
                            break GatherEnd::Complete;
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "inbound frame failed the pinned schema");
                    malformed.get_or_insert(error);
                }
            },
            Some(ConnEvent::Closed) => break GatherEnd::ServerClosed,
            // The connection brands its stop reasons by phase: pre-send failures
            // (dial, mint) keep their own retry-safe class, post-send ones carry
            // the verify-first marker.
            Some(ConnEvent::Error(reason)) => break GatherEnd::Stopped(reason),
            // The connection emits a terminal event before ending, so a bare channel
            // close means the session task died. The phase is unknowable, so the safe
            // reading is verify-first.
            None => {
                break GatherEnd::Stopped(Error::ReplyLost(Box::new(Error::ConnectionClosed(
                    endpoint,
                ))));
            }
        }
    };
    Gathered {
        batch,
        malformed,
        end,
    }
}

/// Gather replies until the batch completes or the session ends. An empty batch is
/// the shortfall verdict as `Err`; a partial one survives as `Ok` with that same
/// verdict riding along, so the reason it stopped short never vanishes at this seam.
///
/// The verdict for an early end: missing replies are verify-first territory whatever
/// else was seen — a frame arrived only if the request was sent. A recorded malformed
/// frame rides along as the [`Error::ReplyLost`] cause (it may well have been the
/// drifted reply) rather than replacing the verdict: a bare parse error would classify
/// as non-retryable with no instruction to check order state. With nothing recorded,
/// the connection's stop reason, then bare [`Error::Unacknowledged`] for a server that
/// closed without answering. A completed batch takes the early return — no verdict
/// path exists for it to take.
async fn collect_replies(
    events: &mut mpsc::UnboundedReceiver<ConnEvent>,
    endpoint: Endpoint,
    req_id: u64,
    expected: usize,
) -> Result<(Vec<MethodResponse>, Option<Error>)> {
    let Gathered {
        batch,
        malformed,
        end,
    } = gather(events, endpoint, req_id, expected).await;

    let verdict = match (malformed, end) {
        (_, GatherEnd::Complete) => return Ok((batch, None)),
        (Some(drift), _) => Error::ReplyLost(Box::new(drift)),
        (None, GatherEnd::Stopped(stop)) => stop,
        (None, GatherEnd::ServerClosed) => Error::Unacknowledged,
    };
    if batch.is_empty() {
        return Err(verdict);
    }
    Ok((batch, Some(verdict)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gather feed as the connection would deliver it: the scripted events, then the
    /// channel closes (the sender drops here).
    fn feed(events: Vec<ConnEvent>) -> mpsc::UnboundedReceiver<ConnEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        for event in events {
            tx.send(event).expect("the receiver is held");
        }
        rx
    }

    fn frame(s: &str) -> ConnEvent {
        ConnEvent::Frame(s.into())
    }

    fn reply(req_id: u64, order_id: &str) -> ConnEvent {
        frame(&format!(
            r#"{{"method":"cancel_order","success":true,"result":{{"order_id":"{order_id}"}},"req_id":{req_id}}}"#
        ))
    }

    fn error(msg: &str) -> ConnEvent {
        ConnEvent::Error(Error::Transport(msg.into()))
    }

    /// A reply whose result body drifted from the pinned schema (`order_id` as a number).
    const DRIFTED_REPLY: &str =
        r#"{"method":"add_order","success":true,"result":{"order_id":42},"req_id":5}"#;

    #[test]
    fn only_a_token_rejection_triggers_invalidation() {
        let dead: Vec<Result<()>> = vec![Err(Error::FrameRejected("EAPI:Invalid token".into()))];
        assert!(saw_token_rejection(&dead));
        let ordinary: Vec<Result<()>> =
            vec![Err(Error::ServerRejected("EOrder:Unknown order".into()))];
        assert!(
            !saw_token_rejection(&ordinary),
            "an order rejection keeps the token"
        );
    }

    #[tokio::test]
    async fn collects_the_single_reply_ended_by_the_server_close() {
        let mut events = feed(vec![reply(5, "O1"), ConnEvent::Closed]);
        let (batch, stopped_short) = collect_replies(&mut events, Endpoint::Auth, 5, 1)
            .await
            .unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].req_id, Some(5));
        assert!(stopped_short.is_none(), "a complete batch stopped nothing");
    }

    #[tokio::test]
    async fn a_full_batch_ends_the_gather_without_a_server_close() {
        // The close-order inversion: the batch is complete at the expected count, no
        // server close required — the old design needed the server's close as the
        // end-of-batch marker, which raced the reply itself (RFC 6455 lets a server
        // drop pending data once it echoes a Close).
        let mut events = feed(vec![reply(7, "O1"), reply(7, "O2"), reply(7, "O3")]);
        let (batch, _) = collect_replies(&mut events, Endpoint::Auth, 7, 3)
            .await
            .unwrap();
        assert_eq!(batch.len(), 3);
    }

    #[tokio::test]
    async fn an_id_less_rejection_ends_a_multi_id_gather_early() {
        // A whole-frame rejection is the only reply there will ever be: waiting for
        // the other per-id replies would stall until the reply deadline.
        let mut events = feed(vec![frame(
            r#"{"method":"cancel_order","success":false,"error":"EGeneral:Invalid arguments"}"#,
        )]);
        let (batch, stopped_short) = collect_replies(&mut events, Endpoint::Auth, 7, 3)
            .await
            .unwrap();
        assert_eq!(batch.len(), 1);
        assert!(
            stopped_short.is_none(),
            "the per-id replies a whole-frame rejection leaves unanswered were never owed"
        );
    }

    #[tokio::test]
    async fn gathers_every_reply_of_a_multi_id_batch() {
        let mut events = feed(vec![
            reply(7, "O1"),
            reply(7, "O2"),
            reply(7, "O3"),
            ConnEvent::Closed,
        ]);
        let (batch, _) = collect_replies(&mut events, Endpoint::Auth, 7, 3)
            .await
            .unwrap();
        assert_eq!(batch.len(), 3);
        assert!(batch.iter().all(|resp| resp.req_id == Some(7)));
    }

    #[tokio::test]
    async fn a_pong_or_mismatched_id_is_not_counted() {
        let mut events = feed(vec![
            frame(r#"{"method":"pong"}"#),
            reply(99, "OTHER"),
            reply(5, "O1"),
            ConnEvent::Closed,
        ]);
        let (batch, _) = collect_replies(&mut events, Endpoint::Auth, 5, 1)
            .await
            .unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].req_id, Some(5));
    }

    #[tokio::test]
    async fn an_id_less_error_reply_is_ours_on_a_single_request_socket() {
        let mut events = feed(vec![
            frame(r#"{"method":"add_order","success":false,"error":"EGeneral:Invalid arguments"}"#),
            ConnEvent::Closed,
        ]);
        let (batch, _) = collect_replies(&mut events, Endpoint::Auth, 5, 1)
            .await
            .unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(
            batch[0].error.as_deref(),
            Some("EGeneral:Invalid arguments")
        );
    }

    #[tokio::test]
    async fn an_empty_close_is_unacknowledged_never_blindly_retryable() {
        // The server received the request (its close is the end-of-batch marker) and
        // answered nothing: the order may have executed, so no retry-safe error here.
        let mut events = feed(vec![ConnEvent::Closed]);
        let err = collect_replies(&mut events, Endpoint::Auth, 5, 1)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Unacknowledged));
    }

    #[tokio::test]
    async fn a_dead_session_task_is_branded_verify_first() {
        // No terminal event at all: the defensive arm for a task that died mid-session.
        // The phase is unknowable, so the safe reading is "may have been sent".
        let mut events = feed(Vec::new());
        let err = collect_replies(&mut events, Endpoint::Auth, 5, 1)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            Error::ReplyLost(cause) if matches!(*cause, Error::ConnectionClosed(Endpoint::Auth))
        ));
    }

    #[tokio::test]
    async fn a_pre_send_stop_reason_passes_through_with_its_own_class() {
        // The connection brands by phase; the gather must neither add nor strip a
        // brand. A dial failure arrives bare and must stay blindly retryable.
        let mut events = feed(vec![error("connection refused")]);
        let err = collect_replies(&mut events, Endpoint::Auth, 5, 1)
            .await
            .unwrap_err();
        assert!(matches!(&err, Error::Transport(reason) if reason == "connection refused"));
        assert!(
            !err.to_string().contains("verify its state"),
            "a pre-send failure must not carry the verify-first marker: {err}"
        );
    }

    #[tokio::test]
    async fn a_drifted_reply_is_verify_first_carrying_the_parse_detail() {
        // A frame arrived, so the request reached the exchange: the verdict must be
        // verify-first, with the parse failure as its cause — a bare Json error would
        // classify as `parse` ("do not retry") and suppress the check-your-orders
        // instruction.
        let mut events = feed(vec![frame(DRIFTED_REPLY), ConnEvent::Closed]);
        let err = collect_replies(&mut events, Endpoint::Auth, 5, 1)
            .await
            .unwrap_err();
        assert!(err.is_verify_first(), "got: {err:?}");
        assert!(
            matches!(&err, Error::ReplyLost(cause) if matches!(&**cause, Error::Json(_))),
            "the parse detail must survive as the cause: {err:?}"
        );
    }

    #[tokio::test]
    async fn a_drifted_reply_then_the_reply_deadline_still_reports_the_parse_detail() {
        // Silence arrives as the connection's Unacknowledged; the recorded parse
        // failure is the more diagnostic cause and must not vanish behind it.
        let mut events = feed(vec![
            frame(DRIFTED_REPLY),
            ConnEvent::Error(Error::Unacknowledged),
        ]);
        let err = collect_replies(&mut events, Endpoint::Auth, 5, 1)
            .await
            .unwrap_err();
        assert!(err.is_verify_first(), "got: {err:?}");
        assert!(
            matches!(&err, Error::ReplyLost(cause) if matches!(&**cause, Error::Json(_))),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn a_drifted_side_frame_does_not_mask_a_parseable_reply() {
        let mut events = feed(vec![
            frame(DRIFTED_REPLY),
            reply(5, "O1"),
            ConnEvent::Closed,
        ]);
        let (batch, _) = collect_replies(&mut events, Endpoint::Auth, 5, 1)
            .await
            .unwrap();
        assert_eq!(batch.len(), 1);
    }

    #[tokio::test]
    async fn a_branded_stop_reason_keeps_its_marker_and_cause() {
        // What the connection emits for a post-send socket loss: the verify-first
        // brand and the underlying cause must both survive the gather.
        let mut events = feed(vec![ConnEvent::Error(Error::ReplyLost(Box::new(
            Error::Transport("boom".into()),
        )))]);
        let err = collect_replies(&mut events, Endpoint::Auth, 5, 1)
            .await
            .unwrap_err();
        assert!(matches!(
            &err,
            Error::ReplyLost(cause) if matches!(&**cause, Error::Transport(reason) if reason == "boom")
        ));
        assert!(
            err.to_string().contains("verify its state before retrying"),
            "the envelope message must carry the verify-first marker: {err}"
        );
    }

    #[tokio::test]
    async fn a_partial_batch_survives_a_mid_batch_stream_error_with_its_stop_reason() {
        let mut events = feed(vec![reply(7, "O1"), reply(7, "O2"), error("reset")]);
        let (batch, stopped_short) = collect_replies(&mut events, Endpoint::Auth, 7, 3)
            .await
            .unwrap();
        assert_eq!(batch.len(), 2);
        assert!(
            matches!(stopped_short, Some(Error::Transport(reason)) if reason == "reset"),
            "the typed stop reason must ride along with the partial batch"
        );
    }

    #[tokio::test]
    async fn a_token_requiring_method_with_no_source_fails_before_dialing() {
        // No credentials configured: the send must fail closed without a socket.
        let client = RequestClient::new(WsConfig::default());
        let err = client
            .send(kraken_core::CancelAllParams {}, std::num::NonZeroU64::MIN)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::NoTokenSource(Endpoint::Auth)));
    }

    #[tokio::test]
    async fn a_partial_batch_survives_the_reply_deadline_keeping_the_verify_first_verdict() {
        let mut events = feed(vec![
            reply(7, "O1"),
            reply(7, "O2"),
            ConnEvent::Error(Error::Unacknowledged),
        ]);
        let (batch, stopped_short) = collect_replies(&mut events, Endpoint::Auth, 7, 3)
            .await
            .unwrap();
        assert_eq!(batch.len(), 2);
        let stop = stopped_short.expect("the deadline is the reason the batch is short");
        assert!(
            stop.is_verify_first(),
            "the missing replies' verdict must not lose its marker: {stop:?}"
        );
    }

    #[tokio::test]
    async fn a_partial_batch_truncated_by_the_close_brands_the_missing_unacknowledged() {
        // The server closed after answering one of three ids: the other two may have
        // executed, so the shortfall must carry the verify-first verdict — the same
        // one an entirely unanswered request gets — not read as a mere count gap.
        let mut events = feed(vec![reply(7, "O1"), ConnEvent::Closed]);
        let (batch, stopped_short) = collect_replies(&mut events, Endpoint::Auth, 7, 3)
            .await
            .unwrap();
        assert_eq!(batch.len(), 1);
        assert!(matches!(stopped_short, Some(Error::Unacknowledged)));
    }
}
