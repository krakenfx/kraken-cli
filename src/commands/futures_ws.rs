//! Futures WebSocket streaming commands.
//!
//! Futures speaks WS v1, not the spot v2 protocol, so it cannot ride the
//! `kraken-ws` stack: private feeds authenticate by signing a server-issued
//! challenge (not a pre-minted token), liveness is a client-side ping, and —
//! unlike the spot stream's uncapped backoff — reconnects are bounded by
//! attempt and per-window safety budgets.

use std::collections::VecDeque;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::Subcommand;
use futures_util::{SinkExt, StreamExt};
use secrecy::ExposeSecret;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, Request};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async, connect_async_tls_with_config,
};

use super::Execute;
use super::streaming::streaming_unsupported;
use crate::cli::AppContext;
use crate::config::Credentials;
use crate::errors::{KrakenError, Result};
use crate::output::{self, CommandOutput};

/// Extract the host portion of a URL for safe logging (avoids leaking full paths/query).
fn url_host(url: &str) -> &str {
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    without_scheme
        .split(['/', '?'])
        .next()
        .unwrap_or(without_scheme)
}

const DEFAULT_FUTURES_WS_URL: &str = "wss://futures.kraken.com/ws/v1";
const MAX_RECONNECTS: u32 = 12;
const RECONNECT_BASE_MS: u64 = 1000;
const FAST_RECONNECT_ATTEMPTS: u32 = 2;
const RECONNECT_MIN_DELAY_MS: u64 = 5000;
const RECONNECT_MAX_DELAY_MS: u64 = 30000;
const RECONNECT_JITTER_MAX_MS: u64 = 750;
const RECONNECT_WINDOW_SECS: u64 = 600;
const MAX_RECONNECTS_PER_WINDOW: usize = 120;
const STABLE_SESSION_SECS: u64 = 30;
const PING_INTERVAL_SECS: u64 = 45;

#[derive(Debug, Subcommand)]
pub(crate) enum FuturesWsCommand {
    /// Stream futures ticker updates (public).
    Ticker {
        /// Market symbols (e.g. PF_XBTUSD).
        #[arg(required = true, num_args = 1..)]
        markets: Vec<String>,
    },
    /// Stream futures trades (public).
    #[command(alias = "trade")]
    Trades {
        /// Market symbols.
        #[arg(required = true, num_args = 1..)]
        markets: Vec<String>,
    },
    /// Stream futures order book updates (public).
    Book {
        /// Market symbols.
        #[arg(required = true, num_args = 1..)]
        markets: Vec<String>,
    },
    /// Stream fills (private, auth required).
    Fills,
    /// Stream open orders (private, auth required).
    OpenOrders,
    /// Stream open positions (private, auth required).
    OpenPositions,
    /// Stream account balances and margins (private, auth required).
    Balances,
    /// Stream notifications (private, auth required).
    Notifications,
    /// Stream account log (private, auth required).
    AccountLog,
}

pub(crate) fn requires_auth(cmd: &FuturesWsCommand) -> bool {
    matches!(
        cmd,
        FuturesWsCommand::Fills
            | FuturesWsCommand::OpenOrders
            | FuturesWsCommand::OpenPositions
            | FuturesWsCommand::Balances
            | FuturesWsCommand::Notifications
            | FuturesWsCommand::AccountLog
    )
}

fn command_to_feed(cmd: &FuturesWsCommand) -> &'static str {
    match cmd {
        FuturesWsCommand::Ticker { .. } => "ticker",
        FuturesWsCommand::Trades { .. } => "trade",
        FuturesWsCommand::Book { .. } => "book",
        FuturesWsCommand::Fills => "fills",
        FuturesWsCommand::OpenOrders => "open_orders",
        FuturesWsCommand::OpenPositions => "open_positions",
        FuturesWsCommand::Balances => "account_balances_and_margins",
        FuturesWsCommand::Notifications => "notifications_auth",
        FuturesWsCommand::AccountLog => "account_log",
    }
}

