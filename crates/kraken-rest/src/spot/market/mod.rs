//! Public market data: no key, unsigned GET.

mod asset_pairs;
mod assets;
mod depth;
mod grouped_book;
mod ohlc;
mod pages;
mod spread;
mod status;
mod ticker;
mod time;
mod trades;

pub use asset_pairs::{AssetPair, AssetPairs, AssetPairsQuery, FeeTier, PairInfo, PairStatus};
pub use assets::{AssetInfo, AssetStatus, Assets, AssetsQuery};
pub use depth::{Depth, DepthQuery, Level};
pub use grouped_book::{GroupedBook, GroupedBookQuery, GroupedLevel};
pub use ohlc::{Candle, Ohlc, OhlcQuery};
pub use spread::{Spread, Spreads, SpreadsQuery};
pub use status::{
    Emergency, Impact, IncidentStatus, Maintenance, MaintenancePhase, NextStep, NextStepKind,
    OrderSubmission, RecommendedAction, Service, SystemStatus, SystemStatusQuery, TradingStatus,
};
pub use ticker::{Ticker, TickerQuery, Tickers};
pub use time::{ServerTime, ServerTimeQuery};
pub use trades::{Trade, TradeOrderType, Trades, TradesQuery};
