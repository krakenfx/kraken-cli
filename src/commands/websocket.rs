/// WebSocket streaming commands for real-time market and private data.
///
/// Implements bounded reconnection with exponential backoff, reconnect safety
/// budgets, and signal-driven graceful shutdown. The reconnect counter is only reset
/// after a connection has been stable for `STABLE_SESSION_SECS`, preventing
/// unbounded reconnects on flapping connections.
use std::collections::{HashMap, VecDeque};
use std::env;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::Subcommand;
use futures_util::{SinkExt, StreamExt};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as RustlsError, SignatureScheme};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, Request};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{
    connect_async, connect_async_tls_with_config, Connector, MaybeTlsStream, WebSocketStream,
};

use crate::client::SpotClient;
use crate::config::SpotCredentials;
use crate::errors::{self, KrakenError, Result};
use crate::output::{self, OutputFormat};

const DEFAULT_WS_PUBLIC_URL: &str = "wss://ws.kraken.com/v2";
const DEFAULT_WS_AUTH_URL: &str = "wss://ws-auth.kraken.com/v2";
const DEFAULT_WS_L3_URL: &str = "wss://ws-l3.kraken.com/v2";
const MAX_RECONNECTS: u32 = 12;

pub(crate) struct WsUrls<'a> {
    pub(crate) public: Option<&'a str>,
    pub(crate) auth: Option<&'a str>,
    pub(crate) l3: Option<&'a str>,
}
const RECONNECT_BASE_MS: u64 = 1000;
const FAST_RECONNECT_ATTEMPTS: u32 = 2;
const RECONNECT_MIN_DELAY_MS: u64 = 5000;
const RECONNECT_MAX_DELAY_MS: u64 = 30000;
const RECONNECT_JITTER_MAX_MS: u64 = 750;
const RECONNECT_WINDOW_SECS: u64 = 600;
const MAX_RECONNECTS_PER_WINDOW: usize = 120;
/// Seconds a connection must stay up before the reconnect counter resets.
const STABLE_SESSION_SECS: u64 = 30;

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum WsCommand {
    /// Stream live ticker updates.
    Ticker {
        /// Trading pairs.
        #[arg(num_args = 1..)]
        pairs: Vec<String>,
        /// Event trigger: trades (default) or bbo.
        #[arg(long)]
        event_trigger: Option<String>,
        /// Request a snapshot on subscribe (default: true).
        #[arg(long)]
        snapshot: Option<bool>,
        /// Client request ID for correlation.
        #[arg(long)]
        req_id: Option<u64>,
    },
    /// Stream live trades.
    Trades {
        /// Trading pairs.
        #[arg(num_args = 1..)]
        pairs: Vec<String>,
        /// Request a snapshot on subscribe (default: true).
        #[arg(long)]
        snapshot: Option<bool>,
        /// Client request ID for correlation.
        #[arg(long)]
        req_id: Option<u64>,
    },
    /// Stream order book updates.
    Book {
        /// Trading pairs.
        #[arg(num_args = 1..)]
        pairs: Vec<String>,
        /// Book depth (10, 25, 100, 500, 1000).
        #[arg(long, default_value = "10")]
        depth: u32,
        /// Request a snapshot on subscribe (default: true).
        #[arg(long)]
        snapshot: Option<bool>,
        /// Client request ID for correlation.
        #[arg(long)]
        req_id: Option<u64>,
    },
    /// Stream OHLC candle updates.
    Ohlc {
        /// Trading pairs.
        #[arg(num_args = 1..)]
        pairs: Vec<String>,
        /// Candle interval in minutes (1, 5, 15, 30, 60, 240, 1440, 10080, 21600).
        #[arg(long, default_value = "1")]
        interval: u32,
        /// Request a snapshot on subscribe (default: true).
        #[arg(long)]
        snapshot: Option<bool>,
        /// Client request ID for correlation.
        #[arg(long)]
        req_id: Option<u64>,
    },
    /// Stream instrument metadata updates.
    Instrument {
        /// Trading pairs (omit for all instruments).
        #[arg(num_args = 0..)]
        pairs: Vec<String>,
        /// Request a snapshot on subscribe (default: true).
        #[arg(long)]
        snapshot: Option<bool>,
        /// Client request ID for correlation.
        #[arg(long)]
        req_id: Option<u64>,
    },
    /// Stream trade executions (auth required).
    Executions {
        /// Include last 50 trade fills in snapshot.
        #[arg(long)]
        snap_trades: Option<bool>,
        /// Include open orders in snapshot (default: true).
        #[arg(long)]
        snap_orders: Option<bool>,
        /// Stream all status transitions (default: true). When false, only open/close transitions.
        #[arg(long)]
        order_status: Option<bool>,
        /// Display xstocks in terms of underlying equity (default: true).
        #[arg(long)]
        rebased: Option<bool>,
        /// Include rate-limit counter in stream.
        #[arg(long)]
        ratecounter: Option<bool>,
        /// Stream events for master and subaccounts (pass "all").
        #[arg(long)]
        users: Option<String>,
        /// Client request ID for correlation.
        #[arg(long)]
        req_id: Option<u64>,
    },
    /// Stream balance updates (auth required).
    Balances {
        /// Request a snapshot on subscribe (default: true).
        #[arg(long)]
        snapshot: Option<bool>,
        /// Display xstocks in terms of underlying equity (default: true).
        #[arg(long)]
        rebased: Option<bool>,
        /// Stream events for master and subaccounts (pass "all").
        #[arg(long)]
        users: Option<String>,
        /// Client request ID for correlation.
        #[arg(long)]
        req_id: Option<u64>,
    },
    /// Stream Level 3 order book (auth required).
    Level3 {
        /// Trading pairs.
        #[arg(num_args = 1..)]
        pairs: Vec<String>,
        /// Number of price levels per side.
        #[arg(long, value_parser = ["10", "100", "1000"])]
        depth: Option<u32>,
        /// Request a snapshot on subscribe (default: true).
        #[arg(long)]
        snapshot: Option<bool>,
        /// Client request ID for correlation.
        #[arg(long)]
        req_id: Option<u64>,
    },

    // === WS Request/Response Commands ===
    /// Place a new order via WebSocket (auth required).
    #[command(name = "add-order")]
    AddOrder {
        /// Order type: limit, market, iceberg, stop-loss, stop-loss-limit, take-profit,
        /// take-profit-limit, trailing-stop, trailing-stop-limit, settle-position.
        #[arg(long)]
        order_type: String,
        /// Side: buy or sell.
        #[arg(long)]
        side: String,
        /// Order quantity in base asset (mutually exclusive with --cash-order-qty).
        #[arg(long)]
        order_qty: Option<f64>,
        /// Trading pair symbol (e.g. BTC/USD).
        #[arg(long)]
        symbol: String,
        /// Limit price.
        #[arg(long)]
        limit_price: Option<f64>,
        /// Time-in-force: gtc (default), gtd, ioc.
        #[arg(long)]
        time_in_force: Option<String>,
        /// Fund on margin.
        #[arg(long)]
        margin: Option<bool>,
        /// Post-only order.
        #[arg(long)]
        post_only: Option<bool>,
        /// Reduce-only order.
        #[arg(long)]
        reduce_only: Option<bool>,
        /// Trigger reference price: last or index.
        #[arg(long)]
        trigger_reference: Option<String>,
        /// Trigger price.
        #[arg(long)]
        trigger_price: Option<f64>,
        /// Trigger price type: static, pct, quote.
        #[arg(long)]
        trigger_price_type: Option<String>,
        /// Effective time (RFC3339).
        #[arg(long)]
        effective_time: Option<String>,
        /// Expire time (RFC3339, for GTD orders).
        #[arg(long)]
        expire_time: Option<String>,
        /// Deadline (RFC3339, 500ms-60s from now).
        #[arg(long)]
        deadline: Option<String>,
        /// Client order ID.
        #[arg(long)]
        cl_ord_id: Option<String>,
        /// Numeric client order reference.
        #[arg(long)]
        order_userref: Option<i64>,
        /// Order volume in quote currency (buy market orders without margin).
        #[arg(long)]
        cash_order_qty: Option<f64>,
        /// Display quantity for iceberg orders.
        #[arg(long)]
        display_qty: Option<f64>,
        /// Fee preference: base or quote.
        #[arg(long)]
        fee_preference: Option<String>,
        /// Self-trade prevention: cancel_newest, cancel_oldest, cancel_both.
        #[arg(long)]
        stp_type: Option<String>,
        /// Sub-account/trader ID for granular STP (institutional accounts).
        /// Accepts long UUID, short UUID, or free text up to 18 chars.
        #[arg(long)]
        sender_sub_id: Option<String>,
        /// Validate only, do not submit.
        #[arg(long)]
        validate: Option<bool>,
        /// Client request ID for correlation.
        #[arg(long)]
        req_id: Option<u64>,
    },
    /// Amend an existing order via WebSocket (auth required).
    /// Provide --order-id or --cl-ord-id to identify the order.
    #[command(name = "amend-order")]
    AmendOrder {
        /// Kraken order ID (required unless --cl-ord-id is given).
        #[arg(long)]
        order_id: Option<String>,
        /// New order quantity.
        #[arg(long)]
        order_qty: Option<f64>,
        /// New limit price.
        #[arg(long)]
        limit_price: Option<f64>,
        /// New trigger price (triggered order types).
        #[arg(long)]
        trigger_price: Option<f64>,
        /// Post-only on limit price change.
        #[arg(long)]
        post_only: Option<bool>,
        /// New display quantity (iceberg).
        #[arg(long)]
        display_qty: Option<f64>,
        /// Deadline (RFC3339).
        #[arg(long)]
        deadline: Option<String>,
        /// Client order ID (alternative to --order-id).
        #[arg(long)]
        cl_ord_id: Option<String>,
        /// Symbol (required for non-crypto pairs).
        #[arg(long)]
        symbol: Option<String>,
        /// Client request ID for correlation.
        #[arg(long)]
        req_id: Option<u64>,
    },
    /// Cancel one or more orders via WebSocket (auth required).
    #[command(name = "cancel-order")]
    CancelOrder {
        /// Kraken order IDs to cancel.
        #[arg(long, num_args = 1..)]
        order_id: Vec<String>,
        /// Client order IDs to cancel.
        #[arg(long, num_args = 1..)]
        cl_ord_id: Vec<String>,
        /// Numeric client order refs to cancel.
        #[arg(long, num_args = 1..)]
        order_userref: Vec<i64>,
        /// Client request ID for correlation.
        #[arg(long)]
        req_id: Option<u64>,
    },
    /// Cancel all open orders via WebSocket (auth required).
    #[command(name = "cancel-all")]
    CancelAll {
        /// Client request ID for correlation.
        #[arg(long)]
        req_id: Option<u64>,
    },
    /// Dead man's switch: cancel all orders after timeout (auth required).
    #[command(name = "cancel-after")]
    CancelAfter {
        /// Timeout in seconds (0 to disable, must be < 86400).
        timeout: u64,
        /// Client request ID for correlation.
        #[arg(long)]
        req_id: Option<u64>,
    },
    /// Batch add orders via WebSocket (2-15 orders, single pair, auth required).
    #[command(name = "batch-add")]
    BatchAdd {
        /// Trading pair symbol (e.g. BTC/USD).
        #[arg(long)]
        symbol: String,
        /// Orders as JSON array (see Kraken WS V2 batch_add docs for schema).
        #[arg(long)]
        orders: String,
        /// Validate only, do not submit.
        #[arg(long)]
        validate: Option<bool>,
        /// Deadline (RFC3339).
        #[arg(long)]
        deadline: Option<String>,
        /// Client request ID for correlation.
        #[arg(long)]
        req_id: Option<u64>,
    },
    /// Batch cancel orders via WebSocket (2-50 orders, auth required).
    #[command(name = "batch-cancel")]
    BatchCancel {
        /// Order IDs or user refs to cancel (required).
        #[arg(long, num_args = 1.., required = true)]
        orders: Vec<String>,
        /// Additional client order IDs to cancel.
        #[arg(long, num_args = 1..)]
        cl_ord_id: Vec<String>,
        /// Client request ID for correlation.
        #[arg(long)]
        req_id: Option<u64>,
    },
}