fn markets(cmd: &FuturesWsCommand) -> Option<&[String]> {
    match cmd {
        FuturesWsCommand::Ticker { markets } => Some(markets),
        FuturesWsCommand::Trades { markets } => Some(markets),
        FuturesWsCommand::Book { markets } => Some(markets),
        _ => None,
    }
}

/// Stream a futures WebSocket feed until the connection ends. This is an
/// interactive/streaming command handled by the dispatcher, not the shared
/// request/response executor — hence `Result<()>` rather than `CommandOutput`.
pub(crate) async fn execute(
    cmd: &FuturesWsCommand,
    creds: Option<&Credentials>,
    ws_futures_url: Option<&str>,
    accept_invalid_certs: bool,
) -> Result<()> {
    // URL precedence (flag > env) is resolved by clap upstream; the default is
    // applied here. The scheme is validated regardless of source.
    let url = ws_futures_url.unwrap_or(DEFAULT_FUTURES_WS_URL).to_string();
    crate::client::validate_url_scheme(&url)?;

    let feed = command_to_feed(cmd);

    if requires_auth(cmd) {
        let creds = creds.ok_or_else(|| {
            KrakenError::Auth("Futures credentials required for private WebSocket feeds.".into())
        })?;
        stream_private(feed, creds, &url, accept_invalid_certs).await?;
    } else {
        let markets = markets(cmd).unwrap_or(&[]);
        stream_public(feed, markets, &url, accept_invalid_certs).await?;
    }

    Ok(())
}

impl Execute for FuturesWsCommand {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        // `futures ws` streams; `dispatch` intercepts it, so only the
        // render-free executor (the MCP path) can reach this rejection.
        Err(streaming_unsupported())
    }
}

fn build_public_subscribe(feed: &str, markets: &[String]) -> Value {
    let mut msg = serde_json::json!({
        "event": "subscribe",
        "feed": feed,
    });
    if !markets.is_empty() {
        msg["product_ids"] = serde_json::json!(markets);
    }
    msg
}

fn ws_handshake_request(url: &str) -> Result<Request<()>> {
    let mut request = url.into_client_request()?;
    let headers = request.headers_mut();
    for (name, value) in crate::telemetry::ws_handshake_headers() {
        headers.insert(name, HeaderValue::from_str(value)?);
    }
    Ok(request)
}

type WsConnection = (
    WebSocketStream<MaybeTlsStream<TcpStream>>,
    tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
);

async fn ws_connect(url: &str, skip_verify: bool) -> Result<WsConnection> {
    let request = ws_handshake_request(url)?;
    let connect = if skip_verify {
        let connector = kraken_ws::dangerous_tls_connector();
        connect_async_tls_with_config(request, None, false, Some(connector)).await
    } else {
        connect_async(request).await
    };
    Ok(connect?)
}

fn reconnect_jitter_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.subsec_nanos() as u64) % (RECONNECT_JITTER_MAX_MS + 1))
        .unwrap_or(0)
}

fn reconnect_backoff_ms(reconnect_count: u32) -> u64 {
    let exponent = reconnect_count.saturating_sub(1);
    let exp_backoff = RECONNECT_BASE_MS.saturating_mul(2u64.pow(exponent));
    let base = if reconnect_count <= FAST_RECONNECT_ATTEMPTS {
        exp_backoff.min(RECONNECT_MIN_DELAY_MS)
    } else {
        exp_backoff.clamp(RECONNECT_MIN_DELAY_MS, RECONNECT_MAX_DELAY_MS)
    };
    base.saturating_add(reconnect_jitter_ms())
}

fn prune_reconnect_window(reconnect_history: &mut VecDeque<Instant>) {
    let window = Duration::from_secs(RECONNECT_WINDOW_SECS);
    while reconnect_history
        .front()
        .is_some_and(|ts| ts.elapsed() >= window)
    {
        reconnect_history.pop_front();
    }
}

