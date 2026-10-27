//! The Spot client end to end against a mock exchange: delivery, signing,
//! the envelope, and what a trace log may see.

use super::{
    Endpoint, Private, SpotRest,
    account::{BalanceQuery, RetrieveExport},
    market::ServerTimeQuery,
    sealed::Sealed,
    trading::{AddOrder, CancelOrderBatch, OrderKind},
};
use crate::{
    ClientOptions, CommaList, Identity, RestError,
    auth::test_auth,
    test_venue::{Answer, scripted_venue, silent_venue},
    transport::Transport,
};
use kraken_core::OrderSide;
use rust_decimal_macros::dec;
use serde_json::json;
use std::sync::atomic::Ordering;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, header, header_exists, method, path},
};

fn client(base_url: impl Into<String>) -> SpotRest {
    SpotRest::new(Transport::for_tests(), Some(base_url.into()))
}

fn market_buy() -> AddOrder {
    AddOrder {
        pair: "XBTUSD".into(),
        side: OrderSide::Buy,
        volume: dec!(0.001),
        kind: OrderKind::Market,
        leverage: None,
        reduce_only: false,
        time_in_force: None,
        starttm: None,
        expiretm: None,
        tag: None,
        oflags: CommaList::default(),
        stptype: None,
        close: None,
        deadline: None,
        validate: false,
        asset_class: None,
    }
}

#[tokio::test]
async fn an_order_that_times_out_is_sent_once() {
    let (base_url, received) = silent_venue().await;
    let err = client(base_url)
        .authenticate(test_auth())
        .sign(&market_buy())
        .await
        .expect_err("the venue never answers");
    assert_eq!(
        received.load(Ordering::SeqCst),
        1,
        "a resend would place a second order"
    );
    assert!(matches!(err, RestError::Unconfirmed(_)), "{err:?}");
}

#[tokio::test]
async fn an_order_to_an_unreachable_venue_is_a_plain_failure() {
    // Nothing can listen on port 0, so no parallel test can take it over.
    let err = client("http://127.0.0.1:0")
        .authenticate(test_auth())
        .sign(&market_buy())
        .await
        .unwrap_err();
    assert!(matches!(err, RestError::Transport(_)), "{err:?}");
}

#[tokio::test]
async fn a_read_that_times_out_is_resent() {
    let (base_url, received) = silent_venue().await;
    client(base_url)
        .authenticate(test_auth())
        .sign(&BalanceQuery::default())
        .await
        .expect_err("the venue never answers");
    assert_eq!(
        received.load(Ordering::SeqCst),
        4,
        "one send and three retries"
    );
}

