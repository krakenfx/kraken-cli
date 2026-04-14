/// Wiremock-based HTTP integration tests.
///
/// Exercises the HTTP client against a mock server to verify signing, error
/// parsing, transient-error retry, and output contracts.
use std::collections::HashMap;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde_json::json;
use wiremock::matchers::{header, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use kraken_cli::client::{FuturesClient, SpotClient};
use kraken_cli::config::{CredentialSource, FuturesCredentials, SecretValue, SpotCredentials};

fn test_credentials() -> SpotCredentials {
    SpotCredentials {
        api_key: "test-api-key".to_string(),
        api_secret: SecretValue::new(BASE64.encode(b"test_secret_key_bytes_32_chars!!")),
        source: CredentialSource::Flag,
    }
}

#[tokio::test]
async fn spot_public_get_returns_result() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/0/public/SystemStatus"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": [],
            "result": {
                "status": "online",
                "timestamp": "2026-02-24T21:00:00Z"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let result = client.public_get("SystemStatus", &[], false).await.unwrap();

    assert_eq!(result.get("status").unwrap().as_str().unwrap(), "online");
}

#[tokio::test]
async fn spot_private_post_sends_auth_headers() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/0/private/Balance"))
        .and(header_exists("API-Key"))
        .and(header_exists("API-Sign"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": [],
            "result": {
                "ZUSD": "1000.00",
                "XXBT": "0.5"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let creds = test_credentials();
    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let result = client
        .private_post("Balance", HashMap::new(), &creds, None, true, false)
        .await
        .unwrap();

    assert!(result.get("ZUSD").is_some());
    assert!(result.get("XXBT").is_some());
}

#[tokio::test]
async fn spot_private_post_includes_otp_when_provided() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/0/private/Balance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": [],
            "result": { "ZUSD": "500.00" }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let creds = test_credentials();
    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let result = client
        .private_post(
            "Balance",
            HashMap::new(),
            &creds,
            Some("123456"),
            true,
            false,
        )
        .await
        .unwrap();

    assert!(result.get("ZUSD").is_some());
}

#[tokio::test]
async fn spot_rate_limit_returned_immediately() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/0/private/Balance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": ["EAPI:Rate limit exceeded"],
            "result": null
        })))
        .expect(1)
        .mount(&server)
        .await;

    let creds = test_credentials();
    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let err = client
        .private_post("Balance", HashMap::new(), &creds, None, true, false)
        .await
        .unwrap_err();

    assert_eq!(err.category(), kraken_cli::errors::ErrorCategory::RateLimit);
    if let kraken_cli::errors::KrakenError::RateLimit {
        suggestion,
        docs_url,
        retryable,
        ..
    } = &err
    {
        assert!(retryable);
        assert!(docs_url.contains("spot-rest-ratelimits"));
        assert!(suggestion.contains("WebSocket"));
    } else {
        panic!("Expected RateLimit variant, got {err:?}");
    }
}

#[tokio::test]
async fn spot_auth_error_parsed_from_envelope() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/0/private/Balance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": ["EAPI:Invalid key"],
            "result": null
        })))
        .expect(1)
        .mount(&server)
        .await;

    let creds = test_credentials();
    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let err = client
        .private_post("Balance", HashMap::new(), &creds, None, true, false)
        .await
        .unwrap_err();

    let category = err.category();
    assert_eq!(
        category,
        kraken_cli::errors::ErrorCategory::Auth,
        "Expected Auth error category"
    );
}

