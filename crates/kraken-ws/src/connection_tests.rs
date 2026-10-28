//! In-process tests for the [`Connection`] actor, driven over the shared
//! [`MockTransport`], so each test drives connect, the fixed-subscription send,
//! replay-on-reconnect, frame decoding, reconnect backoff, the read-idle
//! timeout, and cancellation shutdown without a real socket.

use super::{
    backoff::ReconnectPolicy,
    connection::{Event, Stream, StreamConnection},
    mock_transport::{MockTransport, wait_until},
    token::{BoxFuture, Minted, TokenCache, TokenSource},
    transport::ConnEvent,
};
use kraken_core::{
    Method, WsSubscription,
    endpoint::Endpoint,
    error::{Error, Result},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

// --- mock token source -----------------------------------------------------

struct MockToken(&'static str);

impl TokenSource for MockToken {
    fn fetch(&self) -> BoxFuture<Result<Minted>> {
        let token = self.0.to_string();
        Box::pin(async move { Ok(token.into()) })
    }
}

/// Mints a distinct token per fetch (`tok-0`, `tok-1`, …), so a test can tell
/// which mint a subscribe frame carried.
struct CountingToken {
    calls: Arc<AtomicUsize>,
}

impl TokenSource for CountingToken {
    fn fetch(&self) -> BoxFuture<Result<Minted>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { Ok(format!("tok-{n}").into()) })
    }
}

/// Mints a token on the first fetch, then fails every one after — so the
/// initial connect subscribes but the reconnect's refresh fails.
struct TokenThenFail {
    calls: Arc<AtomicUsize>,
}

impl TokenSource for TokenThenFail {
    fn fetch(&self) -> BoxFuture<Result<Minted>> {
        let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
        Box::pin(async move {
            if first {
                Ok("secret".into())
            } else {
                Err(Error::TokenRejected("refresh failed".into()))
            }
        })
    }
}

/// Fails the first fetch, then mints — so the initial session's setup fails and
/// a test can prove the retry kept first-send semantics.
struct FailThenToken {
    calls: Arc<AtomicUsize>,
}

impl TokenSource for FailThenToken {
    fn fetch(&self) -> BoxFuture<Result<Minted>> {
        let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
        Box::pin(async move {
            if first {
                Err(Error::TokenRejected("first mint failed".into()))
            } else {
                Ok("fresh".into())
            }
        })
    }
}

// --- harness ---------------------------------------------------------------

/// Spawn an actor over `transport` streaming `subs`, returning the shutdown
/// token (cancel it to stop the actor), the event receiver, and the actor
/// task's handle.
fn spawn(
    endpoint: Endpoint,
    transport: MockTransport,
    token_cache: Option<Arc<TokenCache>>,
    subs: Vec<WsSubscription>,
    req_id: Option<std::num::NonZeroU64>,
) -> (
    CancellationToken,
    mpsc::UnboundedReceiver<Event>,
    JoinHandle<()>,
) {
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let shutdown = CancellationToken::new();
    let conn = StreamConnection {
        endpoint,
        transport,
        token_cache,
        events_tx,
        shutdown: shutdown.child_token(),
        mode: Stream {
            policy: fast_policy(),
            subs,
            req_id,
        },
    };
    let handle = tokio::spawn(conn.run());
    (shutdown, events_rx, handle)
}

/// A ready [`TokenCache`] over `source`, hiding the double-`Arc` ceremony.
fn mint_cache(source: impl TokenSource + 'static) -> Arc<TokenCache> {
    Arc::new(TokenCache::new(Arc::new(source)))
}

/// Zero delays so reconnect-driven tests run at full speed.
fn fast_policy() -> ReconnectPolicy {
    ReconnectPolicy {
        initial: Duration::ZERO,
        max: Duration::ZERO,
        jitter_max: Duration::ZERO,
        stable_after: Duration::from_secs(60),
    }
}

/// A raw wire frame as the transport would deliver it: verbatim text.
fn frame(raw: &str) -> ConnEvent {
    ConnEvent::Frame(raw.into())
}

