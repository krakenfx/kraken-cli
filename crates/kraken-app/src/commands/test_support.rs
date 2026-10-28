//! Shared fixtures for in-process command tests.

use crate::{cli::AppContext, venue::Venue};
use base64::Engine as _;
use kraken_auth::{CredentialSource, Credentials};
use kraken_rest::{ClientOptions, Transport, futures::FuturesRest, spot::SpotRest};
use serde_json::{Value, json};

/// A complete wire `Ticker` row quoting `ask`/`bid`; the rest is filler.
pub(crate) fn ticker_row(ask: &str, bid: &str) -> Value {
    json!({
        "a": [ask, "1", "1.000"], "b": [bid, "1", "1.000"], "c": [ask, "0.1"],
        "v": ["1", "1"], "p": [ask, ask], "t": [1, 1], "l": [bid, bid], "h": [ask, ask],
        "o": ask
    })
}

/// A complete wire `QueryOrders` row: `fields` over an open, unfilled sell.
pub(crate) fn order_row(fields: Value) -> Value {
    let mut row = json!({
        "descr": {
            "pair": "XBTUSD", "type": "sell", "ordertype": "limit", "price": "64000",
            "price2": "0", "leverage": "none", "order": "sell 0.25 XBTUSD @ limit 64000",
            "close": ""
        },
        "status": "open", "opentm": 1754550000.0, "expiretm": 0,
        "vol": "0.25", "vol_exec": "0", "cost": "0", "fee": "0", "price": "0",
        "misc": "", "oflags": ""
    });
    if let Some(fields) = fields.as_object() {
        for (key, value) in fields {
            row[key] = value.clone();
        }
    }
    row
}

/// A context with no URL overrides, flags or prompts beyond its arguments;
/// both venues share `api_url` and `credentials`.
pub(crate) fn offline_ctx(api_url: Option<String>, credentials: Option<Credentials>) -> AppContext {
    let transport = Transport::new(&ClientOptions::default()).expect("client builds offline");
    AppContext {
        format: crate::output::OutputFormat::Json,
        ws_public_url: None,
        ws_auth_url: None,
        ws_l3_url: None,
        ws_futures_url: None,
        oauth: Default::default(),
        ws_reconnect_base_ms: None,
        api_secret: None,
        workspace: None,
        force: false,
        accept_invalid_certs: false,
        allow_any_url_host: false,
        mcp_mode: false,
        monitor: None,
        spot: Venue::new(
            SpotRest::new(transport.clone(), api_url.clone()),
            credentials.clone(),
        ),
        futures: Venue::new(FuturesRest::new(transport, api_url), credentials),
        otp: None,
        spot_env_key: false,
    }
}

/// The test key pair, as `--api-key`/`--api-secret` would resolve it.
pub(crate) fn test_credentials(api_key: &str) -> Credentials {
    Credentials {
        api_key: secrecy::SecretString::from(api_key.to_string()),
        api_secret: secrecy::SecretString::from(
            base64::engine::general_purpose::STANDARD.encode(b"test_secret_key_bytes_32_chars!!"),
        ),
        source: CredentialSource::Flag,
    }
}

/// A context whose Spot and Futures engines both point at a mock venue, with
/// credentials for each. In-process because the binary refuses plain-http URLs.
pub(crate) fn venue_ctx(api_url: String, api_key: &str) -> AppContext {
    AppContext {
        force: true,
        allow_any_url_host: true,
        ..offline_ctx(Some(api_url), Some(test_credentials(api_key)))
    }
}