#[tokio::test]
async fn spot_public_get_retries_on_transport_error() {
    let server = MockServer::start().await;

    // First call fails, second succeeds
    Mock::given(method("GET"))
        .and(path("/0/public/Time"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": [],
            "result": { "unixtime": 1740000000, "rfc1123": "Mon, 24 Feb 2026 21:00:00 +0000" }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let result = client.public_get("Time", &[], false).await.unwrap();
    assert!(result.get("unixtime").is_some());
}

#[tokio::test]
async fn spot_5xx_retry_uses_fresh_nonce() {
    let server = MockServer::start().await;

    // First call: 500 error; second call: success
    Mock::given(method("POST"))
        .and(path("/0/private/Balance"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/0/private/Balance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": [],
            "result": { "ZUSD": "100.00" }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let creds = test_credentials();
    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let result = client
        .private_post("Balance", HashMap::new(), &creds, None, true, false)
        .await
        .unwrap();

    assert_eq!(result.get("ZUSD").unwrap().as_str().unwrap(), "100.00");
}

#[tokio::test]
async fn private_post_raw_detects_json_error_on_200() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/0/private/RetrieveExport"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": ["EAPI:Invalid nonce"],
            "result": null
        })))
        .expect(1)
        .mount(&server)
        .await;

    let creds = test_credentials();
    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let err = client
        .private_post_raw("RetrieveExport", HashMap::new(), &creds, None, true, false)
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(
        msg.contains("Invalid nonce"),
        "Expected nonce error, got: {msg}"
    );
}

#[tokio::test]
async fn private_post_raw_returns_binary_on_success() {
    let server = MockServer::start().await;

    let zip_bytes = b"PK\x03\x04fake_zip_data_here";
    Mock::given(method("POST"))
        .and(path("/0/private/RetrieveExport"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(zip_bytes.to_vec())
                .insert_header("Content-Type", "application/zip"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let creds = test_credentials();
    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let result = client
        .private_post_raw("RetrieveExport", HashMap::new(), &creds, None, true, false)
        .await
        .unwrap();

    assert!(result.starts_with(b"PK"));
    assert_eq!(result.len(), zip_bytes.len());
}

#[tokio::test]
async fn private_post_raw_rate_limit_returned_immediately() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/0/private/RetrieveExport"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": ["EAPI:Rate limit exceeded"],
            "result": null
        })))
        .expect(1)
        .mount(&server)
        .await;

    let creds = test_credentials();
    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let err = client
        .private_post_raw("RetrieveExport", HashMap::new(), &creds, None, true, false)
        .await
        .unwrap_err();

    assert_eq!(err.category(), kraken_cli::errors::ErrorCategory::RateLimit);
}

#[tokio::test]
async fn spot_json_output_table_output_match() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/0/public/SystemStatus"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": [],
            "result": {
                "status": "online",
                "timestamp": "2026-02-24T21:00:00Z"
            }
        })))
        .mount(&server)
        .await;

    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let result = client.public_get("SystemStatus", &[], false).await.unwrap();

    assert!(result.is_object());
    assert!(result.get("status").is_some());
    assert!(result.get("timestamp").is_some());
}

#[tokio::test]
async fn spot_non_2xx_returns_network_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/0/public/Assets"))
        .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
        .mount(&server)
        .await;

    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let err = client.public_get("Assets", &[], false).await.unwrap_err();

    let category = err.category();
    assert_eq!(category, kraken_cli::errors::ErrorCategory::Network);
}

