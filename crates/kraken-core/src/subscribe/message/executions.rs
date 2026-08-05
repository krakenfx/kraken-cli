//! The `executions` channel: order status and execution events for the account
//! (the v2 successor of the v1 `openOrders` + `ownTrades` channels).
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/executions>. The fields an
//! entry carries depend on its `exec_type` (`pending_new`, `new`, `trade`, `filled`,
//! `canceled`, …), so only the identity trio — `order_id`, `exec_type`, `timestamp` —
//! is required and everything conditional is an `Option`.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use strum::Display;

use crate::{OrderSide, OrderType, PriceType, TimeInForce, TriggerReference};

/// This channel emits `time_in_force` UPPERCASE (`"GTC"`), unlike the lowercase
/// request-side tokens. Decode case-insensitively into the shared enum and re-emit the
/// channel's own casing, so a round-trip is byte-stable.
mod tif_channel_case {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::TimeInForce;

    pub(super) fn serialize<S: Serializer>(
        value: &Option<TimeInForce>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(tif) => serializer.serialize_str(&tif.to_string().to_uppercase()),
            None => serializer.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<TimeInForce>, D::Error> {
        let token = Option::<String>::deserialize(deserializer)?;
        token
            .map(|t| t.parse::<TimeInForce>().map_err(D::Error::custom))
            .transpose()
    }
}

/// One execution report (see the module schema).
///
/// No `deny_unknown_fields`: an extensible inbound payload — a field Kraken adds later is
/// ignored rather than failing the decode, which would drop every frame on the channel.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionData {
    pub order_id: String,
    pub exec_type: ExecType,
    pub timestamp: String,
    pub exec_id: Option<String>,
    pub order_status: Option<OrderStatus>,
    pub order_userref: Option<i32>,
    pub cl_ord_id: Option<String>,
    pub symbol: Option<String>,
    pub side: Option<OrderSide>,
    pub order_type: Option<OrderType>,
    pub order_qty: Option<Decimal>,
    pub cash_order_qty: Option<Decimal>,
    pub display_qty: Option<Decimal>,
    pub display_qty_remain: Option<Decimal>,
    pub limit_price: Option<Decimal>,
    pub limit_price_type: Option<PriceType>,
    #[serde(default, with = "tif_channel_case")]
    pub time_in_force: Option<TimeInForce>,
    pub effective_time: Option<String>,
    pub expire_time: Option<String>,
    pub last_qty: Option<Decimal>,
    pub last_price: Option<Decimal>,
    pub cost: Option<Decimal>,
    pub cum_qty: Option<Decimal>,
    pub cum_cost: Option<Decimal>,
    pub avg_price: Option<Decimal>,
    pub liquidity_ind: Option<LiquidityIndicator>,
    pub trade_id: Option<u64>,
    pub fees: Option<Vec<Fee>>,
    pub fee_usd_equiv: Option<Decimal>,
    pub fee_ccy_pref: Option<FeeCurrencyPreference>,
    pub liquidated: Option<bool>,
    pub margin: Option<bool>,
    pub margin_borrow: Option<bool>,
    pub no_mpp: Option<bool>,
    pub post_only: Option<bool>,
    pub reduce_only: Option<bool>,
    pub position_status: Option<PositionStatus>,
    pub amended: Option<bool>,
    pub reason: Option<String>,
    pub ord_ref_id: Option<String>,
    pub ext_ord_id: Option<String>,
    pub ext_exec_id: Option<String>,
    pub sender_sub_id: Option<String>,
    pub user: Option<String>,
    pub contingent: Option<ContingentOrder>,
    pub triggers: Option<TriggerStatus>,
    /// Deprecated on the wire (use `triggers`); still sent, so still decoded.
    pub stop_price: Option<Decimal>,
    /// Deprecated on the wire (use `triggers`); still sent, so still decoded.
    pub trigger: Option<TriggerReference>,
    /// Deprecated on the wire (use `triggers`); still sent, so still decoded.
    pub triggered_price: Option<Decimal>,
    /// Deprecated on the wire (use `reason`); still sent, so still decoded.
    pub cancel_reason: Option<String>,
    /// Wire fields outside the pinned schema, preserved verbatim (see [`ExtraFields`](super::ExtraFields)).
    #[serde(flatten)]
    pub extra: super::ExtraFields,
}

impl ExecutionData {
    /// Whether any channel-local enum field decoded through its `Unknown` fallback.
    /// Shared request-side vocabularies (`side`, `order_type`, `time_in_force`, …)
    /// stay strict and are deliberately not walked here.
    pub(crate) fn has_unknown_vocabulary(&self) -> bool {
        self.exec_type == ExecType::Unknown
            || self.order_status == Some(OrderStatus::Unknown)
            || self.liquidity_ind == Some(LiquidityIndicator::Unknown)
            || self.fee_ccy_pref == Some(FeeCurrencyPreference::Unknown)
            || self.position_status == Some(PositionStatus::Unknown)
            || self
                .triggers
                .as_ref()
                .is_some_and(|t| t.status == Some(TriggerState::Unknown))
    }
}