async fn enforce_reconnect_budget(reconnect_history: &mut VecDeque<Instant>, label: &str) {
    prune_reconnect_window(reconnect_history);
    if reconnect_history.len() >= MAX_RECONNECTS_PER_WINDOW {
        if let Some(oldest) = reconnect_history.front() {
            let remaining =
                Duration::from_secs(RECONNECT_WINDOW_SECS).saturating_sub(oldest.elapsed());
            let wait_for = remaining.saturating_add(Duration::from_millis(reconnect_jitter_ms()));
            tracing::warn!(
                label,
                max = MAX_RECONNECTS_PER_WINDOW,
                window_secs = RECONNECT_WINDOW_SECS,
                wait_ms = wait_for.as_millis() as u64,
                "reconnect safety budget reached"
            );
            tokio::time::sleep(wait_for).await;
        }
        prune_reconnect_window(reconnect_history);
    }
    reconnect_history.push_back(Instant::now());
}

async fn stream_public(feed: &str, markets: &[String], url: &str, skip_verify: bool) -> Result<()> {
    let mut reconnect_count = 0u32;
    let mut reconnect_history = VecDeque::new();

    loop {
        if reconnect_count > 0 {
            tracing::debug!(
                feed,
                host = url_host(url),
                reconnect_count,
                max = MAX_RECONNECTS,
                "reconnecting"
            );
        } else {
            tracing::info!(feed, host = url_host(url), "connecting");
        }

        let ws_result = ws_connect(url, skip_verify).await;
        let (mut ws, _) = match ws_result {
            Ok(conn) => conn,
            Err(e) => {
                if reconnect_count >= MAX_RECONNECTS {
                    return Err(KrakenError::websocket(format!(
                        "Connection failed after {MAX_RECONNECTS} reconnect attempts: {e}"
                    )));
                }
                reconnect_count += 1;
                enforce_reconnect_budget(&mut reconnect_history, "futures-ws-public").await;
                let backoff = reconnect_backoff_ms(reconnect_count);
                tracing::warn!(
                    feed,
                    reconnect_count,
                    backoff_ms = backoff,
                    error = %e,
                    "connection failed, retrying"
                );
                tokio::time::sleep(Duration::from_millis(backoff)).await;
                continue;
            }
        };

        let connected_at = Instant::now();

        let subscribe = build_public_subscribe(feed, markets);
        let msg =
            serde_json::to_string(&subscribe).map_err(|e| KrakenError::Parse(e.to_string()))?;

        tracing::info!(feed, "subscribing");

        ws.send(Message::Text(msg.into()))
            .await
            .map_err(|e| KrakenError::websocket(format!("Send failed: {e}")))?;

        let shutdown = tokio::signal::ctrl_c();
        tokio::pin!(shutdown);
        let mut ping_interval = tokio::time::interval(Duration::from_secs(PING_INTERVAL_SECS));
        ping_interval.tick().await;

        let disconnected = loop {
            tokio::select! {
                msg = ws.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Some(err_msg) = handle_futures_ws_message(&text) {
                                let _ = ws.close(None).await;
                                return Err(KrakenError::websocket(format!(
                                    "Futures WS error: {err_msg}"
                                )));
                            }
                        }
                        Some(Ok(Message::Close(_))) => {
                            tracing::debug!(feed, "closed by server");
                            break true;
                        }
                        Some(Err(e)) => {
                            tracing::debug!(feed, error = %e, "stream error");
                            break true;
                        }
                        None => break true,
                        _ => {}
                    }
                }
                _ = ping_interval.tick() => {
                    let ping = serde_json::json!({"event": "ping"});
                    let payload = serde_json::to_string(&ping).unwrap_or_default();
                    if ws.send(Message::Text(payload.into())).await.is_err() {
                        break true;
                    }
                }
                _ = &mut shutdown => {
                    tracing::info!(feed, "shutting down connection");
                    let _ = ws.close(None).await;
                    return Ok(());
                }
            }
        };

        if disconnected {
            if connected_at.elapsed() >= Duration::from_secs(STABLE_SESSION_SECS) {
                reconnect_count = 0;
            }
            reconnect_count += 1;
            if reconnect_count > MAX_RECONNECTS {
                tracing::error!(
                    feed,
                    max = MAX_RECONNECTS,
                    "connection lost after max reconnects"
                );
                return Err(KrakenError::websocket(format!(
                    "Futures WS connection lost after {MAX_RECONNECTS} reconnect attempts"
                )));
            }
            enforce_reconnect_budget(&mut reconnect_history, "futures-ws-public").await;
            let backoff = reconnect_backoff_ms(reconnect_count);
            tracing::warn!(
                feed,
                reconnect_count,
                max = MAX_RECONNECTS,
                backoff_ms = backoff,
                "disconnected, reconnecting"
            );
            tokio::time::sleep(Duration::from_millis(backoff)).await;
        }
    }
}