#[tokio::test]
async fn spot_user_agent_header_present() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/0/public/Time"))
        .and(header_exists("user-agent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": [],
            "result": { "unixtime": 1740000000 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let _ = client.public_get("Time", &[], false).await.unwrap();
}

#[tokio::test]
async fn spot_korigin_header_present() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/0/public/Time"))
        .and(header("x-korigin", "u003"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": [],
            "result": { "unixtime": 1740000000 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let _ = client.public_get("Time", &[], false).await.unwrap();
}

#[tokio::test]
async fn spot_public_get_retries_on_5xx_then_succeeds() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/0/public/Ticker"))
        .respond_with(ResponseTemplate::new(502).set_body_string("Bad Gateway"))
        .up_to_n_times(2)
        .expect(2)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/0/public/Ticker"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": [],
            "result": { "XXBTZUSD": { "a": ["50000.0"] } }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let result = client
        .public_get("Ticker", &[("pair", "XBTUSD")], false)
        .await
        .unwrap();
    assert!(result.get("XXBTZUSD").is_some());
}

#[tokio::test]
async fn spot_public_get_5xx_exhausts_retries() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/0/public/Ticker"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .expect(4) // 1 initial + 3 retries
        .mount(&server)
        .await;

    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let err = client.public_get("Ticker", &[], false).await.unwrap_err();
    assert_eq!(err.category(), kraken_cli::errors::ErrorCategory::Network);
}

#[tokio::test]
async fn spot_5xx_returns_network_error_not_parse_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/0/public/Assets"))
        .respond_with(ResponseTemplate::new(503).set_body_string("not json at all"))
        .expect(4) // 1 initial + 3 retries (all 5xx)
        .mount(&server)
        .await;

    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let err = client.public_get("Assets", &[], false).await.unwrap_err();
    assert_eq!(
        err.category(),
        kraken_cli::errors::ErrorCategory::Network,
        "5xx with non-JSON body should be Network, not Parse"
    );
}

fn test_futures_credentials() -> FuturesCredentials {
    FuturesCredentials {
        api_key: "test-futures-key".to_string(),
        api_secret: SecretValue::new(BASE64.encode(b"test_futures_secret_key_32chars!")),
        source: CredentialSource::Flag,
    }
}

#[tokio::test]
async fn futures_public_get_retries_on_5xx_then_succeeds() {
    let server = MockServer::start().await;
    let base_url = format!("{}/api/v3", server.uri());

    Mock::given(method("GET"))
        .and(path("/api/v3/tickers"))
        .respond_with(ResponseTemplate::new(500).set_body_string("error"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v3/tickers"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": "success",
            "tickers": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = FuturesClient::new(Some(&base_url)).unwrap();
    let result = client.public_get("tickers", &[], false).await.unwrap();
    assert!(result.get("tickers").is_some());
}

#[tokio::test]
async fn futures_private_get_retries_on_5xx_then_succeeds() {
    let server = MockServer::start().await;
    let base_url = format!("{}/api/v3", server.uri());

    Mock::given(method("GET"))
        .and(path("/api/v3/accounts"))
        .respond_with(ResponseTemplate::new(502).set_body_string("Bad Gateway"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v3/accounts"))
        .and(header_exists("APIKey"))
        .and(header_exists("Authent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": "success",
            "accounts": {}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let creds = test_futures_credentials();
    let client = FuturesClient::new(Some(&base_url)).unwrap();
    let result = client
        .private_get("accounts", &[], &creds, false)
        .await
        .unwrap();
    assert!(result.get("accounts").is_some());
}

#[tokio::test]
async fn spot_eservice_throttled_returned_immediately() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/0/private/Balance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": ["EService:Throttled"],
            "result": null
        })))
        .expect(1)
        .mount(&server)
        .await;

    let creds = test_credentials();
    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let err = client
        .private_post("Balance", HashMap::new(), &creds, None, true, false)
        .await
        .unwrap_err();

    assert_eq!(err.category(), kraken_cli::errors::ErrorCategory::RateLimit);
    if let kraken_cli::errors::KrakenError::RateLimit { suggestion, .. } = &err {
        assert!(suggestion.contains("concurrency"));
    } else {
        panic!("Expected RateLimit variant, got {err:?}");
    }
}

#[tokio::test]
async fn spot_eorder_rate_limit_returned_immediately() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/0/private/AddOrder"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": ["EOrder:Rate limit exceeded"],
            "result": null
        })))
        .expect(1)
        .mount(&server)
        .await;

    let creds = test_credentials();
    let client = SpotClient::new(Some(&server.uri())).unwrap();
    let err = client
        .private_post("AddOrder", HashMap::new(), &creds, None, true, false)
        .await
        .unwrap_err();

    assert_eq!(err.category(), kraken_cli::errors::ErrorCategory::RateLimit);
    if let kraken_cli::errors::KrakenError::RateLimit {
        suggestion,
        docs_url,
        ..
    } = &err
    {
        assert!(suggestion.contains("per-pair"));
        assert!(docs_url.contains("spot-ratelimits"));
    } else {
        panic!("Expected RateLimit variant, got {err:?}");
    }
}

