//! Futures WebSocket streaming commands.
//!
//! Futures speaks WS v1, not the spot v2 protocol, so it cannot ride the
//! `kraken-ws` stack: private feeds authenticate by signing a server-issued
//! challenge (not a pre-minted token), liveness is a client-side ping, and —
//! unlike the spot stream's uncapped backoff — reconnects are bounded by
//! attempt and per-window safety budgets.

use super::Execute;
use crate::{
    cli::AppContext,
    errors::{KrakenError, Result},
    output::{self, CommandOutput},
};
use futures_util::{SinkExt, StreamExt};
use kraken_contract::futures_ws::FuturesWsCommand;
use kraken_rest::Auth;
use serde_json::Value;
use std::{
    collections::VecDeque,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{net::TcpStream, time::Instant};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async, connect_async_tls_with_config,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        http::{HeaderValue, Request},
    },
};

/// Extract the host portion of a URL for safe logging (avoids leaking full
/// paths/query).
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

pub(crate) fn requires_auth(cmd: &FuturesWsCommand) -> bool {
    matches!(
        cmd,
        FuturesWsCommand::Fills(_)
            | FuturesWsCommand::OpenOrders(_)
            | FuturesWsCommand::OpenPositions(_)
            | FuturesWsCommand::Balances(_)
            | FuturesWsCommand::Notifications(_)
            | FuturesWsCommand::AccountLog(_)
    )
}

fn command_to_feed(cmd: &FuturesWsCommand) -> &'static str {
    match cmd {
        FuturesWsCommand::Ticker(_) => "ticker",
        FuturesWsCommand::Trades(_) => "trade",
        FuturesWsCommand::Book(_) => "book",
        FuturesWsCommand::Fills(_) => "fills",
        FuturesWsCommand::OpenOrders(_) => "open_orders",
        FuturesWsCommand::OpenPositions(_) => "open_positions",
        FuturesWsCommand::Balances(_) => "account_balances_and_margins",
        FuturesWsCommand::Notifications(_) => "notifications_auth",
        FuturesWsCommand::AccountLog(_) => "account_log",
    }
}

fn markets(cmd: &FuturesWsCommand) -> Option<&[String]> {
    match cmd {
        FuturesWsCommand::Ticker(feed) => Some(&feed.markets),
        FuturesWsCommand::Trades(feed) => Some(&feed.markets),
        FuturesWsCommand::Book(feed) => Some(&feed.markets),
        _ => None,
    }
}

/// Stream a futures WebSocket feed until the connection ends. This is an
/// interactive/streaming command handled by the dispatcher, not the shared
/// request/response executor — hence `Result<()>` rather than `CommandOutput`.
pub(crate) async fn execute(
    cmd: &FuturesWsCommand,
    auth: Option<&Auth>,
    ws_futures_url: Option<&str>,
    accept_invalid_certs: bool,
) -> Result<()> {
    // URL precedence (flag > env) is resolved by clap upstream; the default is
    // applied here. The scheme is validated regardless of source.
    let url = ws_futures_url.unwrap_or(DEFAULT_FUTURES_WS_URL).to_string();
    crate::url_policy::validate_url_scheme(&url)?;

    let feed = command_to_feed(cmd);

    if requires_auth(cmd) {
        let auth = auth.ok_or_else(|| {
            KrakenError::Auth("Futures credentials required for private WebSocket feeds.".into())
        })?;
        stream_private(feed, auth, &url, accept_invalid_certs).await?;
    } else {
        let markets = markets(cmd).unwrap_or(&[]);
        stream_public(feed, markets, &url, accept_invalid_certs).await?;
    }

    Ok(())
}

impl Execute for FuturesWsCommand {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        // Only the private feeds sign in; a public feed runs without credentials.
        let auth = if requires_auth(&self) {
            Some(ctx.futures_account()?.auth().as_ref())
        } else {
            None
        };
        // Qualified: this module has a free `execute` too, and the method
        // sharing its name would otherwise read as a recursive call.
        self::execute(
            &self,
            auth,
            ctx.ws_futures_url.as_deref(),
            ctx.accept_invalid_certs,
        )
        .await?;
        Ok(CommandOutput::inline())
    }

    fn renders_inline(&self) -> bool {
        true
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

async fn stream_private(feed: &str, auth: &Auth, url: &str, skip_verify: bool) -> Result<()> {
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
            "api_key": auth.key(),
        });
        let challenge_msg = serde_json::to_string(&challenge_request)
            .map_err(|e| KrakenError::Parse(e.to_string()))?;

        tracing::debug!(feed, "requesting challenge for auth");

        ws.send(Message::Text(challenge_msg.into()))
            .await
            .map_err(|e| KrakenError::websocket(format!("Send challenge failed: {e}")))?;

        let challenge = receive_challenge(&mut ws).await?;

        let signed = kraken_rest::futures::sign_challenge(&challenge, auth)?;

        let subscribe = serde_json::json!({
            "event": "subscribe",
            "feed": feed,
            "api_key": auth.key(),
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
    use super::*;
    use kraken_contract::futures_ws;

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
    fn command_to_feed_mapping() {
        assert_eq!(
            command_to_feed(&FuturesWsCommand::Fills(futures_ws::Fills {})),
            "fills"
        );
        assert_eq!(
            command_to_feed(&FuturesWsCommand::OpenOrders(futures_ws::OpenOrders {})),
            "open_orders"
        );
        assert_eq!(
            command_to_feed(&FuturesWsCommand::OpenPositions(
                futures_ws::OpenPositions {}
            )),
            "open_positions"
        );
        assert_eq!(
            command_to_feed(&FuturesWsCommand::Balances(futures_ws::Balances {})),
            "account_balances_and_margins"
        );
        assert_eq!(
            command_to_feed(&FuturesWsCommand::Notifications(
                futures_ws::Notifications {}
            )),
            "notifications_auth"
        );
        assert_eq!(
            command_to_feed(&FuturesWsCommand::AccountLog(futures_ws::AccountLog {})),
            "account_log"
        );
    }

    #[test]
    fn requires_auth_public_feeds() {
        assert!(!requires_auth(&FuturesWsCommand::Ticker(
            futures_ws::Ticker { markets: vec![] }
        )));
        assert!(!requires_auth(&FuturesWsCommand::Trades(
            futures_ws::Trades { markets: vec![] }
        )));
        assert!(!requires_auth(&FuturesWsCommand::Book(futures_ws::Book {
            markets: vec![]
        })));
    }

    #[test]
    fn requires_auth_private_feeds() {
        assert!(requires_auth(&FuturesWsCommand::Fills(
            futures_ws::Fills {}
        )));
        assert!(requires_auth(&FuturesWsCommand::OpenOrders(
            futures_ws::OpenOrders {}
        )));
        assert!(requires_auth(&FuturesWsCommand::OpenPositions(
            futures_ws::OpenPositions {}
        )));
        assert!(requires_auth(&FuturesWsCommand::Balances(
            futures_ws::Balances {}
        )));
        assert!(requires_auth(&FuturesWsCommand::Notifications(
            futures_ws::Notifications {}
        )));
        assert!(requires_auth(&FuturesWsCommand::AccountLog(
            futures_ws::AccountLog {}
        )));
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
