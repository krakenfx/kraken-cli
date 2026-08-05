/// Public market data commands — no auth required.
use clap::Subcommand;
use enum_dispatch::enum_dispatch;
use serde_json::Value;

use super::Execute;
use super::schema;
use super::value_types::{AssetClass, CliWire, GroupedDepth, Grouping};
use crate::cli::AppContext;
use crate::errors::Result;
use crate::output::CommandOutput;
use crate::output::json_field;

#[enum_dispatch(Execute)]
#[derive(Debug, Subcommand)]
pub(crate) enum MarketCommand {
    /// Get system status and trading mode.
    Status(Status),
    /// Get server time.
    ServerTime(ServerTime),
    /// Get asset info.
    Assets(Assets),
    /// Get tradable asset pairs.
    Pairs(Pairs),
    /// Get ticker information for one or more pairs.
    Ticker(Ticker),
    /// Get OHLC candle data.
    Ohlc(Ohlc),
    /// Get L2 order book.
    Orderbook(Orderbook),
    /// Get grouped order book.
    OrderbookGrouped(OrderbookGrouped),
    /// Get recent trades.
    Trades(Trades),
    /// Get recent spreads.
    Spreads(Spreads),
}

#[derive(Debug, clap::Args)]
pub(crate) struct Status {}

#[derive(Debug, clap::Args)]
pub(crate) struct ServerTime {}