async fn stream_private(
    feed: &str,
    creds: &Credentials,
    url: &str,
    skip_verify: bool,
) -> Result<()> {
    let mut reconnect_count = 0u32;
    let mut reconnect_history = VecDeque::new();

    loop {
        if reconnect_count > 0 {
            tracing::debug!(
                feed,
                host = url_host(url),
                reconnect_count,
                max = MAX_RECONNECTS,
                "reconnecting"
            );
        } else {
            tracing::info!(feed, host = url_host(url), "connecting");
        }

        let ws_result = ws_connect(url, skip_verify).await;
        let (mut ws, _) = match ws_result {
            Ok(conn) => conn,
            Err(e) => {
                if reconnect_count >= MAX_RECONNECTS {
                    return Err(KrakenError::websocket(format!(
                        "Connection failed after {MAX_RECONNECTS} reconnect attempts: {e}"
                    )));
                }
                reconnect_count += 1;
                enforce_reconnect_budget(&mut reconnect_history, "futures-ws-private").await;
                let backoff = reconnect_backoff_ms(reconnect_count);
                tracing::warn!(
                    feed,
                    reconnect_count,
                    backoff_ms = backoff,
                    error = %e,
                    "connection failed, retrying"
                );
                tokio::time::sleep(Duration::from_millis(backoff)).await;
                continue;
            }
        };

        let connected_at = Instant::now();

        let challenge_request = serde_json::json!({
            "event": "challenge",
            "api_key": creds.api_key.expose_secret(),
        });
        let challenge_msg = serde_json::to_string(&challenge_request)
            .map_err(|e| KrakenError::Parse(e.to_string()))?;

        tracing::debug!(feed, "requesting challenge for auth");

        ws.send(Message::Text(challenge_msg.into()))
            .await
            .map_err(|e| KrakenError::websocket(format!("Send challenge failed: {e}")))?;

        let challenge = receive_challenge(&mut ws).await?;

        let signed = sign_challenge(&challenge, &creds.api_secret)?;

        let subscribe = serde_json::json!({
            "event": "subscribe",
            "feed": feed,
            "api_key": creds.api_key.expose_secret(),
            "original_challenge": challenge,
            "signed_challenge": signed,
        });

        let sub_msg =
            serde_json::to_string(&subscribe).map_err(|e| KrakenError::Parse(e.to_string()))?;

        tracing::info!(feed, "subscribing (private, signed challenge)");

        ws.send(Message::Text(sub_msg.into()))
            .await
            .map_err(|e| KrakenError::websocket(format!("Send subscribe failed: {e}")))?;

        let shutdown = tokio::signal::ctrl_c();
        tokio::pin!(shutdown);
        let mut ping_interval = tokio::time::interval(Duration::from_secs(PING_INTERVAL_SECS));
        ping_interval.tick().await;

        let disconnected = loop {
            tokio::select! {
                msg = ws.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Some(err_msg) = handle_futures_ws_message(&text) {
                                let _ = ws.close(None).await;
                                return Err(KrakenError::websocket(format!(
                                    "Futures WS error: {err_msg}"
                                )));
                            }
                        }
                        Some(Ok(Message::Close(_))) => {
                            tracing::debug!(feed, "closed by server");
                            break true;
                        }
                        Some(Err(e)) => {
                            tracing::debug!(feed, error = %e, "stream error");
                            break true;
                        }
                        None => break true,
                        _ => {}
                    }
                }
                _ = ping_interval.tick() => {
                    let ping = serde_json::json!({"event": "ping"});
                    let payload = serde_json::to_string(&ping).unwrap_or_default();
                    if ws.send(Message::Text(payload.into())).await.is_err() {
                        break true;
                    }
                }
                _ = &mut shutdown => {
                    tracing::info!(feed, "shutting down connection");
                    let _ = ws.close(None).await;
                    return Ok(());
                }
            }
        };

        if disconnected {
            if connected_at.elapsed() >= Duration::from_secs(STABLE_SESSION_SECS) {
                reconnect_count = 0;
            }
            reconnect_count += 1;
            if reconnect_count > MAX_RECONNECTS {
                tracing::error!(
                    feed,
                    max = MAX_RECONNECTS,
                    "connection lost after max reconnects"
                );
                return Err(KrakenError::websocket(format!(
                    "Futures WS connection lost after {MAX_RECONNECTS} reconnect attempts"
                )));
            }
            enforce_reconnect_budget(&mut reconnect_history, "futures-ws-private").await;
            let backoff = reconnect_backoff_ms(reconnect_count);
            tracing::warn!(
                feed,
                reconnect_count,
                max = MAX_RECONNECTS,
                backoff_ms = backoff,
                "disconnected, reconnecting"
            );
            tokio::time::sleep(Duration::from_millis(backoff)).await;
        }
    }
}