pub(crate) async fn execute(
    cmd: &WsCommand,
    format: OutputFormat,
    spot_client: Option<(&SpotClient, &SpotCredentials)>,
    otp: Option<&str>,
    verbose: bool,
    urls: &WsUrls<'_>,
) -> Result<()> {
    let public_url = urls.public.unwrap_or(DEFAULT_WS_PUBLIC_URL);
    let auth_url = urls.auth.unwrap_or(DEFAULT_WS_AUTH_URL);
    let l3_url = urls.l3.unwrap_or(DEFAULT_WS_L3_URL);

    match cmd {
        WsCommand::Ticker {
            pairs,
            event_trigger,
            snapshot,
            req_id,
        } => {
            let mut extra = serde_json::Map::new();
            if let Some(et) = event_trigger {
                validate_event_trigger(et)?;
                extra.insert("event_trigger".into(), serde_json::json!(et));
            }
            add_optional_params(&mut extra, *snapshot, *req_id);
            let extra_val = if extra.is_empty() {
                None
            } else {
                Some(Value::Object(extra))
            };
            stream_public("ticker", pairs, extra_val, format, verbose, public_url).await
        }
        WsCommand::Trades {
            pairs,
            snapshot,
            req_id,
        } => {
            let mut extra = serde_json::Map::new();
            add_optional_params(&mut extra, *snapshot, *req_id);
            let extra_val = if extra.is_empty() {
                None
            } else {
                Some(Value::Object(extra))
            };
            stream_public("trade", pairs, extra_val, format, verbose, public_url).await
        }
        WsCommand::Book {
            pairs,
            depth,
            snapshot,
            req_id,
        } => {
            validate_book_depth(*depth)?;
            let mut extra = serde_json::Map::new();
            extra.insert("depth".into(), serde_json::json!(depth));
            add_optional_params(&mut extra, *snapshot, *req_id);
            stream_public(
                "book",
                pairs,
                Some(Value::Object(extra)),
                format,
                verbose,
                public_url,
            )
            .await
        }
        WsCommand::Ohlc {
            pairs,
            interval,
            snapshot,
            req_id,
        } => {
            validate_ohlc_interval(*interval)?;
            let mut extra = serde_json::Map::new();
            extra.insert("interval".into(), serde_json::json!(interval));
            add_optional_params(&mut extra, *snapshot, *req_id);
            stream_public(
                "ohlc",
                pairs,
                Some(Value::Object(extra)),
                format,
                verbose,
                public_url,
            )
            .await
        }
        WsCommand::Instrument {
            pairs,
            snapshot,
            req_id,
        } => {
            let mut extra = serde_json::Map::new();
            add_optional_params(&mut extra, *snapshot, *req_id);
            let extra_val = if extra.is_empty() {
                None
            } else {
                Some(Value::Object(extra))
            };
            stream_public("instrument", pairs, extra_val, format, verbose, public_url).await
        }
        WsCommand::Executions {
            snap_trades,
            snap_orders,
            order_status,
            rebased,
            ratecounter,
            users,
            req_id,
        } => {
            let (client, creds) = spot_client.ok_or_else(|| {
                KrakenError::Auth("Credentials required for private WebSocket streams".into())
            })?;
            let mut extra = serde_json::Map::new();
            if let Some(v) = snap_trades {
                extra.insert("snap_trades".into(), serde_json::json!(v));
            }
            if let Some(v) = snap_orders {
                extra.insert("snap_orders".into(), serde_json::json!(v));
            }
            if let Some(v) = order_status {
                extra.insert("order_status".into(), serde_json::json!(v));
            }
            if let Some(v) = rebased {
                extra.insert("rebased".into(), serde_json::json!(v));
            }
            if let Some(v) = ratecounter {
                extra.insert("ratecounter".into(), serde_json::json!(v));
            }
            if let Some(v) = users {
                extra.insert("users".into(), serde_json::json!(v));
            }
            if let Some(id) = req_id {
                extra.insert("req_id".into(), serde_json::json!(id));
            }
            stream_private_with_params(
                "executions",
                client,
                creds,
                otp,
                format,
                verbose,
                auth_url,
                extra,
            )
            .await
        }
        WsCommand::Balances {
            snapshot,
            rebased,
            users,
            req_id,
        } => {
            let (client, creds) = spot_client.ok_or_else(|| {
                KrakenError::Auth("Credentials required for private WebSocket streams".into())
            })?;
            let mut extra = serde_json::Map::new();
            if let Some(s) = snapshot {
                extra.insert("snapshot".into(), serde_json::json!(s));
            }
            if let Some(v) = rebased {
                extra.insert("rebased".into(), serde_json::json!(v));
            }
            if let Some(v) = users {
                extra.insert("users".into(), serde_json::json!(v));
            }
            if let Some(id) = req_id {
                extra.insert("req_id".into(), serde_json::json!(id));
            }
            stream_private_with_params(
                "balances", client, creds, otp, format, verbose, auth_url, extra,
            )
            .await
        }
        WsCommand::Level3 {
            pairs,
            depth,
            snapshot,
            req_id,
        } => {
            let (client, creds) = spot_client.ok_or_else(|| {
                KrakenError::Auth("Credentials required for Level 3 WebSocket streams".into())
            })?;
            let mut extra = serde_json::Map::new();
            if !pairs.is_empty() {
                extra.insert("symbol".into(), serde_json::json!(pairs));
            }
            if let Some(d) = depth {
                extra.insert("depth".into(), serde_json::json!(d));
            }
            add_optional_params(&mut extra, *snapshot, *req_id);
            stream_private_with_params("level3", client, creds, otp, format, verbose, l3_url, extra)
                .await
        }

        // === WS Request/Response Commands ===
        WsCommand::AddOrder {
            order_type,
            side,
            order_qty,
            symbol,
            limit_price,
            time_in_force,
            margin,
            post_only,
            reduce_only,
            trigger_reference,
            trigger_price,
            trigger_price_type,
            effective_time,
            expire_time,
            deadline,
            cl_ord_id,
            order_userref,
            cash_order_qty,
            display_qty,
            fee_preference,
            stp_type,
            sender_sub_id,
            validate,
            req_id,
        } => {
            let (client, creds) = spot_client.ok_or_else(|| {
                KrakenError::Auth("Credentials required for WebSocket order commands".into())
            })?;
            validate_order_type(order_type)?;
            validate_side(side)?;
            match (order_qty, cash_order_qty) {
                (Some(_), Some(_)) => {
                    return Err(KrakenError::Validation(
                        "--order-qty and --cash-order-qty are mutually exclusive".into(),
                    ));
                }
                (None, None) => {
                    return Err(KrakenError::Validation(
                        "Either --order-qty or --cash-order-qty is required".into(),
                    ));
                }
                (None, Some(_)) if order_type != "market" || side != "buy" => {
                    return Err(KrakenError::Validation(
                        "--cash-order-qty is only available for market buy orders".into(),
                    ));
                }
                _ => {}
            }
            let mut params = serde_json::json!({
                "order_type": order_type,
                "side": side,
                "symbol": symbol,
            });
            let obj = params.as_object_mut().unwrap();
            if let Some(v) = order_qty {
                obj.insert("order_qty".into(), serde_json::json!(v));
            }
            if let Some(v) = cash_order_qty {
                obj.insert("cash_order_qty".into(), serde_json::json!(v));
            }
            if let Some(v) = limit_price {
                obj.insert("limit_price".into(), serde_json::json!(v));
            }
            if let Some(v) = time_in_force {
                obj.insert("time_in_force".into(), serde_json::json!(v));
            }
            if let Some(v) = margin {
                obj.insert("margin".into(), serde_json::json!(v));
            }
            if let Some(v) = post_only {
                obj.insert("post_only".into(), serde_json::json!(v));
            }
            if let Some(v) = reduce_only {
                obj.insert("reduce_only".into(), serde_json::json!(v));
            }
            if trigger_reference.is_some()
                || trigger_price.is_some()
                || trigger_price_type.is_some()
            {
                let mut triggers = serde_json::Map::new();
                if let Some(v) = trigger_reference {
                    triggers.insert("reference".into(), serde_json::json!(v));
                }
                if let Some(v) = trigger_price {
                    triggers.insert("price".into(), serde_json::json!(v));
                }
                if let Some(v) = trigger_price_type {
                    triggers.insert("price_type".into(), serde_json::json!(v));
                }
                obj.insert("triggers".into(), Value::Object(triggers));
            }
            if let Some(v) = effective_time {
                obj.insert("effective_time".into(), serde_json::json!(v));
            }
            if let Some(v) = expire_time {
                obj.insert("expire_time".into(), serde_json::json!(v));
            }
            if let Some(v) = deadline {
                obj.insert("deadline".into(), serde_json::json!(v));
            }
            if let Some(v) = cl_ord_id {
                obj.insert("cl_ord_id".into(), serde_json::json!(v));
            }
            if let Some(v) = order_userref {
                obj.insert("order_userref".into(), serde_json::json!(v));
            }
            if let Some(v) = display_qty {
                obj.insert("display_qty".into(), serde_json::json!(v));
            }
            if let Some(v) = fee_preference {
                obj.insert("fee_preference".into(), serde_json::json!(v));
            }
            if let Some(v) = stp_type {
                obj.insert("stp_type".into(), serde_json::json!(v));
            }
            if let Some(v) = sender_sub_id {
                obj.insert("sender_sub_id".into(), serde_json::json!(v));
            }
            if let Some(v) = validate {
                obj.insert("validate".into(), serde_json::json!(v));
            }
            ws_request(
                "add_order",
                params,
                *req_id,
                client,
                creds,
                otp,
                format,
                verbose,
                auth_url,
            )
            .await
        }
        WsCommand::AmendOrder {
            order_id,
            order_qty,
            limit_price,
            trigger_price,
            post_only,
            display_qty,
            deadline,
            cl_ord_id,
            symbol,
            req_id,
        } => {
            let (client, creds) = spot_client.ok_or_else(|| {
                KrakenError::Auth("Credentials required for WebSocket order commands".into())
            })?;
            if order_id.is_none() && cl_ord_id.is_none() {
                return Err(KrakenError::Validation(
                    "Either --order-id or --cl-ord-id is required for amend-order".into(),
                ));
            }
            let mut params = serde_json::json!({});
            let obj = params.as_object_mut().unwrap();
            if let Some(v) = order_id {
                obj.insert("order_id".into(), serde_json::json!(v));
            }
            if let Some(v) = cl_ord_id {
                obj.insert("cl_ord_id".into(), serde_json::json!(v));
            }
            if let Some(v) = order_qty {
                obj.insert("order_qty".into(), serde_json::json!(v));
            }
            if let Some(v) = limit_price {
                obj.insert("limit_price".into(), serde_json::json!(v));
            }
            if let Some(v) = trigger_price {
                obj.insert("trigger_price".into(), serde_json::json!(v));
            }
            if let Some(v) = post_only {
                obj.insert("post_only".into(), serde_json::json!(v));
            }
            if let Some(v) = display_qty {
                obj.insert("display_qty".into(), serde_json::json!(v));
            }
            if let Some(v) = deadline {
                obj.insert("deadline".into(), serde_json::json!(v));
            }
            if let Some(v) = symbol {
                obj.insert("symbol".into(), serde_json::json!(v));
            }
            ws_request(
                "amend_order",
                params,
                *req_id,
                client,
                creds,
                otp,
                format,
                verbose,
                auth_url,
            )
            .await
        }
        WsCommand::CancelOrder {
            order_id,
            cl_ord_id,
            order_userref,
            req_id,
        } => {
            let (client, creds) = spot_client.ok_or_else(|| {
                KrakenError::Auth("Credentials required for WebSocket order commands".into())
            })?;
            if order_id.is_empty() && cl_ord_id.is_empty() && order_userref.is_empty() {
                return Err(KrakenError::Validation(
                    "At least one --order-id, --cl-ord-id, or --order-userref is required".into(),
                ));
            }
            let mut params = serde_json::Map::new();
            if !order_id.is_empty() {
                params.insert("order_id".into(), serde_json::json!(order_id));
            }
            if !cl_ord_id.is_empty() {
                params.insert("cl_ord_id".into(), serde_json::json!(cl_ord_id));
            }
            if !order_userref.is_empty() {
                params.insert("order_userref".into(), serde_json::json!(order_userref));
            }
            let item_count = order_id.len() + cl_ord_id.len() + order_userref.len();
            ws_request_multi(
                "cancel_order",
                Value::Object(params),
                *req_id,
                item_count,
                client,
                creds,
                otp,
                format,
                verbose,
                auth_url,
            )
            .await
        }
        WsCommand::CancelAll { req_id } => {
            let (client, creds) = spot_client.ok_or_else(|| {
                KrakenError::Auth("Credentials required for WebSocket order commands".into())
            })?;
            ws_request(
                "cancel_all",
                Value::Object(serde_json::Map::new()),
                *req_id,
                client,
                creds,
                otp,
                format,
                verbose,
                auth_url,
            )
            .await
        }
        WsCommand::CancelAfter { timeout, req_id } => {
            let (client, creds) = spot_client.ok_or_else(|| {
                KrakenError::Auth("Credentials required for WebSocket order commands".into())
            })?;
            if *timeout >= 86400 {
                return Err(KrakenError::Validation(
                    "Timeout must be < 86400 seconds".into(),
                ));
            }
            let params = serde_json::json!({ "timeout": timeout });
            ws_request(
                "cancel_all_orders_after",
                params,
                *req_id,
                client,
                creds,
                otp,
                format,
                verbose,
                auth_url,
            )
            .await
        }
        WsCommand::BatchAdd {
            symbol,
            orders,
            validate,
            deadline,
            req_id,
        } => {
            let (client, creds) = spot_client.ok_or_else(|| {
                KrakenError::Auth("Credentials required for WebSocket order commands".into())
            })?;
            let orders_val: Value = serde_json::from_str(orders)
                .map_err(|e| KrakenError::Validation(format!("Invalid orders JSON: {e}")))?;
            if !orders_val.is_array() {
                return Err(KrakenError::Validation(
                    "--orders must be a JSON array".into(),
                ));
            }
            let count = orders_val.as_array().unwrap().len();
            if !(2..=15).contains(&count) {
                return Err(KrakenError::Validation(format!(
                    "Batch requires 2-15 orders, got {count}"
                )));
            }
            let mut params = serde_json::json!({
                "symbol": symbol,
                "orders": orders_val,
            });
            let obj = params.as_object_mut().unwrap();
            if let Some(v) = validate {
                obj.insert("validate".into(), serde_json::json!(v));
            }
            if let Some(v) = deadline {
                obj.insert("deadline".into(), serde_json::json!(v));
            }
            ws_request(
                "batch_add",
                params,
                *req_id,
                client,
                creds,
                otp,
                format,
                verbose,
                auth_url,
            )
            .await
        }
        WsCommand::BatchCancel {
            orders,
            cl_ord_id,
            req_id,
        } => {
            let (client, creds) = spot_client.ok_or_else(|| {
                KrakenError::Auth("Credentials required for WebSocket order commands".into())
            })?;
            let total = orders.len() + cl_ord_id.len();
            if !(2..=50).contains(&total) {
                return Err(KrakenError::Validation(format!(
                    "Batch cancel requires 2-50 identifiers, got {total}"
                )));
            }
            let mut params = serde_json::Map::new();
            params.insert("orders".into(), serde_json::json!(orders));
            if !cl_ord_id.is_empty() {
                params.insert("cl_ord_id".into(), serde_json::json!(cl_ord_id));
            }
            ws_request(
                "batch_cancel",
                Value::Object(params),
                *req_id,
                client,
                creds,
                otp,
                format,
                verbose,
                auth_url,
            )
            .await
        }
    }
}

