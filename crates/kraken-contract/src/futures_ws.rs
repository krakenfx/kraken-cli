//! Futures WebSocket streaming commands.

use clap::{Args, Subcommand};

/// The `futures ws` family: futures WebSocket feeds, public and private.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FuturesWsCommand {
    /// Stream futures ticker updates (public).
    Ticker(Ticker),
    /// Stream futures trades (public).
    #[command(alias = "trade")]
    Trades(Trades),
    /// Stream futures order book updates (public).
    Book(Book),
    /// Stream fills (private, auth required).
    Fills(Fills),
    /// Stream open orders (private, auth required).
    OpenOrders(OpenOrders),
    /// Stream open positions (private, auth required).
    OpenPositions(OpenPositions),
    /// Stream account balances and margins (private, auth required).
    Balances(Balances),
    /// Stream notifications (private, auth required).
    Notifications(Notifications),
    /// Stream account log (private, auth required).
    AccountLog(AccountLog),
}

#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Ticker {
    /// Market symbols (e.g. PF_XBTUSD).
    #[arg(required = true, num_args = 1..)]
    pub markets: Vec<String>,
}

#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Trades {
    /// Market symbols.
    #[arg(required = true, num_args = 1..)]
    pub markets: Vec<String>,
}

#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Book {
    /// Market symbols.
    #[arg(required = true, num_args = 1..)]
    pub markets: Vec<String>,
}

// The private feeds take no arguments: the subscription is the whole request.
// Each is a struct rather than a unit variant so every leaf of the tree has a
// type the catalog can classify.
#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Fills {}

// Takes no arguments; see `Fills`.
#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct OpenOrders {}

// Takes no arguments; see `Fills`.
#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct OpenPositions {}

// Takes no arguments; see `Fills`.
#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Balances {}

// Takes no arguments; see `Fills`.
#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Notifications {}

// Takes no arguments; see `Fills`.
#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct AccountLog {}

crate::tools! {
    /// The `kraken futures ws` commands.
    fn futures_ws in ["futures", "ws"] of FuturesWsCommand {
        Ticker        as "ticker"         => FuturesWs, ReadOnly, None,    Stream
            example "kraken futures ws ticker PF_XBTUSD PF_ETHUSD";
        Trades        as "trades"         => FuturesWs, ReadOnly, None,    Stream
            example "kraken futures ws trades PF_XBTUSD";
        Book          as "book"           => FuturesWs, ReadOnly, None,    Stream
            example "kraken futures ws book PF_XBTUSD";
        Fills         as "fills"          => FuturesWs, ReadOnly, Futures, Stream
            example "kraken futures ws fills";
        OpenOrders    as "open-orders"    => FuturesWs, ReadOnly, Futures, Stream
            example "kraken futures ws open-orders";
        OpenPositions as "open-positions" => FuturesWs, ReadOnly, Futures, Stream
            example "kraken futures ws open-positions";
        Balances      as "balances"       => FuturesWs, ReadOnly, Futures, Stream
            example "kraken futures ws balances";
        Notifications as "notifications"  => FuturesWs, ReadOnly, Futures, Stream
            example "kraken futures ws notifications";
        AccountLog    as "account-log"    => FuturesWs, ReadOnly, Futures, Stream
            example "kraken futures ws account-log";
    }
}