/// Poll `cond` under paused virtual time, alternating yields (letting the actor
/// run) with small clock steps (firing whatever timer it parks on); panics if
/// `cond` never holds within the advanced window.
async fn advance_until(mut cond: impl FnMut() -> bool) {
    for _ in 0..200 {
        for _ in 0..20 {
            if cond() {
                return;
            }
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_millis(100)).await;
    }
    panic!("condition not met within the advanced window");
}

/// Read events until the first that matches `pick`, returning it.
async fn next_matching<T>(
    rx: &mut mpsc::UnboundedReceiver<Event>,
    mut pick: impl FnMut(&Event) -> Option<T>,
) -> T {
    for _ in 0..500 {
        if let Ok(event) = rx.try_recv() {
            if let Some(found) = pick(&event) {
                return found;
            }
        } else {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }
    panic!("expected event never arrived");
}

fn ticker(symbol: &str) -> WsSubscription {
    WsSubscription::Ticker {
        symbol: vec![symbol.into()],
        event_trigger: None,
        snapshot: None,
    }
}

fn balances() -> WsSubscription {
    WsSubscription::Balances {
        snapshot: None,
        rebased: None,
        users: None,
    }
}

// --- tests -----------------------------------------------------------------

#[tokio::test]
async fn the_subscription_set_is_sent_on_connect() {
    let transport = MockTransport::new();
    let (_shutdown, _events, _handle) = spawn(
        Endpoint::Public,
        transport.clone(),
        None,
        vec![ticker("BTC/USD")],
        None,
    );
    wait_until(|| transport.sent().iter().any(|f| f.contains("\"ticker\""))).await;
}

#[tokio::test]
async fn data_frame_reaches_the_consumer_as_a_typed_message() {
    let transport = MockTransport::new();
    let (_shutdown, mut events, _handle) =
        spawn(Endpoint::Public, transport.clone(), None, vec![], None);
    wait_until(|| transport.connect_count() >= 1).await;
    let raw = r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD",
        "side":"buy","price":1.0,"qty":2.0,"ord_type":"market","trade_id":7,"timestamp":"TS"}]}"#;
    transport.reader(0).send(frame(raw)).unwrap();
    let message = next_matching(&mut events, |e| match e {
        Event::Message(m) => Some(m.clone()),
        _ => None,
    })
    .await;
    assert!(serde_json::to_string(&message).unwrap().contains("BTC/USD"));
}

#[tokio::test]
async fn an_unparseable_frame_is_counted_not_fatal() {
    let transport = MockTransport::new();
    let (_shutdown, mut events, _handle) =
        spawn(Endpoint::Public, transport.clone(), None, vec![], None);
    wait_until(|| transport.connect_count() >= 1).await;
    // Schema drift: the frame is dropped, but the loss must surface as an event so
    // sinks and monitors can count the hole — and the session must keep streaming.
    transport
        .reader(0)
        .send(frame(
            r#"{"channel":"ticker","type":"update","data":[{"last":"drift"}]}"#,
        ))
        .unwrap();
    let dropped_on = next_matching(&mut events, |e| match e {
        Event::ParseFailure(endpoint) => Some(*endpoint),
        _ => None,
    })
    .await;
    assert_eq!(dropped_on, Endpoint::Public);
    let raw = r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD",
        "side":"buy","price":1.0,"qty":2.0,"ord_type":"market","trade_id":7,"timestamp":"TS"}]}"#;
    transport.reader(0).send(frame(raw)).unwrap();
    next_matching(&mut events, |e| match e {
        Event::Message(m) => Some(m.clone()),
        _ => None,
    })
    .await;
}