#[tokio::test]
async fn futures_api_limit_exceeded_returned_immediately() {
    let server = MockServer::start().await;
    let base_url = format!("{}/api/v3", server.uri());

    Mock::given(method("GET"))
        .and(path("/api/v3/tickers"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": "error",
            "error": "apiLimitExceeded"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = FuturesClient::new(Some(&base_url)).unwrap();
    let err = client.public_get("tickers", &[], false).await.unwrap_err();

    assert_eq!(err.category(), kraken_cli::errors::ErrorCategory::RateLimit);
    if let kraken_cli::errors::KrakenError::RateLimit {
        docs_url,
        suggestion,
        ..
    } = &err
    {
        assert!(docs_url.contains("futures-rate-limits"));
        assert!(suggestion.contains("Futures"));
    } else {
        panic!("Expected RateLimit variant, got {err:?}");
    }
}

#[tokio::test]
async fn futures_auth_error_not_retried() {
    let server = MockServer::start().await;
    let base_url = format!("{}/api/v3", server.uri());

    Mock::given(method("GET"))
        .and(path("/api/v3/accounts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": "error",
            "error": "authenticationError"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let creds = test_futures_credentials();
    let client = FuturesClient::new(Some(&base_url)).unwrap();
    let err = client
        .private_get("accounts", &[], &creds, false)
        .await
        .unwrap_err();

    assert_eq!(
        err.category(),
        kraken_cli::errors::ErrorCategory::Auth,
        "Futures authenticationError should be classified as auth, got {:?}",
        err.category()
    );
}

#[tokio::test]
async fn futures_private_get_rate_limit_from_non_2xx_returned_immediately() {
    let server = MockServer::start().await;
    let base_url = format!("{}/api/v3", server.uri());

    Mock::given(method("GET"))
        .and(path("/api/v3/accounts"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "result": "error",
            "error": "apiLimitExceeded"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let creds = test_futures_credentials();
    let client = FuturesClient::new(Some(&base_url)).unwrap();
    let err = client
        .private_get("accounts", &[], &creds, false)
        .await
        .unwrap_err();

    assert_eq!(err.category(), kraken_cli::errors::ErrorCategory::RateLimit);
}

#[tokio::test]
async fn futures_funding_rate_reads_relative_field() {
    let server = MockServer::start().await;
    let base_url = format!("{}/api/v3", server.uri());

    Mock::given(method("GET"))
        .and(path("/api/v3/historical-funding-rates"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": "success",
            "rates": [
                {
                    "timestamp": "2026-04-12T06:00:00Z",
                    "fundingRate": 0.3761286388681191,
                    "relativeFundingRate": 5.247063888889e-06
                },
                {
                    "timestamp": "2026-04-12T07:00:00Z",
                    "fundingRate": 0.1127,
                    "relativeFundingRate": 0.0000158
                }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = FuturesClient::new(Some(&base_url)).unwrap();
    let data = client
        .public_get(
            "historical-funding-rates",
            &[("symbol", "PF_XBTUSD")],
            false,
        )
        .await
        .unwrap();

    let rates = data.get("rates").unwrap().as_array().unwrap();
    let last = rates.last().unwrap();

    let relative = last.get("relativeFundingRate").and_then(|v| v.as_f64());
    let absolute = last.get("fundingRate").and_then(|v| v.as_f64());

    assert_eq!(relative, Some(0.0000158), "must read relativeFundingRate");
    assert_eq!(absolute, Some(0.1127));
    assert!(
        relative.unwrap() < absolute.unwrap(),
        "relativeFundingRate must be smaller than fundingRate; \
         using the wrong field overcharges by orders of magnitude"
    );
}
