//! `kraken market`.

use crate::value_types::{AssetClass, GroupedDepth, Grouping};
use clap::Subcommand;

/// Below one second, `--watch` degenerates into hammering the venue; clap
/// rejects it at parse time.
const WATCH_FLOOR_SECS: u64 = 1;

/// The `market` family: public market data. Needs no credentials, which is
/// why it is flattened to the root rather than nested behind a verb.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum MarketCommand {
    /// Get system status and trading mode.
    Status(Status),
    /// Get server time.
    ServerTime(ServerTime),
    /// Get asset info.
    Assets(Assets),
    /// Get tradable asset pairs.
    Pairs(Pairs),
    /// Get ticker information for one or more pairs; asset classes may be
    /// mixed.
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

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Status {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ServerTime {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Assets {
    /// Comma-separated list of assets to query.
    #[arg(long)]
    pub asset: Option<String>,
    /// Asset class filter.
    #[arg(long, alias = "aclass")]
    pub asset_class: Option<AssetClass>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Pairs {
    /// Comma-separated pairs to query.
    #[arg(long)]
    pub pair: Option<String>,
    /// Info level: info, leverage, fees, margin.
    #[arg(long)]
    pub info: Option<String>,
    /// Asset class filter.
    #[arg(long, alias = "aclass")]
    pub asset_class: Option<AssetClass>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Ticker {
    /// One or more trading pairs.
    #[arg(num_args = 1.., required_unless_present = "all")]
    #[serde(default)]
    pub pairs: Vec<String>,
    /// Every tradable pair (currency + tokenized_asset; forex pairs are not
    /// enumerable upstream and need explicit pairs).
    #[arg(long, conflicts_with = "pairs")]
    #[serde(default)]
    pub all: bool,
    /// Asset class filter.
    #[arg(long)]
    pub asset_class: Option<AssetClass>,
    /// Re-fetch every N seconds, emitting one snapshot per tick (NDJSON in
    /// JSON mode) until SIGINT/SIGTERM.
    ///
    /// Kept out of the tool surface: a request/response call has nowhere to put
    /// a stream, so the executor refuses this shape — advertising the
    /// argument would only offer an agent a call that always fails. `serde`
    /// still carries it, so `ticker --watch` stays legible to anything
    /// judging the command.
    #[arg(long, value_parser = clap::value_parser!(u64).range(WATCH_FLOOR_SECS..))]
    #[schemars(skip)]
    pub watch: Option<u64>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Ohlc {
    /// Trading pair.
    pub pair: String,
    /// Candle interval in minutes (1, 5, 15, 30, 60, 240, 1440, 10080, 21600).
    #[arg(long, default_value = "60")]
    #[serde(default = "hourly_candles")]
    pub interval: u32,
    /// Unix timestamp to fetch data since.
    #[arg(long)]
    pub since: Option<String>,
    /// Asset class filter.
    #[arg(long)]
    pub asset_class: Option<AssetClass>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Orderbook {
    /// Trading pair.
    pub pair: String,
    /// Number of price levels (max 500).
    #[arg(long, default_value = "25")]
    #[serde(default = "book_levels")]
    pub count: u32,
    /// Asset class filter.
    #[arg(long)]
    pub asset_class: Option<AssetClass>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct OrderbookGrouped {
    /// Trading pair.
    pub pair: String,
    /// Number of price levels per side.
    #[arg(long, default_value = "10")]
    #[serde(default = "ten_levels")]
    pub depth: GroupedDepth,
    /// Tick levels within each price level (bids rounded down, asks up).
    #[arg(long, default_value = "1")]
    #[serde(default = "finest_grouping")]
    pub grouping: Grouping,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Trades {
    /// Trading pair.
    pub pair: String,
    /// Fetch trades since this timestamp.
    #[arg(long)]
    pub since: Option<String>,
    /// Maximum number of trades.
    #[arg(long, default_value = "1000")]
    #[serde(default = "trade_page")]
    pub count: u32,
    /// Asset class filter.
    #[arg(long)]
    pub asset_class: Option<AssetClass>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Spreads {
    /// Trading pair.
    pub pair: String,
    /// Fetch spreads since this timestamp.
    #[arg(long)]
    pub since: Option<String>,
    /// Asset class filter.
    #[arg(long)]
    pub asset_class: Option<AssetClass>,
}

/// The candle interval, in minutes, when the caller names none.
fn hourly_candles() -> u32 {
    60
}

/// Book levels per side when the caller names none.
fn book_levels() -> u32 {
    25
}

/// Trades per page when the caller names none.
fn trade_page() -> u32 {
    1000
}

/// Grouped-book depth when the caller names none.
fn ten_levels() -> GroupedDepth {
    GroupedDepth::D10
}

/// Price grouping when the caller names none: none at all.
fn finest_grouping() -> Grouping {
    Grouping::G1
}

crate::tools! {
    /// The `kraken market` commands.
    fn market in ["market"] of MarketCommand {
        Status           as "status"            => Market, ReadOnly, None, Tool
            example "kraken status -o json";
        ServerTime       as "server-time"       => Market, ReadOnly, None, Tool
            example "kraken server-time";
        Assets           as "assets"            => Market, ReadOnly, None, Tool
            example "kraken assets --asset BTC,ETH -o json";
        Pairs            as "pairs"             => Market, ReadOnly, None, Tool
            example "kraken pairs --pair BTC/USD,ETH/USD -o json";
        Ticker           as "ticker"            => Market, ReadOnly, None, Tool
            where ExactlyOne(&["pairs", "all"])
            example "kraken ticker BTC/USD ETH/USD -o json";
        Ohlc             as "ohlc"              => Market, ReadOnly, None, Tool
            example "kraken ohlc BTC/USD --interval 60 -o json";
        Orderbook        as "orderbook"         => Market, ReadOnly, None, Tool
            example "kraken orderbook BTC/USD --count 25";
        OrderbookGrouped as "orderbook-grouped" => Market, ReadOnly, None, Tool
            example "kraken orderbook-grouped BTC/USD --depth 10 --grouping 100";
        Trades           as "trades"            => Market, ReadOnly, None, Tool
            example "kraken trades BTC/USD --count 50";
        Spreads          as "spreads"           => Market, ReadOnly, None, Tool
            example "kraken spreads BTC/USD";
    }
}
