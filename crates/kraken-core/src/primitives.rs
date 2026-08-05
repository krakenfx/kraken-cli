//! The order primitives shared by the request and subscribe sides.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

/// Order side. Shared by order requests and the `trade` channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum OrderSide {
    Buy,
    Sell,
}

/// Order type — the ten execution models the `add_order` schema accepts
/// (<https://docs.kraken.com/api/docs/websocket-v2/add_order>). Also decodes the `trade`
/// channel's taker `ord_type`, where only `limit`/`market` occur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case", ascii_case_insensitive)]
pub enum OrderType {
    Limit,
    Market,
    Iceberg,
    StopLoss,
    StopLossLimit,
    TakeProfit,
    TakeProfitLimit,
    TrailingStop,
    TrailingStopLimit,
    SettlePosition,
}