/// Build a V2 subscribe payload. Omits `symbol` when `pairs` is empty.
/// `req_id` is placed at the top level per V2 spec, not inside `params`.
pub(crate) fn build_subscribe_payload(
    channel: &str,
    pairs: &[String],
    extra_params: Option<&Value>,
) -> Value {
    let mut params = serde_json::json!({ "channel": channel });
    let mut top_level_req_id: Option<Value> = None;
    if !pairs.is_empty() {
        params["symbol"] = serde_json::json!(pairs);
    }
    if let Some(extra) = extra_params {
        if let (Some(target), Some(source)) = (params.as_object_mut(), extra.as_object()) {
            for (k, v) in source {
                if k == "req_id" {
                    top_level_req_id = Some(v.clone());
                } else {
                    target.insert(k.clone(), v.clone());
                }
            }
        }
    }
    let mut msg = serde_json::json!({
        "method": "subscribe",
        "params": params,
    });
    if let Some(id) = top_level_req_id {
        msg["req_id"] = id;
    }
    msg
}

/// Replace all occurrences of `token` with `<redacted>` in a payload string.
pub(crate) fn redact_token_in_payload(payload: &str, token: &str) -> String {
    payload.replace(token, "<redacted>")
}

const VALID_BOOK_DEPTHS: &[u32] = &[10, 25, 100, 500, 1000];
const VALID_OHLC_INTERVALS: &[u32] = &[1, 5, 15, 30, 60, 240, 1440, 10080, 21600];
const VALID_EVENT_TRIGGERS: &[&str] = &["bbo", "trades"];

