//! Streaming channel args → their [`WsSubscription`], infallible: clap pins every
//! per-flag bound, so the conversion cannot fail.
//!
//! Each `From` impl destructures without `..`, so a newly added flag won't compile until
//! it is mapped onto the wire type.
//!
//! The symbol-scoped channels require at least one pair: an empty list would send no
//! subscribe frame and idle forever — the server answers an empty-symbol subscribe
//! with silence, not a rejection. `instrument` differs by design: no pairs means the
//! whole catalogue.

use clap::Args;
use kraken_core::{BookDepth, EventTrigger, Level3Depth, OhlcInterval, Users, WsSubscription};

#[derive(Debug, Args)]
pub(crate) struct Ticker {
    /// Trading pairs.
    #[arg(required = true, num_args = 1..)]
    pairs: Vec<String>,
    /// Event trigger: trades (default) or bbo.
    #[arg(long)]
    event_trigger: Option<EventTrigger>,
    /// Request a snapshot on subscribe (default: true).
    #[arg(long)]
    snapshot: Option<bool>,
}

impl From<Ticker> for WsSubscription {
    fn from(args: Ticker) -> Self {
        let Ticker {
            pairs,
            event_trigger,
            snapshot,
        } = args;
        WsSubscription::Ticker {
            symbol: pairs,
            event_trigger,
            snapshot,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct Trades {
    /// Trading pairs.
    #[arg(required = true, num_args = 1..)]
    pairs: Vec<String>,
    /// Request a snapshot on subscribe (default: true).
    #[arg(long)]
    snapshot: Option<bool>,
}

impl From<Trades> for WsSubscription {
    fn from(args: Trades) -> Self {
        let Trades { pairs, snapshot } = args;
        WsSubscription::Trade {
            symbol: pairs,
            snapshot,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct Book {
    /// Trading pairs.
    #[arg(required = true, num_args = 1..)]
    pairs: Vec<String>,
    /// Book depth (10, 25, 100, 500, 1000).
    #[arg(long, default_value = "10")]
    depth: BookDepth,
    /// Request a snapshot on subscribe (default: true).
    #[arg(long)]
    snapshot: Option<bool>,
}

impl From<Book> for WsSubscription {
    fn from(args: Book) -> Self {
        let Book {
            pairs,
            depth,
            snapshot,
        } = args;
        WsSubscription::Book {
            symbol: pairs,
            depth,
            snapshot,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct Ohlc {
    /// Trading pairs.
    #[arg(required = true, num_args = 1..)]
    pairs: Vec<String>,
    /// Candle interval in minutes (1, 5, 15, 30, 60, 240, 1440, 10080, 21600).
    #[arg(long, default_value = "1")]
    interval: OhlcInterval,
    /// Request a snapshot on subscribe (default: true).
    #[arg(long)]
    snapshot: Option<bool>,
}

impl From<Ohlc> for WsSubscription {
    fn from(args: Ohlc) -> Self {
        let Ohlc {
            pairs,
            interval,
            snapshot,
        } = args;
        WsSubscription::Ohlc {
            symbol: pairs,
            interval,
            snapshot,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct Instrument {
    /// Trading pairs (omit for all instruments).
    #[arg(num_args = 0..)]
    pairs: Vec<String>,
    /// Request a snapshot on subscribe (default: true).
    #[arg(long)]
    snapshot: Option<bool>,
}

impl From<Instrument> for WsSubscription {
    fn from(args: Instrument) -> Self {
        let Instrument { pairs, snapshot } = args;
        WsSubscription::Instrument {
            symbol: pairs,
            snapshot,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct Executions {
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
    users: Option<Users>,
}

impl From<Executions> for WsSubscription {
    fn from(args: Executions) -> Self {
        let Executions {
            snap_trades,
            snap_orders,
            order_status,
            rebased,
            ratecounter,
            users,
        } = args;
        WsSubscription::Executions {
            snap_trades,
            snap_orders,
            order_status,
            rebased,
            ratecounter,
            users,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct Balances {
    /// Request a snapshot on subscribe (default: true).
    #[arg(long)]
    snapshot: Option<bool>,
    /// Display xstocks in terms of underlying equity (default: true).
    #[arg(long)]
    rebased: Option<bool>,
    /// Stream events for master and subaccounts (pass "all").
    #[arg(long)]
    users: Option<Users>,
}

impl From<Balances> for WsSubscription {
    fn from(args: Balances) -> Self {
        let Balances {
            snapshot,
            rebased,
            users,
        } = args;
        WsSubscription::Balances {
            snapshot,
            rebased,
            users,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct Level3 {
    /// Trading pairs.
    #[arg(required = true, num_args = 1..)]
    pairs: Vec<String>,
    /// Number of price levels per side (10, 100, 1000).
    #[arg(long)]
    depth: Option<Level3Depth>,
    /// Request a snapshot on subscribe (default: true).
    #[arg(long)]
    snapshot: Option<bool>,
}

impl From<Level3> for WsSubscription {
    fn from(args: Level3) -> Self {
        let Level3 {
            pairs,
            depth,
            snapshot,
        } = args;
        WsSubscription::Level3 {
            symbol: pairs,
            depth,
            snapshot,
        }
    }
}