#[tokio::test]
async fn a_read_whose_reply_is_cut_off_is_resent() {
    let (base_url, received) = scripted_venue(|n| match n {
        0 => Answer::CutOff,
        _ => Answer::Reply(r#"{"error":[],"result":{"ZUSD":"1.0"}}"#),
    })
    .await;
    client(base_url)
        .authenticate(test_auth())
        .sign(&BalanceQuery::default())
        .await
        .unwrap();
    assert_eq!(received.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_order_whose_reply_is_cut_off_is_sent_once() {
    let (base_url, received) = scripted_venue(|_| Answer::CutOff).await;
    client(base_url)
        .authenticate(test_auth())
        .sign(&market_buy())
        .await
        .expect_err("the reply never arrives whole");
    assert_eq!(
        received.load(Ordering::SeqCst),
        1,
        "the venue answered, so the order exists"
    );
}

#[tokio::test]
async fn a_read_retries_a_server_error_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(path("/0/public/Time"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(path("/0/public/Time"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": [], "result": {"unixtime": 1, "rfc1123": "now"}
        })))
        .mount(&server)
        .await;
    let time = client(server.uri())
        .query(&ServerTimeQuery::default())
        .await
        .unwrap();
    assert_eq!(time.unixtime, 1);
}

#[tokio::test]
async fn an_order_is_not_retried_after_a_server_error() {
    let server = MockServer::start().await;
    Mock::given(path("/0/private/AddOrder"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let err = client(server.uri())
        .authenticate(test_auth())
        .sign(&market_buy())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, RestError::Unconfirmed(cause)
            if matches!(**cause, RestError::Status { status, .. } if status == 503)),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_private_call_is_a_signed_json_post() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/0/private/Balance"))
        .and(header("API-Key", "test-api-key"))
        .and(header_exists("API-Sign"))
        .and(header("Content-Type", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": [], "result": {"ZUSD": "1.0"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    client(server.uri())
        .authenticate(test_auth())
        .sign(&BalanceQuery::default())
        .await
        .unwrap();
    let request = &server.received_requests().await.unwrap()[0];
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert!(body["nonce"].is_u64(), "{body}");
}

#[tokio::test]
async fn a_rejection_inside_a_200_is_final() {
    let server = MockServer::start().await;
    Mock::given(path("/0/private/Balance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": ["EAPI:Invalid key"]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let err = client(server.uri())
        .authenticate(test_auth())
        .sign(&BalanceQuery::default())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, RestError::Rejected(code) if code == "EAPI:Invalid key"),
        "{err}"
    );
}

#[tokio::test]
async fn a_gateway_page_is_a_status_error_with_an_excerpt() {
    let server = MockServer::start().await;
    Mock::given(path("/0/private/Balance"))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>bad gateway</html>"))
        .mount(&server)
        .await;
    let err = client(server.uri())
        .authenticate(test_auth())
        .sign(&BalanceQuery::default())
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "HTTP 502 Bad Gateway: <html>bad gateway</html>"
    );
}

#[tokio::test]
async fn a_drifted_reply_names_the_field() {
    let server = MockServer::start().await;
    Mock::given(path("/0/public/Time"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": [], "result": {"unixtime": "soon", "rfc1123": "now"}
        })))
        .mount(&server)
        .await;
    let err = client(server.uri())
        .query(&ServerTimeQuery::default())
        .await
        .unwrap_err();
    assert!(err.to_string().starts_with("Time: unixtime:"), "{err}");
}

/// Kraken accepted the order, so a drifted reply proves nothing failed.
#[tokio::test]
async fn an_order_whose_reply_drifted_is_unconfirmed() {
    let server = MockServer::start().await;
    Mock::given(path("/0/private/AddOrder"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": [], "result": {"txid": ["OABC-123"]}
        })))
        .mount(&server)
        .await;
    let err = client(server.uri())
        .authenticate(test_auth())
        .sign(&market_buy())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, RestError::Unconfirmed(cause) if matches!(**cause, RestError::Decode { .. })),
        "{err:?}"
    );
}

/// A 5xx proves nothing about a write, even one naming a Kraken error, and a
/// resend under a fresh nonce would place it again.
#[tokio::test]
async fn an_order_rejected_with_a_server_error_is_unconfirmed() {
    let server = MockServer::start().await;
    Mock::given(path("/0/private/AddOrder"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "error": ["EGeneral:Internal error"]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let err = client(server.uri())
        .authenticate(test_auth())
        .sign(&market_buy())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, RestError::Unconfirmed(cause) if matches!(**cause, RestError::Rejected(_))),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_rejected_order_is_not_unconfirmed() {
    let server = MockServer::start().await;
    Mock::given(path("/0/private/AddOrder"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": ["EOrder:Insufficient funds"]
        })))
        .mount(&server)
        .await;
    let err = client(server.uri())
        .authenticate(test_auth())
        .sign(&market_buy())
        .await
        .unwrap_err();
    assert!(matches!(err, RestError::Rejected(_)), "{err:?}");
}

/// Everything emitted at `TRACE` while `f` runs, on a thread-local subscriber
/// so parallel tests cannot interleave.
fn capture_trace(f: impl FnOnce()) -> String {
    use std::{
        io::Write,
        sync::{Arc, Mutex},
    };
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> MakeWriter<'a> for Buf {
        type Writer = Buf;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    let sink = Buf(Arc::new(Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(sink.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let bytes = sink.0.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn a_private_body_never_reaches_a_trace_log() {
    let reply = crate::transport::Reply {
        status: reqwest::StatusCode::OK,
        body: br#"{"error":[],"result":{"ZUSD":"123456.7890"}}"#.to_vec(),
    };
    let logged = capture_trace(|| reply.trace(crate::transport::BodyLog::SizeOnly));
    assert!(!logged.contains("123456.7890"), "{logged}");
    assert!(logged.contains("body_bytes"), "{logged}");
}

#[test]
fn a_public_body_is_traced_for_diagnostics() {
    let reply = crate::transport::Reply {
        status: reqwest::StatusCode::OK,
        body: br#"{"error":[],"result":{"XXBTZUSD":{"c":["50000.0","1.0"]}}}"#.to_vec(),
    };
    let logged = capture_trace(|| reply.trace(crate::transport::BodyLog::Full));
    assert!(logged.contains("50000.0"), "{logged}");
}

fn balance_reply() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"error": [], "result": {"ZUSD": "1.0"}}))
}

