//! Account data: balances, orders, fills, positions, ledger, fees, exports.
//! Every endpoint is a signed read except the export lifecycle.

mod balance;
mod credit_lines;
mod export;
mod ledger;
mod level3;
mod orders;
mod positions;
mod trade_balance;
mod trade_volume;
mod trades;

pub use balance::{
    BalanceQuery, Balances, ExtendedBalance, ExtendedBalanceQuery, ExtendedBalances,
};
pub use credit_lines::{CreditLimits, CreditLineAsset, CreditLines, CreditLinesQuery};
pub use export::{
    AddExport, ExportCreated, ExportFormat, ExportProgress, ExportRemoved, ExportReport,
    ExportReportKind, ExportStatus, ExportStatusQuery, Removal, RemoveExport, RetrieveExport,
};
pub use ledger::{Ledger, LedgerEntry, LedgerQuery, QueriedLedger, QueryLedgerQuery};
pub use level3::{Level3, Level3Order, Level3Query};
pub use orders::{
    CloseTime, ClosedOrders, ClosedOrdersQuery, OpenOrders, OpenOrdersQuery, Order,
    OrderDescription, QueriedOrders, QueryOrdersQuery,
};
pub use positions::{ConsolidatedPosition, OpenPositionsQuery, Position, Positions};
use serde::Serialize;
pub use trade_balance::{TradeBalance, TradeBalanceQuery};
pub use trade_volume::{FeeTierStatus, TradeVolume, TradeVolumeInputs, TradeVolumeQuery};
pub use trades::{
    Fill, QueriedFills, QueryTradesQuery, TradeFilter, TradesHistory, TradesHistoryQuery,
};

/// A page cursor and the next page's id, when Kraken paginates by cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Cursor {
    pub next: Option<String>,
}

/// A history window bound: a Unix time, or the id of a record to start or end
/// at. Both reach Kraken under the same parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Bound {
    Time(i64),
    Id(String),
}

impl From<String> for Bound {
    /// A bare integer is a time; anything else names a record.
    fn from(bound: String) -> Self {
        bound.parse().map_or(Self::Id(bound), Self::Time)
    }
}

/// `start`/`end`/`ofs`/`without_count`, shared by the history endpoints.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct Window {
    pub start: Option<Bound>,
    pub end: Option<Bound>,
    /// Result offset.
    pub ofs: Option<u64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub without_count: bool,
}