/// One fee paid on a trade event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fee {
    pub asset: String,
    pub qty: Decimal,
    /// Wire fields outside the pinned schema, preserved verbatim (see [`ExtraFields`](super::ExtraFields)).
    #[serde(flatten)]
    pub extra: super::ExtraFields,
}

/// Fee currency preference as this channel spells it — the REST-era flags (`fcib` =
/// fee in base, `fciq` = fee in quote), not the request side's `base`/`quote`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
#[non_exhaustive]
pub enum FeeCurrencyPreference {
    Fcib,
    Fciq,
    /// Vocabulary Kraken adds later; re-serializes as `"unknown"`, not the original token.
    #[serde(other)]
    Unknown,
}

/// The template for the secondary close orders a primary fill spawns.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContingentOrder {
    /// The docs list only the trigger-capable subset of [`OrderType`] here; decoding
    /// with the full enum keeps one order-type vocabulary.
    pub order_type: Option<OrderType>,
    pub trigger_price: Option<Decimal>,
    pub trigger_price_type: Option<PriceType>,
    pub limit_price: Option<Decimal>,
    pub limit_price_type: Option<PriceType>,
    /// Wire fields outside the pinned schema, preserved verbatim (see [`ExtraFields`](super::ExtraFields)).
    #[serde(flatten)]
    pub extra: super::ExtraFields,
}

/// The parameters and live state of a triggered order type's price trigger.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerStatus {
    pub reference: Option<TriggerReference>,
    pub price: Option<Decimal>,
    pub price_type: Option<PriceType>,
    pub actual_price: Option<Decimal>,
    pub peak_price: Option<Decimal>,
    pub last_price: Option<Decimal>,
    pub status: Option<TriggerState>,
    pub timestamp: Option<String>,
    /// Wire fields outside the pinned schema, preserved verbatim (see [`ExtraFields`](super::ExtraFields)).
    #[serde(flatten)]
    pub extra: super::ExtraFields,
}

/// The lifecycle event an execution report describes; decides which conditional
/// fields are present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[non_exhaustive]
pub enum ExecType {
    PendingNew,
    New,
    Trade,
    Filled,
    IcebergRefill,
    Canceled,
    Expired,
    Amended,
    Restated,
    Status,
    /// Vocabulary Kraken adds later; re-serializes as `"unknown"`, not the original token.
    #[serde(other)]
    Unknown,
}

/// The order's state after the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[non_exhaustive]
pub enum OrderStatus {
    PendingNew,
    New,
    PartiallyFilled,
    Filled,
    Canceled,
    Expired,
    /// Vocabulary Kraken adds later; re-serializes as `"unknown"`, not the original token.
    #[serde(other)]
    Unknown,
}

/// Whether the fill added (maker) or removed (taker) book liquidity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum LiquidityIndicator {
    #[serde(rename = "m")]
    Maker,
    #[serde(rename = "t")]
    Taker,
    /// Vocabulary Kraken adds later; re-serializes as `"unknown"`, not the original token.
    #[serde(other, rename = "unknown")]
    Unknown,
}

/// The margin position's state on a margin order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum PositionStatus {
    Opened,
    Closing,
    Closed,
    /// Vocabulary Kraken adds later; re-serializes as `"unknown"`, not the original token.
    #[serde(other)]
    Unknown,
}

/// Whether the price trigger has fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum TriggerState {
    Triggered,
    Untriggered,
    /// Vocabulary Kraken adds later; re-serializes as `"unknown"`, not the original token.
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_in_force_round_trips_the_channels_uppercase_casing() {
        let entry: ExecutionData = serde_json::from_str(
            r#"{"order_id":"O1","exec_type":"new","timestamp":"TS","time_in_force":"GTC"}"#,
        )
        .expect("the channel's UPPERCASE token decodes into the shared enum");
        assert_eq!(entry.time_in_force, Some(TimeInForce::Gtc));
        let out = serde_json::to_value(&entry).unwrap();
        assert_eq!(out["time_in_force"], "GTC", "re-emits the channel's casing");
    }

    #[test]
    fn an_unknown_time_in_force_token_is_a_hard_error() {
        let result = serde_json::from_str::<ExecutionData>(
            r#"{"order_id":"O1","exec_type":"new","timestamp":"TS","time_in_force":"WEEKLY"}"#,
        );
        assert!(result.is_err(), "vocabulary drift must fail loud");
    }

    #[test]
    fn an_unknown_exec_type_token_falls_back_to_unknown() {
        let entry: ExecutionData =
            serde_json::from_str(r#"{"order_id":"O1","exec_type":"settled","timestamp":"TS"}"#)
                .expect("a token outside the pinned vocabulary degrades the field, not the entry");
        assert_eq!(entry.exec_type, ExecType::Unknown);
        let out = serde_json::to_value(&entry).unwrap();
        assert_eq!(
            out["exec_type"], "unknown",
            "the original token is not preserved"
        );
    }
}