#[tokio::test]
async fn the_second_factor_rides_in_the_signed_body() {
    let server = MockServer::start().await;
    Mock::given(path("/0/private/Balance"))
        .and(body_string_contains(r#""otp":"123456""#))
        .respond_with(balance_reply())
        .expect(1)
        .mount(&server)
        .await;
    client(server.uri())
        .authenticate(test_auth())
        .with_otp(Some("123456".into()))
        .sign(&BalanceQuery::default())
        .await
        .unwrap();
}

/// A replayed nonce is refused by Kraken, so a resend must be signed afresh.
#[tokio::test]
async fn each_resend_carries_a_fresh_nonce_and_signature() {
    let server = MockServer::start().await;
    Mock::given(path("/0/private/Balance"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/0/private/Balance"))
        .respond_with(balance_reply())
        .mount(&server)
        .await;
    client(server.uri())
        .authenticate(test_auth())
        .sign(&BalanceQuery::default())
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let nonce = |i: usize| {
        let body: serde_json::Value = serde_json::from_slice(&requests[i].body).unwrap();
        body["nonce"].as_u64().unwrap()
    };
    assert_eq!(requests.len(), 2);
    assert!(nonce(1) > nonce(0));
    assert_ne!(
        requests[0].headers["API-Sign"],
        requests[1].headers["API-Sign"]
    );
}

#[tokio::test]
async fn every_request_names_its_caller() {
    let server = MockServer::start().await;
    Mock::given(path("/0/public/Time"))
        .and(header("user-agent", "kraken-cli/test (claude)"))
        .and(header("x-korigin", "u004"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": [], "result": {"unixtime": 1, "rfc1123": "Thu,  1 Jan 70 00:00:01 +0000"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let transport = Transport::new(&ClientOptions {
        identity: Identity {
            user_agent: "kraken-cli/test (claude)".into(),
            headers: vec![("x-korigin".into(), "u004".into())],
        },
        ..ClientOptions::default()
    })
    .unwrap();
    SpotRest::new(transport, Some(server.uri()))
        .query(&ServerTimeQuery::default())
        .await
        .unwrap();
}

#[tokio::test]
async fn an_export_downloads_as_its_file_unless_rejected() {
    let server = MockServer::start().await;
    Mock::given(path("/0/private/RetrieveExport"))
        .and(body_string_contains(r#""id":"ready""#))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"PK\x03\x04zip".to_vec()))
        .mount(&server)
        .await;
    Mock::given(path("/0/private/RetrieveExport"))
        .and(body_string_contains(r#""id":"gone""#))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": ["EGeneral:Invalid arguments"]
        })))
        .mount(&server)
        .await;
    let account = client(server.uri()).authenticate(test_auth());
    let file = account
        .download(&RetrieveExport { id: "ready".into() })
        .await
        .unwrap();
    assert_eq!(file, b"PK\x03\x04zip");
    let err = account
        .download(&RetrieveExport { id: "gone".into() })
        .await
        .unwrap_err();
    assert!(
        matches!(&err, RestError::Rejected(code) if code == "EGeneral:Invalid arguments"),
        "{err}"
    );
}

#[tokio::test]
async fn a_batch_cancel_is_not_retried_after_a_server_error() {
    let server = MockServer::start().await;
    Mock::given(path("/0/private/CancelOrderBatch"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let request = CancelOrderBatch {
        orders: vec!["OABC12-DEFGH-IJKLMN".into()],
        cl_ord_ids: vec![],
    };
    client(server.uri())
        .authenticate(test_auth())
        .sign(&request)
        .await
        .unwrap_err();
}

/// A new endpoint that names no delivery, as a forgotten override would.
#[derive(serde::Serialize)]
struct UndeclaredEndpoint;

impl Sealed for UndeclaredEndpoint {}
impl Endpoint for UndeclaredEndpoint {
    const PATH: &'static str = "Undeclared";
    type Access = Private;
    type Response = bool;
}

#[tokio::test]
async fn an_endpoint_that_declares_no_delivery_is_sent_once() {
    let (base_url, received) = silent_venue().await;
    let err = client(base_url)
        .authenticate(test_auth())
        .sign(&UndeclaredEndpoint)
        .await
        .expect_err("the venue never answers");
    assert_eq!(
        received.load(Ordering::SeqCst),
        1,
        "the default fails closed"
    );
    assert!(matches!(err, RestError::Unconfirmed(_)), "{err:?}");
}