async fn receive_challenge(ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>) -> Result<String> {
    let timeout = tokio::time::sleep(Duration::from_secs(10));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(val) = serde_json::from_str::<Value>(&text) {
                            let event = val.get("event").and_then(|e| e.as_str()).unwrap_or("");
                            if event == "challenge"
                                && let Some(message) = val.get("message").and_then(|m| m.as_str()) {
                                    tracing::debug!("challenge received");
                                    return Ok(message.to_string());
                                }
                            if event == "error" {
                                let err_msg = val.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
                                return Err(KrakenError::websocket(format!(
                                    "Challenge request failed: {err_msg}"
                                )));
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        return Err(KrakenError::websocket(
                            "Connection closed before challenge received",
                        ));
                    }
                    Some(Err(e)) => {
                        return Err(KrakenError::websocket(format!(
                            "Stream error waiting for challenge: {e}"
                        )));
                    }
                    _ => {}
                }
            }
            _ = &mut timeout => {
                return Err(KrakenError::websocket(
                    "Timed out waiting for challenge response",
                ));
            }
        }
    }
}

/// Sign a Futures WS challenge.
///
/// Algorithm: HMAC-SHA512(SHA256(challenge), base64_decode(api_secret))
/// Result is base64-encoded.
pub(crate) fn sign_challenge(
    challenge: &str,
    api_secret: &secrecy::SecretString,
) -> Result<String> {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::{Digest, Sha256, Sha512};

    let secret_bytes = BASE64
        .decode(api_secret.expose_secret())
        .map_err(|e| KrakenError::Auth(format!("Failed to decode Futures API secret: {e}")))?;

    let sha_digest = Sha256::digest(challenge.as_bytes());

    let mut mac = Hmac::<Sha512>::new_from_slice(&secret_bytes)
        .map_err(|e| KrakenError::Auth(format!("HMAC key error: {e}")))?;
    mac.update(&sha_digest);

    let result = mac.finalize().into_bytes();
    Ok(BASE64.encode(result))
}