fn validate_book_depth(depth: u32) -> Result<()> {
    if !VALID_BOOK_DEPTHS.contains(&depth) {
        return Err(KrakenError::Validation(format!(
            "Invalid book depth: {depth}. Valid values: {}",
            VALID_BOOK_DEPTHS
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(())
}

fn validate_ohlc_interval(interval: u32) -> Result<()> {
    if !VALID_OHLC_INTERVALS.contains(&interval) {
        return Err(KrakenError::Validation(format!(
            "Invalid OHLC interval: {interval}. Valid values: {}",
            VALID_OHLC_INTERVALS
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(())
}

fn validate_event_trigger(trigger: &str) -> Result<()> {
    if !VALID_EVENT_TRIGGERS.contains(&trigger) {
        return Err(KrakenError::Validation(format!(
            "Invalid event_trigger: {trigger}. Valid values: {}",
            VALID_EVENT_TRIGGERS.join(", ")
        )));
    }
    Ok(())
}

const VALID_ORDER_TYPES: &[&str] = &[
    "limit",
    "market",
    "iceberg",
    "stop-loss",
    "stop-loss-limit",
    "take-profit",
    "take-profit-limit",
    "trailing-stop",
    "trailing-stop-limit",
    "settle-position",
];

fn validate_order_type(ot: &str) -> Result<()> {
    if !VALID_ORDER_TYPES.contains(&ot) {
        return Err(KrakenError::Validation(format!(
            "Invalid order_type: {ot}. Valid values: {}",
            VALID_ORDER_TYPES.join(", ")
        )));
    }
    Ok(())
}

fn validate_side(side: &str) -> Result<()> {
    if side != "buy" && side != "sell" {
        return Err(KrakenError::Validation(format!(
            "Invalid side: {side}. Valid values: buy, sell"
        )));
    }
    Ok(())
}

fn add_optional_params(
    extra: &mut serde_json::Map<String, Value>,
    snapshot: Option<bool>,
    req_id: Option<u64>,
) {
    if let Some(s) = snapshot {
        extra.insert("snapshot".into(), serde_json::json!(s));
    }
    if let Some(id) = req_id {
        extra.insert("req_id".into(), serde_json::json!(id));
    }
}

#[allow(clippy::too_many_arguments)]
async fn ws_send_request(
    method: &str,
    params: Value,
    req_id: Option<u64>,
    client: &SpotClient,
    creds: &SpotCredentials,
    otp: Option<&str>,
    verbose: bool,
    url: &str,
) -> Result<(WebSocketStream<MaybeTlsStream<TcpStream>>, String)> {
    let token_resp = client
        .private_post(
            "GetWebSocketsToken",
            HashMap::new(),
            creds,
            otp,
            true,
            verbose,
        )
        .await?;
    let token = token_resp
        .get("token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| KrakenError::Auth("Failed to obtain WebSocket token".into()))?
        .to_string();

    if verbose {
        output::verbose(&format!("Connecting to {url}"));
    }

    let (mut ws, _) = ws_connect(url).await?;

    let mut msg_params = params;
    if let Some(obj) = msg_params.as_object_mut() {
        obj.insert("token".into(), serde_json::json!(&token));
    }
    let mut msg = serde_json::json!({
        "method": method,
        "params": msg_params,
    });
    if let Some(id) = req_id {
        msg["req_id"] = serde_json::json!(id);
    }

    let payload = serde_json::to_string(&msg).map_err(|e| KrakenError::Parse(e.to_string()))?;
    if verbose {
        let redacted = redact_token_in_payload(&payload, &token);
        output::verbose(&format!("Sending: {redacted}"));
    }
    ws.send(Message::Text(payload))
        .await
        .map_err(|e| KrakenError::WebSocket(format!("Send failed: {e}")))?;

    Ok((ws, token))
}

#[allow(clippy::too_many_arguments)]
async fn ws_request(
    method: &str,
    params: Value,
    req_id: Option<u64>,
    client: &SpotClient,
    creds: &SpotCredentials,
    otp: Option<&str>,
    format: OutputFormat,
    verbose: bool,
    url: &str,
) -> Result<()> {
    let (mut ws, token) =
        ws_send_request(method, params, req_id, client, creds, otp, verbose, url).await?;

    let timeout = tokio::time::sleep(Duration::from_secs(10));
    tokio::pin!(timeout);
    loop {
        tokio::select! {
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(val) = serde_json::from_str::<Value>(&text) {
                            let resp_method = val.get("method").and_then(|m| m.as_str()).unwrap_or("");
                            if resp_method == method {
                                let _ = ws.close(None).await;
                                return render_and_check(&val, format);
                            }
                            if verbose && resp_method != "pong" {
                                let redacted = redact_token_in_payload(&text, &token);
                                output::verbose(&format!("Ignoring message: {redacted}"));
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        return Err(KrakenError::WebSocket("Connection closed before response".into()));
                    }
                    Some(Err(e)) => {
                        return Err(KrakenError::WebSocket(format!("Stream error: {e}")));
                    }
                    _ => {}
                }
            }
            _ = &mut timeout => {
                let _ = ws.close(None).await;
                return Err(KrakenError::WebSocket("Timed out waiting for response".into()));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn ws_request_multi(
    method: &str,
    params: Value,
    req_id: Option<u64>,
    expected_count: usize,
    client: &SpotClient,
    creds: &SpotCredentials,
    otp: Option<&str>,
    format: OutputFormat,
    verbose: bool,
    url: &str,
) -> Result<()> {
    let (mut ws, token) =
        ws_send_request(method, params, req_id, client, creds, otp, verbose, url).await?;

    let mut received = 0usize;
    let mut had_failure = false;
    let timeout = tokio::time::sleep(Duration::from_secs(10));
    tokio::pin!(timeout);
    loop {
        tokio::select! {
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(val) = serde_json::from_str::<Value>(&text) {
                            let resp_method = val.get("method").and_then(|m| m.as_str()).unwrap_or("");
                            if resp_method == method {
                                if render_and_check(&val, format).is_err() {
                                    had_failure = true;
                                }
                                received += 1;
                                if received >= expected_count {
                                    let _ = ws.close(None).await;
                                    if had_failure {
                                        return Err(KrakenError::Api {
                                            category: errors::ErrorCategory::Api,
                                            message: format!("{method}: one or more items failed"),
                                        });
                                    }
                                    return Ok(());
                                }
                            } else if verbose && resp_method != "pong" {
                                let redacted = redact_token_in_payload(&text, &token);
                                output::verbose(&format!("Ignoring message: {redacted}"));
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        let _ = ws.close(None).await;
                        if received == 0 {
                            return Err(KrakenError::WebSocket("Connection closed before response".into()));
                        }
                        if had_failure || received < expected_count {
                            return Err(KrakenError::Api {
                                category: errors::ErrorCategory::Api,
                                message: format!("{method}: {received}/{expected_count} responses, failures: {had_failure}"),
                            });
                        }
                        return Ok(());
                    }
                    Some(Err(e)) => {
                        return Err(KrakenError::WebSocket(format!("Stream error: {e}")));
                    }
                    _ => {}
                }
            }
            _ = &mut timeout => {
                let _ = ws.close(None).await;
                if received == 0 {
                    return Err(KrakenError::WebSocket("Timed out waiting for response".into()));
                }
                if had_failure || received < expected_count {
                    return Err(KrakenError::Api {
                        category: errors::ErrorCategory::Api,
                        message: format!("{method}: {received}/{expected_count} responses, failures: {had_failure}"),
                    });
                }
                return Ok(());
            }
        }
    }
}

fn render_and_check(val: &Value, format: OutputFormat) -> Result<()> {
    let method = val.get("method").and_then(|m| m.as_str()).unwrap_or("-");
    let success = val
        .get("success")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);

    match format {
        OutputFormat::Json => {
            output::json::render_ndjson(val);
        }
        OutputFormat::Table => {
            if success {
                if let Some(result) = val.get("result") {
                    let summary = format_ws_result(method, result);
                    output::table::render_stream_line(&[("method", method), ("result", &summary)]);
                } else {
                    output::table::render_stream_line(&[("method", method), ("result", "ok")]);
                }
            } else {
                let error = val
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("unknown error");
                output::table::render_stream_line(&[("method", method), ("error", error)]);
            }
        }
    }

    if success {
        Ok(())
    } else {
        let error = val
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("unknown error");
        Err(KrakenError::Api {
            category: errors::ErrorCategory::Api,
            message: format!("{method}: {error}"),
        })
    }
}

fn format_ws_result(method: &str, result: &Value) -> String {
    match method {
        "add_order" => {
            let order_id = result
                .get("order_id")
                .and_then(|s| s.as_str())
                .unwrap_or("-");
            let cl = result.get("cl_ord_id").and_then(|s| s.as_str());
            match cl {
                Some(id) => format!("order_id:{order_id} cl_ord_id:{id}"),
                None => format!("order_id:{order_id}"),
            }
        }
        "amend_order" => {
            let amend_id = result
                .get("amend_id")
                .and_then(|s| s.as_str())
                .unwrap_or("-");
            let order_id = result.get("order_id").and_then(|s| s.as_str());
            let cl_ord_id = result.get("cl_ord_id").and_then(|s| s.as_str());
            match (order_id, cl_ord_id) {
                (Some(oid), Some(cid)) => {
                    format!("amend_id:{amend_id} order_id:{oid} cl_ord_id:{cid}")
                }
                (Some(oid), None) => format!("amend_id:{amend_id} order_id:{oid}"),
                (None, Some(cid)) => format!("amend_id:{amend_id} cl_ord_id:{cid}"),
                (None, None) => format!("amend_id:{amend_id}"),
            }
        }
        "cancel_order" => {
            let order_id = result
                .get("order_id")
                .and_then(|s| s.as_str())
                .unwrap_or("-");
            format!("order_id:{order_id}")
        }
        "cancel_all" | "batch_cancel" => {
            let count = result.get("count").and_then(|c| c.as_u64()).unwrap_or(0);
            format!("cancelled:{count}")
        }
        "cancel_all_orders_after" => {
            let trigger = result
                .get("triggerTime")
                .and_then(|t| t.as_str())
                .unwrap_or("-");
            let current = result
                .get("currentTime")
                .and_then(|t| t.as_str())
                .unwrap_or("-");
            format!("trigger:{trigger} current:{current}")
        }
        "batch_add" => {
            if let Some(arr) = result.as_array() {
                arr.iter()
                    .map(|item| {
                        let oid = item.get("order_id").and_then(|s| s.as_str()).unwrap_or("-");
                        oid.to_string()
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                result.to_string()
            }
        }
        _ => result.to_string(),
    }
}

fn ws_handshake_request(url: &str) -> Result<Request<()>> {
    let mut request = url
        .into_client_request()
        .map_err(|e| KrakenError::WebSocket(format!("Invalid WebSocket URL: {e}")))?;
    let headers = request.headers_mut();
    headers.insert(
        "User-Agent",
        HeaderValue::from_str(crate::telemetry::user_agent())
            .map_err(|e| KrakenError::WebSocket(format!("Invalid header value: {e}")))?,
    );
    headers.insert(
        "X-Kraken-Client",
        HeaderValue::from_static(crate::telemetry::CLIENT_NAME),
    );
    headers.insert(
        "X-Kraken-Client-Version",
        HeaderValue::from_static(crate::telemetry::CLIENT_VERSION),
    );
    headers.insert(
        "X-Kraken-Agent-Client",
        HeaderValue::from_str(crate::telemetry::agent_client())
            .map_err(|e| KrakenError::WebSocket(format!("Invalid header value: {e}")))?,
    );
    headers.insert(
        "X-Kraken-Instance-Id",
        HeaderValue::from_str(crate::telemetry::instance_id())
            .map_err(|e| KrakenError::WebSocket(format!("Invalid header value: {e}")))?,
    );
    headers.insert(
        "x-korigin",
        HeaderValue::from_static(crate::telemetry::KORIGIN_WS),
    );
    Ok(request)
}

type WsConnection = (
    WebSocketStream<MaybeTlsStream<TcpStream>>,
    tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
);

async fn ws_connect(url: &str) -> Result<WsConnection> {
    let skip_verify = env::var("KRAKEN_DANGER_ACCEPT_INVALID_CERTS")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    let request = ws_handshake_request(url)?;
    let connect = if skip_verify {
        eprintln!("WARNING: TLS certificate verification is DISABLED for WebSocket connections.");
        let config = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth();
        let connector = Connector::Rustls(Arc::new(config));
        connect_async_tls_with_config(request, None, false, Some(connector)).await
    } else {
        connect_async(request).await
    };
    connect.map_err(|e| KrakenError::WebSocket(format!("Connection failed: {e}")))
}

#[derive(Debug)]
struct NoVerify;

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, RustlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
            SignatureScheme::ED448,
        ]
    }
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
            output::warn(&format!(
                "{label}: reconnect safety budget reached ({MAX_RECONNECTS_PER_WINDOW}/{RECONNECT_WINDOW_SECS}s), waiting {}ms",
                wait_for.as_millis()
            ));
            tokio::time::sleep(wait_for).await;
        }
        prune_reconnect_window(reconnect_history);
    }
    reconnect_history.push_back(Instant::now());
}

async fn stream_public(
    channel: &str,
    pairs: &[String],
    extra_params: Option<Value>,
    format: OutputFormat,
    verbose: bool,
    url: &str,
) -> Result<()> {
    let mut reconnect_count = 0u32;
    let mut reconnect_history: VecDeque<Instant> = VecDeque::new();

    loop {
        if verbose {
            if reconnect_count > 0 {
                output::verbose(&format!(
                    "Reconnecting to {url} (attempt {reconnect_count}/{MAX_RECONNECTS})"
                ));
            } else {
                output::verbose(&format!("Connecting to {url}"));
            }
        }

        let ws_result = ws_connect(url).await;
        let (mut ws, _) = match ws_result {
            Ok(conn) => conn,
            Err(e) => {
                if reconnect_count >= MAX_RECONNECTS {
                    return Err(KrakenError::WebSocket(format!(
                        "Connection failed after {MAX_RECONNECTS} reconnect attempts: {e}"
                    )));
                }
                reconnect_count += 1;
                enforce_reconnect_budget(&mut reconnect_history, "spot-ws-public").await;
                let backoff = reconnect_backoff_ms(reconnect_count);
                output::warn(&format!(
                    "WebSocket connection failed: {e}, retrying in {backoff}ms"
                ));
                tokio::time::sleep(Duration::from_millis(backoff)).await;
                continue;
            }
        };

        let connected_at = Instant::now();

        let subscribe = build_subscribe_payload(channel, pairs, extra_params.as_ref());

        let msg =
            serde_json::to_string(&subscribe).map_err(|e| KrakenError::Parse(e.to_string()))?;

        if verbose {
            output::verbose(&format!("Subscribing: {msg}"));
        }

        ws.send(Message::Text(msg))
            .await
            .map_err(|e| KrakenError::WebSocket(format!("Send failed: {e}")))?;

        let shutdown = tokio::signal::ctrl_c();
        tokio::pin!(shutdown);

        let disconnected = loop {
            tokio::select! {
                msg = ws.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            handle_ws_message(&text, format);
                        }
                        Some(Ok(Message::Close(_))) => {
                            if verbose {
                                output::verbose("WebSocket closed by server");
                            }
                            break true;
                        }
                        Some(Err(e)) => {
                            if verbose {
                                output::verbose(&format!("WebSocket stream error: {e}"));
                            }
                            break true;
                        }
                        None => break true,
                        _ => {}
                    }
                }
                _ = &mut shutdown => {
                    if verbose {
                        output::verbose("Shutting down WebSocket connection");
                    }
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
                return Err(KrakenError::WebSocket(format!(
                    "Connection lost after {MAX_RECONNECTS} reconnect attempts"
                )));
            }
            enforce_reconnect_budget(&mut reconnect_history, "spot-ws-public").await;
            let backoff = reconnect_backoff_ms(reconnect_count);
            output::warn(&format!(
                "WebSocket disconnected, reconnecting in {backoff}ms ({reconnect_count}/{MAX_RECONNECTS})"
            ));
            tokio::time::sleep(Duration::from_millis(backoff)).await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn stream_private_with_params(
    channel: &str,
    client: &SpotClient,
    creds: &SpotCredentials,
    otp: Option<&str>,
    format: OutputFormat,
    verbose: bool,
    url: &str,
    extra_params: serde_json::Map<String, Value>,
) -> Result<()> {
    let mut reconnect_count = 0u32;
    let mut reconnect_history: VecDeque<Instant> = VecDeque::new();
    let mut cached_token: Option<String> = None;

    loop {
        let token = match cached_token.take() {
            Some(t) => t,
            None => {
                let token_result = client
                    .private_post(
                        "GetWebSocketsToken",
                        HashMap::new(),
                        creds,
                        otp,
                        true,
                        verbose,
                    )
                    .await;

                match token_result {
                    Ok(resp) => resp
                        .get("token")
                        .and_then(|t| t.as_str())
                        .ok_or_else(|| {
                            KrakenError::Auth("Failed to obtain WebSocket token".into())
                        })?
                        .to_string(),
                    Err(KrakenError::RateLimit { ref message, .. }) => {
                        if reconnect_count >= MAX_RECONNECTS {
                            return Err(KrakenError::WebSocket(format!(
                                "Token refresh rate-limited after {MAX_RECONNECTS} attempts: {message}"
                            )));
                        }
                        reconnect_count += 1;
                        enforce_reconnect_budget(&mut reconnect_history, "spot-ws-token").await;
                        let backoff = reconnect_backoff_ms(reconnect_count);
                        output::warn(&format!(
                            "Token refresh rate-limited ({message}), retrying in {backoff}ms"
                        ));
                        tokio::time::sleep(Duration::from_millis(backoff)).await;
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
        };

        if verbose {
            if reconnect_count > 0 {
                output::verbose(&format!(
                    "Reconnecting to {url} (attempt {reconnect_count}/{MAX_RECONNECTS})"
                ));
            } else {
                output::verbose(&format!("Connecting to {url}"));
            }
        }

        let ws_result = ws_connect(url).await;
        let (mut ws, _) = match ws_result {
            Ok(conn) => conn,
            Err(e) => {
                if reconnect_count >= MAX_RECONNECTS {
                    return Err(KrakenError::WebSocket(format!(
                        "Connection failed after {MAX_RECONNECTS} reconnect attempts: {e}"
                    )));
                }
                reconnect_count += 1;
                enforce_reconnect_budget(&mut reconnect_history, "spot-ws-private").await;
                let backoff = reconnect_backoff_ms(reconnect_count);
                output::warn(&format!(
                    "WebSocket connection failed: {e}, retrying in {backoff}ms"
                ));
                tokio::time::sleep(Duration::from_millis(backoff)).await;
                continue;
            }
        };

        let connected_at = Instant::now();

        let mut params = serde_json::json!({
            "channel": channel,
            "token": token,
        });
        let mut top_level_req_id: Option<Value> = None;
        if let Some(obj) = params.as_object_mut() {
            for (k, v) in &extra_params {
                if k == "req_id" {
                    top_level_req_id = Some(v.clone());
                } else {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
        let mut subscribe = serde_json::json!({
            "method": "subscribe",
            "params": params,
        });
        if let Some(id) = top_level_req_id {
            subscribe["req_id"] = id;
        }

        let msg =
            serde_json::to_string(&subscribe).map_err(|e| KrakenError::Parse(e.to_string()))?;
        if verbose {
            let redacted = redact_token_in_payload(&msg, &token);
            output::verbose(&format!("Subscribing: {redacted}"));
        }
        ws.send(Message::Text(msg))
            .await
            .map_err(|e| KrakenError::WebSocket(format!("Send failed: {e}")))?;

        let shutdown = tokio::signal::ctrl_c();
        tokio::pin!(shutdown);

        let disconnected = loop {
            tokio::select! {
                msg = ws.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            handle_ws_message(&text, format);
                        }
                        Some(Ok(Message::Close(_))) => {
                            if verbose {
                                output::verbose("WebSocket closed by server");
                            }
                            break true;
                        }
                        Some(Err(e)) => {
                            if verbose {
                                output::verbose(&format!("WebSocket stream error: {e}"));
                            }
                            break true;
                        }
                        None => break true,
                        _ => {}
                    }
                }
                _ = &mut shutdown => {
                    if verbose {
                        output::verbose("Shutting down WebSocket connection");
                    }
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
                return Err(KrakenError::WebSocket(format!(
                    "Connection lost after {MAX_RECONNECTS} reconnect attempts"
                )));
            }
            enforce_reconnect_budget(&mut reconnect_history, "spot-ws-private").await;
            let backoff = reconnect_backoff_ms(reconnect_count);
            output::warn(&format!(
                "WebSocket disconnected, reconnecting in {backoff}ms ({reconnect_count}/{MAX_RECONNECTS})"
            ));
            tokio::time::sleep(Duration::from_millis(backoff)).await;
        }
    }
}

fn handle_ws_message(text: &str, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            if let Ok(val) = serde_json::from_str::<Value>(text) {
                output::json::render_ndjson(&val);
            }
        }
        OutputFormat::Table => {
            if let Ok(val) = serde_json::from_str::<Value>(text) {
                let channel = val
                    .get("channel")
                    .and_then(|c| c.as_str())
                    .unwrap_or("data");
                let type_str = val.get("type").and_then(|t| t.as_str()).unwrap_or("");

                if type_str == "subscribe" || channel == "status" || channel == "heartbeat" {
                    return;
                }

                if let Some(data) = val.get("data") {
                    let summary = format_ws_data(channel, type_str, data);
                    output::table::render_stream_line(&[("channel", channel), ("data", &summary)]);
                }
            }
        }
    }
}

fn format_ws_data(channel: &str, msg_type: &str, data: &Value) -> String {
    match channel {
        "ticker" => {
            if let Some(arr) = data.as_array() {
                arr.iter()
                    .filter_map(|item| {
                        let symbol = item.get("symbol").and_then(|s| s.as_str())?;
                        let last = item.get("last").map(|v| v.to_string()).unwrap_or_default();
                        Some(format!("{symbol}: {last}"))
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                data.to_string()
            }
        }
        "executions" => {
            if let Some(arr) = data.as_array() {
                arr.iter()
                    .map(|item| {
                        let exec_type = item.get("exec_type").and_then(|s| s.as_str()).unwrap_or("unknown");
                        let order_id = item.get("order_id").and_then(|s| s.as_str()).unwrap_or("-");
                        match exec_type {
                            "trade" => {
                                let exec_id = item.get("exec_id").and_then(|s| s.as_str()).unwrap_or("-");
                                let symbol = item.get("symbol").and_then(|s| s.as_str()).unwrap_or("-");
                                let side = item.get("side").and_then(|s| s.as_str()).unwrap_or("-");
                                let qty = item.get("last_qty").map(|v| v.to_string()).unwrap_or_default();
                                let price = item.get("last_price").map(|v| v.to_string()).unwrap_or_default();
                                let cost = item.get("cost").map(|v| v.to_string()).unwrap_or_default();
                                format!("{exec_id} {symbol} {side} {qty}@{price} cost:{cost}")
                            }
                            "pending_new" => {
                                let symbol = item.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
                                let side = item.get("side").and_then(|s| s.as_str()).unwrap_or("");
                                let order_type = item.get("order_type").and_then(|s| s.as_str()).unwrap_or("");
                                let qty_part = if let Some(q) = item.get("order_qty").filter(|v| v.as_f64().is_some_and(|n| n > 0.0)) {
                                    format!(" qty:{q}")
                                } else if let Some(c) = item.get("cash_order_qty").filter(|v| v.as_f64().is_some_and(|n| n > 0.0)) {
                                    format!(" cash:{c}")
                                } else {
                                    String::new()
                                };
                                format!("{order_id} pending_new {symbol} {side} {order_type}{qty_part}")
                            }
                            "new" => {
                                format!("{order_id} new")
                            }
                            "filled" | "canceled" | "expired" => {
                                let status = item.get("order_status").and_then(|s| s.as_str()).unwrap_or(exec_type);
                                let cum_qty = item.get("cum_qty").map(|v| v.to_string()).unwrap_or_default();
                                let avg_price = item.get("avg_price").map(|v| v.to_string()).unwrap_or_default();
                                let cum_cost = item.get("cum_cost").map(|v| v.to_string()).unwrap_or_default();
                                format!("{order_id} {status} qty:{cum_qty} avg:{avg_price} cost:{cum_cost}")
                            }
                            "partially_filled" => {
                                let cum_qty = item.get("cum_qty").map(|v| v.to_string()).unwrap_or_default();
                                let avg_price = item.get("avg_price").map(|v| v.to_string()).unwrap_or_default();
                                let cum_cost = item.get("cum_cost").map(|v| v.to_string()).unwrap_or_default();
                                format!("{order_id} partial qty:{cum_qty} avg:{avg_price} cost:{cum_cost}")
                            }
                            _ => {
                                let status = item.get("order_status").and_then(|s| s.as_str()).unwrap_or("-");
                                format!("{order_id} {exec_type} status:{status}")
                            }
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                data.to_string()
            }
        }
        "balances" => {
            if let Some(arr) = data.as_array() {
                arr.iter()
                    .map(|item| {
                        let asset = item.get("asset").and_then(|s| s.as_str()).unwrap_or("-");
                        let balance = item.get("balance").map(|v| v.to_string()).unwrap_or_default();
                        let entry_type = item.get("type").and_then(|s| s.as_str()).unwrap_or("");
                        let amount = item.get("amount").map(|v| v.to_string()).unwrap_or_default();
                        let fee = item.get("fee").map(|v| v.to_string()).unwrap_or_default();
                        let ledger_id = item.get("ledger_id").and_then(|s| s.as_str()).unwrap_or("");
                        let wallet = item.get("wallet_type").and_then(|s| s.as_str()).unwrap_or("");
                        if !entry_type.is_empty() {
                            let fee_part = if fee != "0" && fee != "0.0" && !fee.chars().all(|c| c == '0' || c == '.' || c == '-') {
                                format!(" fee:{fee}")
                            } else {
                                String::new()
                            };
                            format!("{ledger_id} {entry_type} {asset} {amount}{fee_part} bal:{balance} ({wallet})")
                        } else {
                            format!("{asset}: {balance}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                data.to_string()
            }
        }
        "instrument" => {
            if let Some(arr) = data.as_array() {
                arr.iter()
                    .map(|item| {
                        let symbol = item.get("symbol").and_then(|s| s.as_str()).unwrap_or("-");
                        let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("-");
                        format!("{symbol}: {status}")
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                data.to_string()
            }
        }
        "trade" => {
            if let Some(arr) = data.as_array() {
                arr.iter()
                    .map(|item| {
                        let symbol = item.get("symbol").and_then(|s| s.as_str()).unwrap_or("-");
                        let side = item.get("side").and_then(|s| s.as_str()).unwrap_or("-");
                        let qty = item.get("qty").map(|v| v.to_string()).unwrap_or_default();
                        let price = item.get("price").map(|v| v.to_string()).unwrap_or_default();
                        let ord_type = item.get("ord_type").and_then(|s| s.as_str()).unwrap_or("");
                        format!("{symbol} {side} {qty}@{price} ({ord_type})")
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                data.to_string()
            }
        }
        "book" => {
            if let Some(arr) = data.as_array() {
                arr.iter()
                    .map(|item| {
                        let symbol = item.get("symbol").and_then(|s| s.as_str()).unwrap_or("-");
                        let asks = item.get("asks").and_then(|a| a.as_array());
                        let bids = item.get("bids").and_then(|b| b.as_array());
                        let ask_count = asks.map(|a| a.len()).unwrap_or(0);
                        let bid_count = bids.map(|b| b.len()).unwrap_or(0);
                        let best_ask = asks
                            .and_then(|a| a.first())
                            .and_then(|a| a.get("price"))
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "-".into());
                        let best_bid = bids
                            .and_then(|b| b.first())
                            .and_then(|b| b.get("price"))
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "-".into());
                        format!("{symbol} ask:{best_ask}({ask_count}) bid:{best_bid}({bid_count})")
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                data.to_string()
            }
        }
        "ohlc" => {
            if let Some(arr) = data.as_array() {
                arr.iter()
                    .map(|item| {
                        let symbol = item.get("symbol").and_then(|s| s.as_str()).unwrap_or("-");
                        let interval = item
                            .get("interval")
                            .map(|v| v.to_string())
                            .unwrap_or_default();
                        let open = item.get("open").map(|v| v.to_string()).unwrap_or_default();
                        let high = item.get("high").map(|v| v.to_string()).unwrap_or_default();
                        let low = item.get("low").map(|v| v.to_string()).unwrap_or_default();
                        let close = item.get("close").map(|v| v.to_string()).unwrap_or_default();
                        let volume = item
                            .get("volume")
                            .map(|v| v.to_string())
                            .unwrap_or_default();
                        format!(
                            "{symbol} {interval}m O:{open} H:{high} L:{low} C:{close} V:{volume}"
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                data.to_string()
            }
        }
        "level3" => format_l3_data(msg_type, data),
        _ => {
            let s = data.to_string();
            let truncated = crate::client::truncate(&s, 200);
            if truncated.len() < s.len() {
                format!("{truncated}...")
            } else {
                s
            }
        }
    }
}

fn format_l3_order(order: &Value) -> String {
    let id = order
        .get("order_id")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let price = order
        .get("limit_price")
        .map(|v| v.to_string())
        .unwrap_or_default();
    let qty = order
        .get("order_qty")
        .map(|v| v.to_string())
        .unwrap_or_default();
    format!("{id} {qty}@{price}")
}

fn format_l3_data(msg_type: &str, data: &Value) -> String {
    let book = match data.as_array().and_then(|a| a.first()) {
        Some(b) => b,
        None => return data.to_string(),
    };

    let symbol = book.get("symbol").and_then(|s| s.as_str()).unwrap_or("?");

    if msg_type == "snapshot" {
        let bid_count = book
            .get("bids")
            .and_then(|v| v.as_array())
            .map_or(0, |a| a.len());
        let ask_count = book
            .get("asks")
            .and_then(|v| v.as_array())
            .map_or(0, |a| a.len());
        format!("{symbol} snapshot: {bid_count} bids, {ask_count} asks")
    } else {
        let mut parts = Vec::new();
        for (side, label) in [("bids", "bid"), ("asks", "ask")] {
            if let Some(events) = book.get(side).and_then(|v| v.as_array()) {
                for ev in events {
                    let event = ev.get("event").and_then(|e| e.as_str()).unwrap_or("?");
                    let detail = format_l3_order(ev);
                    parts.push(format!("{label} {event}: {detail}"));
                }
            }
        }
        if parts.is_empty() {
            format!("{symbol} update (empty)")
        } else {
            format!("{symbol} | {}", parts.join(" | "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_handshake_includes_korigin() {
        let req = ws_handshake_request("wss://ws.kraken.com/v2").unwrap();
        let value = req
            .headers()
            .get("x-korigin")
            .expect("x-korigin header missing");
        assert_eq!(value, "u004");
    }

    #[test]
    fn build_subscribe_payload_ticker_includes_symbol() {
        let payload = build_subscribe_payload("ticker", &["BTC/USD".into()], None);
        let params = payload.get("params").unwrap();
        assert_eq!(params["channel"], "ticker");
        let symbols = params["symbol"].as_array().unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0], "BTC/USD");
    }

    #[test]
    fn build_subscribe_payload_instrument_omits_symbol_when_empty() {
        let payload = build_subscribe_payload("instrument", &[], None);
        let params = payload.get("params").unwrap();
        assert_eq!(params["channel"], "instrument");
        assert!(params.get("symbol").is_none());
    }

    #[test]
    fn build_subscribe_payload_instrument_includes_symbol_when_provided() {
        let payload = build_subscribe_payload("instrument", &["BTC/USD".into()], None);
        let params = payload.get("params").unwrap();
        assert_eq!(params["channel"], "instrument");
        let symbols = params["symbol"].as_array().unwrap();
        assert_eq!(symbols[0], "BTC/USD");
    }

    #[test]
    fn build_subscribe_payload_book_includes_extra_params() {
        let extra = serde_json::json!({"depth": 10});
        let payload = build_subscribe_payload("book", &["BTC/USD".into()], Some(&extra));
        let params = payload.get("params").unwrap();
        assert_eq!(params["channel"], "book");
        assert_eq!(params["depth"], 10);
        assert!(params.get("symbol").is_some());
    }

    #[test]
    fn redact_token_replaces_in_subscribe_payload() {
        let token = "abc123xyz";
        let payload = format!(
            r#"{{"method":"subscribe","params":{{"channel":"executions","token":"{token}"}}}}"#
        );
        let redacted = redact_token_in_payload(&payload, token);
        assert!(redacted.contains("<redacted>"));
        assert!(!redacted.contains(token));
    }

    #[test]
    fn redact_token_preserves_non_matching_payload() {
        let payload = r#"{"method":"subscribe","params":{"channel":"ticker"}}"#;
        let result = redact_token_in_payload(payload, "xyz");
        assert_eq!(result, payload);
    }

    #[test]
    fn redact_token_handles_multiple_occurrences() {
        let token = "abc123";
        let payload = format!("token:{token} again:{token}");
        let redacted = redact_token_in_payload(&payload, token);
        assert!(!redacted.contains(token));
        assert_eq!(redacted.matches("<redacted>").count(), 2);
    }

    #[test]
    fn validate_book_depth_accepts_valid() {
        for d in &[10, 25, 100, 500, 1000] {
            assert!(validate_book_depth(*d).is_ok());
        }
    }

    #[test]
    fn validate_book_depth_rejects_invalid() {
        for d in &[0, 1, 5, 50, 200, 2000] {
            assert!(validate_book_depth(*d).is_err());
        }
    }

    #[test]
    fn validate_ohlc_interval_accepts_valid() {
        for i in &[1, 5, 15, 30, 60, 240, 1440, 10080, 21600] {
            assert!(validate_ohlc_interval(*i).is_ok());
        }
    }

    #[test]
    fn validate_ohlc_interval_rejects_invalid() {
        for i in &[0, 2, 6, 7, 45, 100] {
            assert!(validate_ohlc_interval(*i).is_err());
        }
    }

    #[test]
    fn validate_event_trigger_accepts_valid() {
        assert!(validate_event_trigger("bbo").is_ok());
        assert!(validate_event_trigger("trades").is_ok());
    }

    #[test]
    fn validate_event_trigger_rejects_invalid() {
        assert!(validate_event_trigger("invalid").is_err());
        assert!(validate_event_trigger("").is_err());
    }

    #[test]
    fn build_subscribe_payload_includes_req_id_at_top_level() {
        let extra = serde_json::json!({"req_id": 42});
        let payload = build_subscribe_payload("ticker", &["BTC/USD".into()], Some(&extra));
        assert_eq!(payload["req_id"], 42);
        assert!(payload["params"].get("req_id").is_none());
    }

    #[test]
    fn build_subscribe_payload_includes_snapshot() {
        let extra = serde_json::json!({"snapshot": false});
        let payload = build_subscribe_payload("ticker", &["BTC/USD".into()], Some(&extra));
        assert_eq!(payload["params"]["snapshot"], false);
    }

    #[test]
    fn build_subscribe_payload_req_id_not_in_params() {
        let extra = serde_json::json!({"req_id": 99, "snapshot": true});
        let payload = build_subscribe_payload("book", &["ETH/USD".into()], Some(&extra));
        assert_eq!(payload["req_id"], 99);
        assert!(payload["params"].get("req_id").is_none());
        assert_eq!(payload["params"]["snapshot"], true);
    }

    // -- L3 formatter --

    #[test]
    fn l3_snapshot_shows_bid_ask_counts() {
        let data = serde_json::json!([{
            "symbol": "BTC/USD",
            "bids": [
                {"order_id": "O1", "limit_price": 50000.0, "order_qty": 1.0, "timestamp": "2023-10-06T18:20:00Z"},
                {"order_id": "O2", "limit_price": 49999.0, "order_qty": 0.5, "timestamp": "2023-10-06T18:20:01Z"}
            ],
            "asks": [
                {"order_id": "O3", "limit_price": 50001.0, "order_qty": 2.0, "timestamp": "2023-10-06T18:20:02Z"}
            ],
            "checksum": 12345,
            "timestamp": "2023-10-06T18:20:03Z"
        }]);
        let result = format_l3_data("snapshot", &data);
        assert_eq!(result, "BTC/USD snapshot: 2 bids, 1 asks");
    }

    #[test]
    fn l3_update_shows_events() {
        let data = serde_json::json!([{
            "symbol": "MATIC/USD",
            "checksum": 2841398499u64,
            "bids": [],
            "asks": [
                {"event": "delete", "order_id": "OOIATY-6EIWY-ACVIUN", "limit_price": 0.5636, "order_qty": 302.89, "timestamp": "2023-10-06T18:21:00Z"},
                {"event": "add", "order_id": "O2BN53-5RSB2-V3J57T", "limit_price": 0.564, "order_qty": 3500.77, "timestamp": "2023-10-06T18:20:27Z"}
            ]
        }]);
        let result = format_l3_data("update", &data);
        assert!(result.starts_with("MATIC/USD | "));
        assert!(result.contains("ask delete: OOIATY-6EIWY-ACVIUN"));
        assert!(result.contains("ask add: O2BN53-5RSB2-V3J57T"));
    }

    #[test]
    fn l3_update_empty_bids_and_asks() {
        let data = serde_json::json!([{
            "symbol": "ETH/USD",
            "bids": [],
            "asks": []
        }]);
        let result = format_l3_data("update", &data);
        assert_eq!(result, "ETH/USD update (empty)");
    }

    #[test]
    fn l3_update_bid_modify() {
        let data = serde_json::json!([{
            "symbol": "BTC/USD",
            "bids": [
                {"event": "modify", "order_id": "OABC-12345", "limit_price": 49500.0, "order_qty": 0.25, "timestamp": "2023-10-06T18:22:00Z"}
            ],
            "asks": []
        }]);
        let result = format_l3_data("update", &data);
        assert!(result.contains("bid modify: OABC-12345 0.25@49500"));
    }

    #[test]
    fn l3_snapshot_empty_book() {
        let data = serde_json::json!([{
            "symbol": "XRP/USD",
            "bids": [],
            "asks": []
        }]);
        let result = format_l3_data("snapshot", &data);
        assert_eq!(result, "XRP/USD snapshot: 0 bids, 0 asks");
    }

    #[test]
    fn l3_invalid_data_falls_back() {
        let data = serde_json::json!("not an array");
        let result = format_l3_data("snapshot", &data);
        assert_eq!(result, "\"not an array\"");
    }
}