#[tokio::test]
async fn an_unexpected_method_success_is_not_an_ack() {
    let transport = MockTransport::new();
    let (_shutdown, mut events, _handle) =
        spawn(Endpoint::Public, transport.clone(), None, vec![], None);
    wait_until(|| transport.connect_count() >= 1).await;
    // A success this actor never requested (it sends only subscribe + ping): drift,
    // not readiness — surfacing it as an ack would fire session-ready hooks.
    transport
        .reader(0)
        .send(frame(
            r#"{"method":"cancel_all","success":true,"result":{"count":1},"req_id":9}"#,
        ))
        .unwrap();
    transport
        .reader(0)
        .send(frame(r#"{"method":"subscribe","success":true,"req_id":1}"#))
        .unwrap();
    let resp = next_matching(&mut events, |e| match e {
        Event::Ack(r) => Some(r.clone()),
        _ => None,
    })
    .await;
    assert_eq!(resp.method(), Method::Subscribe);
}

#[tokio::test]
async fn a_malformed_frame_does_not_kill_the_stream() {
    let transport = MockTransport::new();
    let (_shutdown, mut events, _handle) =
        spawn(Endpoint::Public, transport.clone(), None, vec![], None);
    wait_until(|| transport.connect_count() >= 1).await;
    let drifted = r#"{"method":"add_order","success":true,"result":{"order_id":42}}"#;
    transport.reader(0).send(frame(drifted)).unwrap();
    transport
        .reader(0)
        .send(frame(r#"{"channel":"heartbeat"}"#))
        .unwrap();
    next_matching(&mut events, |e| matches!(e, Event::Heartbeat).then_some(())).await;
    assert_eq!(transport.connect_count(), 1, "must not reconnect");
}

#[tokio::test]
async fn failed_subscribe_ack_surfaces_as_api_error() {
    let transport = MockTransport::new();
    let (_shutdown, mut events, _handle) =
        spawn(Endpoint::Public, transport.clone(), None, vec![], None);
    wait_until(|| transport.connect_count() >= 1).await;
    let ack = r#"{"method":"subscribe","success":false,"error":"Currency pair not supported","req_id":1}"#;
    transport.reader(0).send(frame(ack)).unwrap();
    let resp = next_matching(&mut events, |e| match e {
        Event::ApiError(r) => Some(r.clone()),
        _ => None,
    })
    .await;
    assert_eq!(resp.error.as_deref(), Some("Currency pair not supported"));
}

#[tokio::test]
async fn a_pong_reply_is_not_surfaced_as_an_ack() {
    let transport = MockTransport::new();
    let (_shutdown, mut events, _handle) =
        spawn(Endpoint::Public, transport.clone(), None, vec![], None);
    wait_until(|| transport.connect_count() >= 1).await;
    transport
        .reader(0)
        .send(frame(r#"{"method":"pong"}"#))
        .unwrap();
    transport
        .reader(0)
        .send(frame(r#"{"method":"subscribe","success":true,"req_id":1}"#))
        .unwrap();
    let resp = next_matching(&mut events, |e| match e {
        Event::Ack(r) => Some(r.clone()),
        _ => None,
    })
    .await;
    assert_eq!(
        resp.method(),
        Method::Subscribe,
        "the pong must not surface as an ack"
    );
}

#[tokio::test]
async fn reconnect_replays_the_subscription_with_a_fresh_token_and_forced_snapshot() {
    let transport = MockTransport::new();
    let (_shutdown, _events, _handle) = spawn(
        Endpoint::Auth,
        transport.clone(),
        Some(mint_cache(MockToken("secret"))),
        vec![balances()],
        None,
    );
    wait_until(|| transport.sent().iter().any(|f| f.contains("balances"))).await;

    transport.reader(0).send(ConnEvent::Closed).unwrap();
    wait_until(|| transport.connect_count() >= 2).await;

    wait_until(|| {
        transport
            .sent()
            .iter()
            .any(|f| f.contains("\"token\":\"secret\"") && f.contains("\"snapshot\":true"))
    })
    .await;
}

#[tokio::test]
async fn a_failed_token_refresh_on_reconnect_backs_off_instead_of_replaying_a_stale_token() {
    let transport = MockTransport::new();
    let (_shutdown, _events, _handle) = spawn(
        Endpoint::Auth,
        transport.clone(),
        Some(mint_cache(TokenThenFail {
            calls: Arc::new(AtomicUsize::new(0)),
        })),
        vec![balances()],
        None,
    );

    wait_until(|| transport.sent().iter().any(|f| f.contains("balances"))).await;
    transport.reader(0).send(ConnEvent::Closed).unwrap();
    wait_until(|| transport.connect_count() >= 3).await;
    assert_eq!(
        transport
            .sent()
            .iter()
            .filter(|f| f.contains("balances"))
            .count(),
        1,
        "a stale token must not be replayed after a failed refresh"
    );
}

#[tokio::test]
async fn a_reconnect_subscribes_with_its_own_fresh_mint() {
    let transport = MockTransport::new();
    let cache = mint_cache(CountingToken {
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let (_shutdown, _events, _handle) = spawn(
        Endpoint::Auth,
        transport.clone(),
        Some(cache),
        vec![balances()],
        None,
    );
    wait_until(|| transport.sent().iter().any(|f| f.contains("tok-0"))).await;

    transport.reader(0).send(ConnEvent::Closed).unwrap();
    wait_until(|| transport.sent().iter().any(|f| f.contains("tok-1"))).await;
}

#[tokio::test]
async fn a_failed_dial_re_mints_the_token_for_the_next_attempt() {
    let transport = MockTransport::new();
    transport.fail_first.store(1, Ordering::SeqCst);
    let cache = mint_cache(CountingToken {
        calls: Arc::new(AtomicUsize::new(0)),
    });
    cache.get().await.unwrap();
    let (_shutdown, _events, _handle) = spawn(
        Endpoint::Auth,
        transport.clone(),
        Some(cache),
        vec![balances()],
        None,
    );

    wait_until(|| transport.sent().iter().any(|f| f.contains("balances"))).await;
    let sent = transport.sent();
    let frame = sent.iter().find(|f| f.contains("balances")).unwrap();
    assert!(
        frame.contains("tok-1"),
        "the session after a failed dial must carry a fresh mint, got: {frame}"
    );
}

#[tokio::test]
async fn a_rejected_subscribe_reconnects_to_resubscribe() {
    let transport = MockTransport::new();
    let (_shutdown, _events, _handle) = spawn(
        Endpoint::Public,
        transport.clone(),
        None,
        vec![ticker("BTC/USD")],
        None,
    );
    wait_until(|| transport.connect_count() >= 1).await;
    let ack = r#"{"method":"subscribe","success":false,"error":"stale token","req_id":1}"#;
    transport.reader(0).send(frame(ack)).unwrap();
    wait_until(|| transport.connect_count() >= 2).await;
    wait_until(|| {
        transport
            .sent()
            .iter()
            .filter(|f| f.contains("\"ticker\""))
            .count()
            >= 2
    })
    .await;
}

#[tokio::test]
async fn a_new_rejection_after_a_cured_incident_gets_its_own_fresh_token_retry() {
    // The incident is per symbol, not per endpoint: after BOGUS/USD's rejection
    // spent its fresh-token retry and the replay was ACKED (the incident is
    // cured), a later rejection of a DIFFERENT pair inside the stability window
    // must earn its own fresh-token reconnect — never count as the old
    // incident's second strike and be evicted unheard.
    let transport = MockTransport::new();
    let (_shutdown, _events, _handle) = spawn(
        Endpoint::Public,
        transport.clone(),
        None,
        vec![WsSubscription::Ticker {
            symbol: vec!["BTC/USD".into(), "ETH/USD".into()],
            event_trigger: None,
            snapshot: None,
        }],
        None,
    );
    wait_until(|| transport.connect_count() >= 1).await;
    transport
        .reader(0)
        .send(frame(
            r#"{"method":"subscribe","success":false,"error":"stale","symbol":"BTC/USD"}"#,
        ))
        .unwrap();
    wait_until(|| transport.connect_count() >= 2).await;
    // The replay is acked: BTC/USD's incident is cured.
    transport
        .reader(1)
        .send(frame(
            r#"{"method":"subscribe","success":true,"result":{"channel":"ticker","symbol":"BTC/USD"}}"#,
        ))
        .unwrap();
    // A different pair rejects within the stability window.
    transport
        .reader(1)
        .send(frame(
            r#"{"method":"subscribe","success":false,"error":"stale","symbol":"ETH/USD"}"#,
        ))
        .unwrap();
    // Its own fresh-token reconnect — not an eviction.
    wait_until(|| transport.connect_count() >= 3).await;
    wait_until(|| {
        transport
            .sent()
            .iter()
            .filter(|f| f.contains("\"ticker\""))
            .count()
            >= 3
    })
    .await;
    let replay = transport
        .sent()
        .into_iter()
        .rev()
        .find(|f| f.contains("\"ticker\""))
        .expect("a replayed subscribe");
    assert!(
        replay.contains("ETH/USD"),
        "ETH/USD must still be in the replay set, not evicted: {replay}"
    );
}

#[tokio::test]
async fn an_anonymous_rejection_after_a_named_incident_gets_its_own_retry() {
    // Named and anonymous incidents are separate axes: a named pair's spent retry
    // must not consume the endpoint's anonymous retry.
    let transport = MockTransport::new();
    let (_shutdown, _events, _handle) = spawn(
        Endpoint::Public,
        transport.clone(),
        None,
        vec![ticker("BTC/USD")],
        None,
    );
    wait_until(|| transport.connect_count() >= 1).await;
    transport
        .reader(0)
        .send(frame(
            r#"{"method":"subscribe","success":false,"error":"stale","symbol":"BTC/USD"}"#,
        ))
        .unwrap();
    wait_until(|| transport.connect_count() >= 2).await;
    transport
        .reader(1)
        .send(frame(
            r#"{"method":"subscribe","success":false,"error":"stale"}"#,
        ))
        .unwrap();
    // The anonymous incident earns a reconnect instead of aborting the endpoint.
    wait_until(|| transport.connect_count() >= 3).await;
}

#[tokio::test]
async fn a_repeat_rejection_evicts_even_when_every_session_is_stable() {
    // The strike must survive its own session's stability clear (record runs after
    // the clear): with stable_after = ZERO every session is "stable", and a
    // repeated rejection must still evict deterministically instead of earning
    // endless retries.
    let transport = MockTransport::new();
    let (events_tx, _events_rx) = mpsc::unbounded_channel();
    let shutdown = CancellationToken::new();
    let conn = StreamConnection {
        endpoint: Endpoint::Public,
        transport: transport.clone(),
        token_cache: None,
        events_tx,
        shutdown: shutdown.child_token(),
        mode: Stream {
            policy: ReconnectPolicy {
                initial: Duration::ZERO,
                max: Duration::ZERO,
                jitter_max: Duration::ZERO,
                stable_after: Duration::ZERO,
            },
            subs: vec![WsSubscription::Ticker {
                symbol: vec!["BTC/USD".into(), "BOGUS/USD".into()],
                event_trigger: None,
                snapshot: None,
            }],
            req_id: None,
        },
    };
    let _handle = tokio::spawn(conn.run());
    let reject = r#"{"method":"subscribe","success":false,"error":"nope","symbol":"BOGUS/USD"}"#;
    wait_until(|| transport.connect_count() >= 1).await;
    transport.reader(0).send(frame(reject)).unwrap();
    wait_until(|| transport.connect_count() >= 2).await;
    transport.reader(1).send(frame(reject)).unwrap();
    // Eviction, not a third fresh-token reconnect: the replay after a close
    // excludes the pair.
    transport.reader(1).send(ConnEvent::Closed).unwrap();
    wait_until(|| {
        transport
            .sent()
            .iter()
            .filter(|f| f.contains("\"ticker\""))
            .count()
            >= 3
    })
    .await;
    let replay = transport
        .sent()
        .into_iter()
        .rev()
        .find(|f| f.contains("\"ticker\""))
        .expect("a replayed subscribe");
    assert!(
        !replay.contains("BOGUS/USD"),
        "the twice-rejected pair must be evicted: {replay}"
    );
    let _ = shutdown;
}

#[tokio::test]
async fn a_cured_pair_rejected_again_gets_a_fresh_retry_not_eviction() {
    // The successful re-subscribe ack closes the incident: a later rejection of the
    // SAME pair is a new problem owed its own fresh-token retry — not the old
    // incident's second strike.
    let transport = MockTransport::new();
    let (_shutdown, _events, _handle) = spawn(
        Endpoint::Public,
        transport.clone(),
        None,
        vec![ticker("BTC/USD")],
        None,
    );
    let reject = r#"{"method":"subscribe","success":false,"error":"stale","symbol":"BTC/USD"}"#;
    wait_until(|| transport.connect_count() >= 1).await;
    transport.reader(0).send(frame(reject)).unwrap();
    wait_until(|| transport.connect_count() >= 2).await;
    // The replay is acked (result carries the pair): the incident is cured.
    transport
        .reader(1)
        .send(frame(
            r#"{"method":"subscribe","success":true,"result":{"channel":"ticker","symbol":"BTC/USD"}}"#,
        ))
        .unwrap();
    // The socket drops (well inside the stability window), then the pair rejects
    // again.
    transport.reader(1).send(ConnEvent::Closed).unwrap();
    wait_until(|| transport.connect_count() >= 3).await;
    transport.reader(2).send(frame(reject)).unwrap();
    // A fresh-token reconnect with the pair still in the replay — not an eviction.
    wait_until(|| transport.connect_count() >= 4).await;
    wait_until(|| {
        transport
            .sent()
            .iter()
            .filter(|f| f.contains("\"ticker\""))
            .count()
            >= 4
    })
    .await;
    let replay = transport
        .sent()
        .into_iter()
        .rev()
        .find(|f| f.contains("\"ticker\""))
        .expect("a replayed subscribe");
    assert!(
        replay.contains("BTC/USD"),
        "the cured pair must stay in the replay: {replay}"
    );
}

#[tokio::test]
async fn a_repeat_rejection_evicts_the_subscription_from_the_replay() {
    let transport = MockTransport::new();
    let (_shutdown, mut events, _handle) = spawn(
        Endpoint::Public,
        transport.clone(),
        None,
        vec![WsSubscription::Ticker {
            symbol: vec!["BTC/USD".into(), "BOGUS/USD".into()],
            event_trigger: None,
            snapshot: None,
        }],
        None,
    );
    let reject = r#"{"method":"subscribe","success":false,"error":"Currency pair not supported BOGUS/USD","symbol":"BOGUS/USD"}"#;
    wait_until(|| transport.connect_count() >= 1).await;
    transport.reader(0).send(frame(reject)).unwrap();
    wait_until(|| transport.connect_count() >= 2).await;
    transport.reader(1).send(frame(reject)).unwrap();
    transport
        .reader(1)
        .send(frame(r#"{"channel":"heartbeat"}"#))
        .unwrap();
    next_matching(&mut events, |e| matches!(e, Event::Heartbeat).then_some(())).await;
    assert_eq!(transport.connect_count(), 2);
    transport.reader(1).send(ConnEvent::Closed).unwrap();
    wait_until(|| {
        transport
            .sent()
            .iter()
            .filter(|f| f.contains("\"ticker\""))
            .count()
            >= 3
    })
    .await;
    let replay = transport
        .sent()
        .into_iter()
        .rev()
        .find(|f| f.contains("\"ticker\""))
        .expect("a replayed ticker subscribe");
    assert!(replay.contains("BTC/USD"));
    assert!(!replay.contains("BOGUS/USD"));
}

#[tokio::test]
async fn all_subscriptions_rejected_twice_stop_the_actor() {
    let transport = MockTransport::new();
    let (_shutdown, mut events, _handle) = spawn(
        Endpoint::Public,
        transport.clone(),
        None,
        vec![ticker("BOGUS/USD")],
        None,
    );
    let reject = r#"{"method":"subscribe","success":false,"error":"Currency pair not supported BOGUS/USD","symbol":"BOGUS/USD"}"#;
    wait_until(|| transport.connect_count() >= 1).await;
    transport.reader(0).send(frame(reject)).unwrap();
    wait_until(|| transport.connect_count() >= 2).await;
    transport.reader(1).send(frame(reject)).unwrap();
    let mut aborted = false;
    wait_until(|| {
        while let Ok(event) = events.try_recv() {
            aborted |= matches!(event, Event::Aborted { .. });
        }
        events.is_closed()
    })
    .await;
    assert!(
        aborted,
        "the terminal give-up must surface as Event::Aborted"
    );
    assert_eq!(transport.connect_count(), 2);
}

#[tokio::test]
async fn a_sole_symbol_less_subscription_rejected_twice_stops_the_actor() {
    let transport = MockTransport::new();
    let (_shutdown, mut events, _handle) = spawn(
        Endpoint::Auth,
        transport.clone(),
        Some(mint_cache(MockToken("secret"))),
        vec![balances()],
        None,
    );
    let reject = r#"{"method":"subscribe","success":false,"error":"EGeneral:Permission denied"}"#;
    wait_until(|| transport.connect_count() >= 1).await;
    transport.reader(0).send(frame(reject)).unwrap();
    wait_until(|| transport.connect_count() >= 2).await;
    transport.reader(1).send(frame(reject)).unwrap();
    wait_until(|| {
        while events.try_recv().is_ok() {}
        events.is_closed()
    })
    .await;
    assert_eq!(transport.connect_count(), 2);
}

#[tokio::test]
async fn two_symbol_less_subscriptions_both_rejected_twice_stop_the_actor() {
    // Two account channels ride one Auth socket. When the fresh-token retry rejects
    // both without naming a pair, nothing is streaming — the actor must abort, not
    // idle forever on its own keepalive pongs.
    let transport = MockTransport::new();
    let (_shutdown, mut events, _handle) = spawn(
        Endpoint::Auth,
        transport.clone(),
        Some(mint_cache(MockToken("secret"))),
        vec![executions(), balances()],
        None,
    );
    let reject = r#"{"method":"subscribe","success":false,"error":"EGeneral:Permission denied"}"#;
    wait_until(|| transport.connect_count() >= 1).await;
    transport.reader(0).send(frame(reject)).unwrap();
    wait_until(|| transport.connect_count() >= 2).await;
    transport.reader(1).send(frame(reject)).unwrap();
    transport.reader(1).send(frame(reject)).unwrap();
    let mut aborted = false;
    wait_until(|| {
        while let Ok(event) = events.try_recv() {
            aborted |= matches!(event, Event::Aborted { .. });
        }
        events.is_closed()
    })
    .await;
    assert!(
        aborted,
        "an all-channels-rejected socket must abort, not hang"
    );
    assert_eq!(transport.connect_count(), 2);
}

#[tokio::test]
async fn one_dead_channel_among_live_ones_does_not_stop_the_actor() {
    // Only one of the two account channels is rejected on the fresh token: the
    // healthy one keeps streaming and the socket stays up.
    let transport = MockTransport::new();
    let (_shutdown, mut events, _handle) = spawn(
        Endpoint::Auth,
        transport.clone(),
        Some(mint_cache(MockToken("secret"))),
        vec![executions(), balances()],
        None,
    );
    let reject = r#"{"method":"subscribe","success":false,"error":"EGeneral:Permission denied"}"#;
    wait_until(|| transport.connect_count() >= 1).await;
    transport.reader(0).send(frame(reject)).unwrap();
    wait_until(|| transport.connect_count() >= 2).await;
    transport.reader(1).send(frame(reject)).unwrap();
    transport
        .reader(1)
        .send(frame(r#"{"channel":"heartbeat"}"#))
        .unwrap();
    next_matching(&mut events, |e| matches!(e, Event::Heartbeat).then_some(())).await;
    assert_eq!(transport.connect_count(), 2, "must not reconnect or abort");
}

fn executions() -> WsSubscription {
    WsSubscription::Executions {
        snap_trades: None,
        snap_orders: None,
        order_status: None,
        rebased: None,
        ratecounter: None,
        users: None,
    }
}

#[tokio::test]
async fn a_setup_failure_retries_as_a_first_send_not_a_replay() {
    let transport = MockTransport::new();
    let (_shutdown, _events, _handle) = spawn(
        Endpoint::Auth,
        transport.clone(),
        Some(mint_cache(FailThenToken {
            calls: Arc::new(AtomicUsize::new(0)),
        })),
        vec![balances()],
        std::num::NonZeroU64::new(7),
    );
    wait_until(|| transport.sent().iter().any(|f| f.contains("\"balances\""))).await;
    let subscribe = transport
        .sent()
        .into_iter()
        .find(|f| f.contains("\"balances\""))
        .expect("a subscribe frame");
    assert!(
        subscribe.contains("\"req_id\":7"),
        "a first send echoes the caller's req_id: {subscribe}"
    );
    assert!(
        !subscribe.contains("\"snapshot\":true"),
        "a first send keeps its own snapshot preference: {subscribe}"
    );
}

#[tokio::test]
async fn connect_failures_do_not_stop_the_actor() {
    let mut transport = MockTransport::new();
    transport.fail = true;
    let (_shutdown, _events, _handle) =
        spawn(Endpoint::Public, transport.clone(), None, vec![], None);
    wait_until(|| transport.connect_count() >= 3).await;
}

#[tokio::test]
async fn a_cancel_during_a_hung_connect_shuts_the_actor_down() {
    let mut transport = MockTransport::new();
    transport.hang = true;
    let (shutdown, _events, handle) =
        spawn(Endpoint::Public, transport.clone(), None, vec![], None);

    wait_until(|| transport.connect_count() >= 1).await;
    shutdown.cancel(); // handle gone while the handshake is hung

    tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("actor must shut down promptly during a hung connect")
        .expect("actor task panicked");
}

#[tokio::test]
async fn the_actor_stops_when_every_consumer_drops() {
    let transport = MockTransport::new();
    let (_shutdown, events, handle) =
        spawn(Endpoint::Public, transport.clone(), None, vec![], None);
    wait_until(|| transport.connect_count() >= 1).await;

    drop(events); // the event stream dropped while the socket is live
    let _ = transport
        .reader(0)
        .send(frame(r#"{"channel":"heartbeat"}"#));

    tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("actor must stop once every consumer is gone")
        .expect("actor task panicked");
}

#[tokio::test(start_paused = true)]
async fn a_session_past_stable_after_resets_the_backoff_schedule() {
    // Two instant drops escalate the schedule (1s, then 2s, base now 4s); a session
    // that then streams past `stable_after` must reset it, so the next reconnect
    // waits `initial` again instead of the escalated 4s.
    let transport = MockTransport::new();
    let (events_tx, _events_rx) = mpsc::unbounded_channel();
    let shutdown = CancellationToken::new();
    let conn = StreamConnection {
        endpoint: Endpoint::Public,
        transport: transport.clone(),
        token_cache: None,
        events_tx,
        shutdown: shutdown.child_token(),
        mode: Stream {
            policy: ReconnectPolicy {
                initial: Duration::from_secs(1),
                max: Duration::from_secs(8),
                jitter_max: Duration::ZERO,
                stable_after: Duration::from_secs(5),
            },
            subs: vec![],
            req_id: None,
        },
    };
    let _handle = tokio::spawn(conn.run());

    advance_until(|| transport.connect_count() >= 1).await;
    transport.reader(0).send(ConnEvent::Closed).unwrap();
    advance_until(|| transport.connect_count() >= 2).await;
    let second_drop = tokio::time::Instant::now();
    transport.reader(1).send(ConnEvent::Closed).unwrap();
    advance_until(|| transport.connect_count() >= 3).await;
    assert!(
        second_drop.elapsed() >= Duration::from_secs(2),
        "the instant drops must have escalated the schedule, or the reset below pins nothing"
    );

    tokio::time::advance(Duration::from_secs(6)).await; // stream past stable_after
    let stable_drop = tokio::time::Instant::now();
    transport.reader(2).send(ConnEvent::Closed).unwrap();
    advance_until(|| transport.connect_count() >= 4).await;
    assert!(
        stable_drop.elapsed() < Duration::from_secs(2),
        "a stable session must reset the reconnect delay to initial, waited {:?}",
        stable_drop.elapsed()
    );
}

#[tokio::test(start_paused = true)]
async fn a_silent_socket_reconnects_after_the_read_idle_timeout() {
    let transport = MockTransport::new();
    let (_shutdown, _events, _handle) =
        spawn(Endpoint::Public, transport.clone(), None, vec![], None);
    while transport.connect_count() < 1 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(super::connection::READ_IDLE_TIMEOUT + Duration::from_secs(1)).await;
    while transport.connect_count() < 2 {
        tokio::task::yield_now().await;
    }
}