/// Route one inbound frame: control events (pong/subscribed/challenge/info) are
/// swallowed, data frames render as JSONL on stdout, and a server `error` event
/// returns its message for the caller to fail the stream on.
fn handle_futures_ws_message(text: &str) -> Option<String> {
    if let Ok(val) = serde_json::from_str::<Value>(text) {
        let event = val.get("event").and_then(|e| e.as_str()).unwrap_or("");
        if event == "error" {
            let err_msg = val
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Some(err_msg.to_string());
        }
        if event == "pong" || event == "subscribed" || event == "challenge" || event == "info" {
            return None;
        }
        output::json::render_jsonl(&val);
    }
    None
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use secrecy::SecretString;

    use super::*;

    #[test]
    fn ws_handshake_includes_korigin() {
        let req = ws_handshake_request("wss://futures.kraken.com/ws/v1").unwrap();
        let value = req
            .headers()
            .get("x-korigin")
            .expect("x-korigin header missing");
        assert_eq!(value, "u004");
    }

    #[test]
    fn sign_challenge_deterministic() {
        let secret = SecretString::from(BASE64.encode(b"futures_ws_secret_key_32chars!!"));
        let challenge = "test-challenge-uuid-12345";
        let sig1 = sign_challenge(challenge, &secret).unwrap();
        let sig2 = sign_challenge(challenge, &secret).unwrap();
        assert_eq!(sig1, sig2);
        assert!(!sig1.is_empty());
    }

    #[test]
    fn sign_challenge_different_inputs_different_outputs() {
        let secret = SecretString::from(BASE64.encode(b"futures_ws_secret_key_32chars!!"));
        let sig1 = sign_challenge("challenge-a", &secret).unwrap();
        let sig2 = sign_challenge("challenge-b", &secret).unwrap();
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn sign_challenge_rejects_invalid_base64_secret() {
        let secret = SecretString::from("not-valid-base64!!!".to_string());
        assert!(sign_challenge("test", &secret).is_err());
    }

    #[test]
    fn command_to_feed_mapping() {
        assert_eq!(command_to_feed(&FuturesWsCommand::Fills), "fills");
        assert_eq!(
            command_to_feed(&FuturesWsCommand::OpenOrders),
            "open_orders"
        );
        assert_eq!(
            command_to_feed(&FuturesWsCommand::OpenPositions),
            "open_positions"
        );
        assert_eq!(
            command_to_feed(&FuturesWsCommand::Balances),
            "account_balances_and_margins"
        );
        assert_eq!(
            command_to_feed(&FuturesWsCommand::Notifications),
            "notifications_auth"
        );
        assert_eq!(
            command_to_feed(&FuturesWsCommand::AccountLog),
            "account_log"
        );
    }

    #[test]
    fn requires_auth_public_feeds() {
        assert!(!requires_auth(&FuturesWsCommand::Ticker {
            markets: vec![]
        }));
        assert!(!requires_auth(&FuturesWsCommand::Trades {
            markets: vec![]
        }));
        assert!(!requires_auth(&FuturesWsCommand::Book { markets: vec![] }));
    }

    #[test]
    fn requires_auth_private_feeds() {
        assert!(requires_auth(&FuturesWsCommand::Fills));
        assert!(requires_auth(&FuturesWsCommand::OpenOrders));
        assert!(requires_auth(&FuturesWsCommand::OpenPositions));
        assert!(requires_auth(&FuturesWsCommand::Balances));
        assert!(requires_auth(&FuturesWsCommand::Notifications));
        assert!(requires_auth(&FuturesWsCommand::AccountLog));
    }

    #[test]
    fn build_public_subscribe_with_products() {
        let msg = build_public_subscribe("ticker", &["PI_XBTUSD".to_string()]);
        assert_eq!(msg["event"], "subscribe");
        assert_eq!(msg["feed"], "ticker");
        assert_eq!(msg["product_ids"][0], "PI_XBTUSD");
    }

    #[test]
    fn build_public_subscribe_without_products() {
        let msg = build_public_subscribe("ticker", &[]);
        assert_eq!(msg["event"], "subscribe");
        assert_eq!(msg["feed"], "ticker");
        assert!(msg.get("product_ids").is_none());
    }
}