#[derive(Debug, clap::Args)]
pub(crate) struct Assets {
    /// Comma-separated list of assets to query.
    #[arg(long)]
    asset: Option<String>,
    /// Asset class filter.
    #[arg(long, alias = "aclass")]
    asset_class: Option<AssetClass>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Pairs {
    /// Comma-separated pairs to query.
    #[arg(long)]
    pair: Option<String>,
    /// Info level: info, leverage, fees, margin.
    #[arg(long)]
    info: Option<String>,
    /// Asset class filter.
    #[arg(long, alias = "aclass")]
    asset_class: Option<AssetClass>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Ticker {
    /// One or more trading pairs.
    #[arg(num_args = 1..)]
    pairs: Vec<String>,
    /// Asset class filter.
    #[arg(long)]
    asset_class: Option<AssetClass>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Ohlc {
    /// Trading pair.
    pair: String,
    /// Candle interval in minutes (1, 5, 15, 30, 60, 240, 1440, 10080, 21600).
    #[arg(long, default_value = "60")]
    interval: u32,
    /// Unix timestamp to fetch data since.
    #[arg(long)]
    since: Option<String>,
    /// Asset class filter.
    #[arg(long)]
    asset_class: Option<AssetClass>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Orderbook {
    /// Trading pair.
    pair: String,
    /// Number of price levels (max 500).
    #[arg(long, default_value = "25")]
    count: u32,
    /// Asset class filter.
    #[arg(long)]
    asset_class: Option<AssetClass>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct OrderbookGrouped {
    /// Trading pair.
    pair: String,
    /// Number of price levels per side.
    #[arg(long, default_value = "10")]
    depth: GroupedDepth,
    /// Tick levels within each price level (bids rounded down, asks up).
    #[arg(long, default_value = "1")]
    grouping: Grouping,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Trades {
    /// Trading pair.
    pair: String,
    /// Fetch trades since this timestamp.
    #[arg(long)]
    since: Option<String>,
    /// Maximum number of trades.
    #[arg(long, default_value = "1000")]
    count: u32,
    /// Asset class filter.
    #[arg(long)]
    asset_class: Option<AssetClass>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Spreads {
    /// Trading pair.
    pair: String,
    /// Fetch spreads since this timestamp.
    #[arg(long)]
    since: Option<String>,
    /// Asset class filter.
    #[arg(long)]
    asset_class: Option<AssetClass>,
}

impl Execute for Status {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.spot()?;
        let data = client.public_get("SystemStatus", &[]).await?;
        let status = data
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let timestamp = data.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
        Ok(CommandOutput::key_value(
            vec![
                ("Status".into(), status.to_string()),
                ("Timestamp".into(), timestamp.to_string()),
            ],
            data,
        ))
    }
}

impl Execute for ServerTime {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.spot()?;
        // Already schema-shaped on the wire: pure passthrough.
        let data = client.public_get("Time", &[]).await?;
        let unixtime = data.get("unixtime").and_then(|v| v.as_u64()).unwrap_or(0);
        let rfc1123 = data.get("rfc1123").and_then(|v| v.as_str()).unwrap_or("");
        Ok(CommandOutput::key_value(
            vec![
                ("Unix Time".into(), unixtime.to_string()),
                ("RFC 1123".into(), rfc1123.to_string()),
            ],
            data,
        ))
    }
}

impl Execute for Assets {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.spot()?;
        let Self { asset, asset_class } = self;
        let mut params = Vec::new();
        if let Some(a) = &asset {
            params.push(("asset", a.as_str()));
        }
        let ac_wire;
        if let Some(ac) = asset_class {
            ac_wire = ac.as_wire();
            params.push(("aclass", ac_wire.as_str()));
        }
        let mut data = client.public_get("Assets", &params).await?;
        schema::assets(&mut data);
        let (headers, rows) = parse_asset_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

impl Execute for Pairs {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.spot()?;
        let Self {
            pair,
            info,
            asset_class,
        } = self;
        let mut params = Vec::new();
        if let Some(p) = &pair {
            params.push(("pair", p.as_str()));
        }
        if let Some(i) = &info {
            params.push(("info", i.as_str()));
        }
        let ac_wire;
        if let Some(ac) = asset_class {
            ac_wire = ac.as_wire();
            params.push(("aclass", ac_wire.as_str()));
        }
        let mut data = client.public_get("AssetPairs", &params).await?;
        schema::asset_pairs(&mut data);
        let (headers, rows) = parse_pairs_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

impl Execute for Ticker {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.spot()?;
        let Self { pairs, asset_class } = self;
        let pair_str = pairs.join(",");
        let mut params = vec![("pair", &*pair_str)];
        let ac_wire;
        if let Some(ac) = asset_class {
            ac_wire = ac.as_wire();
            params.push(("asset_class", ac_wire.as_str()));
        }
        let mut data = client.public_get("Ticker", &params).await?;
        schema::ticker(&mut data);
        let (headers, rows) = parse_ticker_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

impl Execute for Ohlc {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.spot()?;
        let Self {
            pair,
            interval,
            since,
            asset_class,
        } = self;
        let interval_str = interval.to_string();
        let mut params = vec![("pair", pair.as_str()), ("interval", &interval_str)];
        if let Some(s) = &since {
            params.push(("since", s.as_str()));
        }
        let ac_wire;
        if let Some(ac) = asset_class {
            ac_wire = ac.as_wire();
            params.push(("asset_class", ac_wire.as_str()));
        }
        let mut data = client.public_get("OHLC", &params).await?;
        schema::ohlc(&mut data, &pair);
        let (headers, rows) = parse_ohlc_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

impl Execute for Orderbook {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.spot()?;
        let Self {
            pair,
            count,
            asset_class,
        } = self;
        let count_str = count.to_string();
        let mut params = vec![("pair", pair.as_str()), ("count", &count_str)];
        let ac_wire;
        if let Some(ac) = asset_class {
            ac_wire = ac.as_wire();
            params.push(("asset_class", ac_wire.as_str()));
        }
        let mut data = client.public_get("Depth", &params).await?;
        schema::orderbook(&mut data);
        let (headers, rows) = parse_orderbook_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

impl Execute for OrderbookGrouped {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.spot()?;
        let Self {
            pair,
            depth,
            grouping,
        } = self;
        let depth_wire = depth.as_wire();
        let grouping_wire = grouping.as_wire();
        let mut params = vec![("pair", pair.as_str()), ("depth", depth_wire.as_str())];
        if grouping != Grouping::G1 {
            params.push(("grouping", grouping_wire.as_str()));
        }
        let data = client.public_get("GroupedBook", &params).await?;
        Ok(CommandOutput::new(
            data,
            vec!["Data".into()],
            vec![vec![
                "Grouped orderbook data returned (see JSON output)".into(),
            ]],
        ))
    }
}

impl Execute for Trades {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.spot()?;
        let Self {
            pair,
            since,
            count,
            asset_class,
        } = self;
        let count_str = count.to_string();
        let mut params = vec![("pair", pair.as_str()), ("count", &count_str)];
        if let Some(s) = &since {
            params.push(("since", s.as_str()));
        }
        let ac_wire;
        if let Some(ac) = asset_class {
            ac_wire = ac.as_wire();
            params.push(("asset_class", ac_wire.as_str()));
        }
        let mut data = client.public_get("Trades", &params).await?;
        schema::trades(&mut data, &pair);
        let (headers, rows) = parse_trades_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

impl Execute for Spreads {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.spot()?;
        let Self {
            pair,
            since,
            asset_class,
        } = self;
        let mut params = vec![("pair", pair.as_str())];
        if let Some(s) = &since {
            params.push(("since", s.as_str()));
        }
        let ac_wire;
        if let Some(ac) = asset_class {
            ac_wire = ac.as_wire();
            params.push(("asset_class", ac_wire.as_str()));
        }
        let mut data = client.public_get("Spread", &params).await?;
        schema::spreads(&mut data, &pair);
        let (headers, rows) = parse_spreads_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

fn parse_asset_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec![
        "Asset".into(),
        "Altname".into(),
        "Decimals".into(),
        "Display Decimals".into(),
        "Status".into(),
    ];
    let mut rows = Vec::new();
    if let Some(obj) = data.as_object() {
        for (key, val) in obj {
            rows.push(vec![
                key.clone(),
                json_field(val, "altname"),
                json_field(val, "decimals"),
                json_field(val, "display_decimals"),
                json_field(val, "status"),
            ]);
        }
    }
    rows.sort_by(|a, b| a[0].cmp(&b[0]));
    (headers, rows)
}

fn parse_pairs_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec![
        "Pair".into(),
        "Status".into(),
        "Tick Size".into(),
        "Order Min".into(),
        "Cost Min".into(),
    ];
    let mut rows = Vec::new();
    if let Some(obj) = data.as_object() {
        for (key, val) in obj {
            rows.push(vec![
                key.clone(),
                json_field(val, "status"),
                json_field(val, "tick_size"),
                json_field(val, "ordermin"),
                json_field(val, "costmin"),
            ]);
        }
    }
    rows.sort_by(|a, b| a[0].cmp(&b[0]));
    (headers, rows)
}

fn parse_ticker_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec![
        "Pair".into(),
        "Ask".into(),
        "Bid".into(),
        "Last".into(),
        "Volume (24h)".into(),
        "High (24h)".into(),
        "Low (24h)".into(),
    ];
    let mut rows = Vec::new();
    if let Some(obj) = data.as_object() {
        for (key, val) in obj {
            rows.push(vec![
                key.clone(),
                json_field(val, "ask_price"),
                json_field(val, "bid_price"),
                json_field(val, "last_price"),
                json_field(val, "volume_24h"),
                json_field(val, "high_24h"),
                json_field(val, "low_24h"),
            ]);
        }
    }
    (headers, rows)
}

fn parse_ohlc_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec![
        "Time".into(),
        "Open".into(),
        "High".into(),
        "Low".into(),
        "Close".into(),
        "Volume".into(),
    ];
    let mut rows = Vec::new();
    if let Some(Value::Array(candles)) = data.get("candles") {
        for candle in candles {
            rows.push(vec![
                json_field(candle, "time"),
                json_field(candle, "open"),
                json_field(candle, "high"),
                json_field(candle, "low"),
                json_field(candle, "close"),
                json_field(candle, "volume"),
            ]);
        }
    }
    (headers, rows)
}

/// Table shows top-of-book only (10 levels a side); the JSON payload carries
/// the full requested depth.
fn parse_orderbook_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    const TABLE_LEVELS: usize = 10;
    let headers = vec!["Side".into(), "Price".into(), "Volume".into()];
    let mut rows = Vec::new();
    for (side, label) in [("asks", "ASK"), ("bids", "BID")] {
        if let Some(Value::Array(levels)) = data.get(side) {
            for level in levels.iter().take(TABLE_LEVELS) {
                rows.push(vec![
                    label.into(),
                    json_field(level, "price"),
                    json_field(level, "volume"),
                ]);
            }
        }
    }
    (headers, rows)
}

/// Table shows the newest 20 trades (Kraken returns oldest-first); the JSON
/// payload carries the full result plus the `last` pagination cursor.
fn parse_trades_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    const TABLE_TRADES: usize = 20;
    let headers = vec![
        "Price".into(),
        "Volume".into(),
        "Time".into(),
        "Side".into(),
        "Type".into(),
    ];
    let mut rows = Vec::new();
    if let Some(Value::Array(trades)) = data.get("trades") {
        for trade in trades.iter().rev().take(TABLE_TRADES) {
            // JSON keeps the raw wire token (`l`/`m`); the human table
            // spells it out.
            let order_type = match trade.get("order_type").and_then(Value::as_str) {
                Some("l") => "limit".into(),
                Some("m") => "market".into(),
                _ => json_field(trade, "order_type"),
            };
            rows.push(vec![
                json_field(trade, "price"),
                json_field(trade, "volume"),
                json_field(trade, "time"),
                json_field(trade, "side"),
                order_type,
            ]);
        }
    }
    (headers, rows)
}

fn parse_spreads_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec!["Time".into(), "Bid".into(), "Ask".into()];
    let mut rows = Vec::new();
    if let Some(Value::Array(spreads)) = data.get("spreads") {
        for spread in spreads.iter().rev().take(20) {
            rows.push(vec![
                json_field(spread, "time"),
                json_field(spread, "bid"),
                json_field(spread, "ask"),
            ]);
        }
    }
    (headers, rows)
}
