//! The Futures client end to end against a mock exchange.

use super::{
    FuturesRest, PathSegment, Reply,
    account::{AccountsQuery, FillsQuery, SetLeveragePreference},
    history::{AccountLogCsv, ExecutionsQuery},
    market::{TickerQuery, Tickers, TickersQuery},
    trading::{FuturesOrderKind, OrdersStatusQuery, SendOrder},
};
use crate::{RestError, auth::test_auth, test_venue::silent_venue, transport::Transport};
use kraken_core::OrderSide;
use rust_decimal_macros::dec;
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header_exists, method, path, query_param},
};

fn client(root: impl Into<String>) -> FuturesRest {
    FuturesRest::new(Transport::for_tests(), Some(root.into()))
}

fn limit_buy() -> SendOrder {
    SendOrder {
        symbol: "PF_XBTUSD".into(),
        side: OrderSide::Buy,
        size: dec!(1),
        kind: FuturesOrderKind::Limit { price: dec!(50000) },
        cli_ord_id: None,
        reduce_only: false,
    }
}

fn success(body: serde_json::Value) -> ResponseTemplate {
    let mut reply = json!({"result": "success", "serverTime": "2026-10-20T12:00:00.000Z"});
    for (key, value) in body.as_object().unwrap() {
        reply[key] = value.clone();
    }
    ResponseTemplate::new(200).set_body_json(reply)
}

#[test]
fn the_history_root_sits_beside_the_trading_root() {
    let futures = client("https://futures.kraken.com/derivatives/api/v3");
    assert_eq!(
        futures.history_root,
        "https://futures.kraken.com/api/history/v3"
    );
    let local = client("http://127.0.0.1:9");
    assert_eq!(local.history_root, "http://127.0.0.1:9/api/history/v3");
}

#[tokio::test]
async fn an_order_is_a_signed_form_post_sent_once_after_a_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sendorder"))
        .and(header_exists("APIKey"))
        .and(header_exists("Nonce"))
        .and(header_exists("Authent"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let err = client(server.uri())
        .authenticate(test_auth())
        .sign(&limit_buy())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, RestError::Unconfirmed(cause)
            if matches!(**cause, RestError::Status { status, .. } if status == 503)),
        "{err:?}"
    );
    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        std::str::from_utf8(&request.body).unwrap(),
        "orderType=lmt&symbol=PF_XBTUSD&side=buy&size=1&limitPrice=50000"
    );
}

#[tokio::test]
async fn a_get_carries_its_parameters_in_the_query() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/fills"))
        .and(query_param("lastFillTime", "2026-01-01T00:00:00Z"))
        .respond_with(success(json!({"fills": []})))
        .expect(1)
        .mount(&server)
        .await;
    let query = FillsQuery {
        last_fill_time: Some("2026-01-01T00:00:00Z".into()),
    };
    client(server.uri())
        .authenticate(test_auth())
        .sign(&query)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_path_parameter_fills_the_path() {
    let server = MockServer::start().await;
    Mock::given(path("/tickers/PF_XBTUSD"))
        .respond_with(success(json!({
            "ticker": {"symbol": "in_xbtusd", "last": 1, "lastTime": "t"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let query = TickerQuery {
        symbol: PathSegment::new("symbol", "PF_XBTUSD").unwrap(),
    };
    client(server.uri()).query(&query).await.unwrap();
}

#[tokio::test]
async fn a_history_query_reaches_the_history_api() {
    let server = MockServer::start().await;
    Mock::given(path("/api/history/v3/executions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "accountUid": "a", "len": 0, "elements": []
        })))
        .expect(1)
        .mount(&server)
        .await;
    client(server.uri())
        .authenticate(test_auth())
        .sign(&ExecutionsQuery::default())
        .await
        .unwrap();
}

#[tokio::test]
async fn an_inline_error_is_a_rejection() {
    let server = MockServer::start().await;
    Mock::given(path("/tickers"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": "error", "error": "apiLimitExceeded", "serverTime": "t"
        })))
        .mount(&server)
        .await;
    let err = client(server.uri())
        .query(&TickersQuery {})
        .await
        .unwrap_err();
    assert!(
        matches!(&err, RestError::Rejected(code) if code == "apiLimitExceeded"),
        "{err}"
    );
}

#[tokio::test]
async fn a_reply_keeps_its_outcome_and_clock() {
    let server = MockServer::start().await;
    Mock::given(path("/tickers"))
        .respond_with(success(json!({"tickers": []})))
        .mount(&server)
        .await;
    let tickers: Reply<Tickers> = client(server.uri()).query(&TickersQuery {}).await.unwrap();
    let published = serde_json::to_value(&tickers).unwrap();
    assert_eq!(published["result"], "success");
    assert_eq!(published["serverTime"], "2026-10-20T12:00:00.000Z");
}

#[tokio::test]
async fn an_order_that_times_out_is_sent_once() {
    let (base_url, received) = silent_venue().await;
    client(base_url)
        .authenticate(test_auth())
        .sign(&limit_buy())
        .await
        .expect_err("the venue never answers");
    assert_eq!(
        received.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a resend would place a second order"
    );
}

#[tokio::test]
async fn a_read_retries_a_server_error_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(path("/accounts"))
        .respond_with(ResponseTemplate::new(502))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/accounts"))
        .respond_with(success(json!({"accounts": {}})))
        .mount(&server)
        .await;
    client(server.uri())
        .authenticate(test_auth())
        .sign(&AccountsQuery {})
        .await
        .unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

/// A throttled reply is final: resending it would only dig the hole deeper.
#[tokio::test]
async fn a_rejection_with_an_error_status_is_never_resent() {
    let server = MockServer::start().await;
    Mock::given(path("/accounts"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "result": "error", "error": "apiLimitExceeded", "serverTime": "t"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let err = client(server.uri())
        .authenticate(test_auth())
        .sign(&AccountsQuery {})
        .await
        .unwrap_err();
    assert!(
        matches!(&err, RestError::Rejected(code) if code == "apiLimitExceeded"),
        "{err}"
    );
}

#[tokio::test]
async fn a_setter_retries_a_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/leveragepreferences"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/leveragepreferences"))
        .respond_with(success(json!({})))
        .mount(&server)
        .await;
    let request = SetLeveragePreference {
        symbol: "PF_XBTUSD".into(),
        max_leverage: Some(dec!(5)),
    };
    client(server.uri())
        .authenticate(test_auth())
        .sign(&request)
        .await
        .unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

/// Bad bytes in the export are a data problem, never an auth one.
#[tokio::test]
async fn a_csv_that_is_not_utf8_is_not_text() {
    let server = MockServer::start().await;
    Mock::given(path("/api/history/v3/accountlogcsv"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'a', 0xff, b'\n']))
        .mount(&server)
        .await;
    let err = client(server.uri())
        .authenticate(test_auth())
        .download(&AccountLogCsv::default())
        .await
        .unwrap_err();
    assert!(matches!(err, RestError::NotText { .. }), "{err:?}");
}

/// A read Kraken takes as a POST is resent like any other read.
#[tokio::test]
async fn an_order_status_lookup_retries_a_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/orders/status"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/orders/status"))
        .respond_with(success(json!({"orders": []})))
        .mount(&server)
        .await;
    let query = OrdersStatusQuery {
        order_ids: vec!["o1".to_owned()].into(),
    };
    client(server.uri())
        .authenticate(test_auth())
        .sign(&query)
        .await
        .unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}
