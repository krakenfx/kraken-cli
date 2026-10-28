//! Scalar value objects for order requests — the typed enums that parameterize
//! `add_order` / `amend_order` / `batch_add`. The `executions` channel echoes
//! [`PriceType`] and [`TriggerReference`] back with the same tokens, so the
//! subscribe side decodes with these rather than duplicate enums.
//!
//! Source: the enum value sets are the ones the `add_order` params accept
//! (<https://docs.kraken.com/api/docs/websocket-v2/add_order>).
//!
//! Each serializes to its exact wire token (`serde`) and parses the same token
//! case-insensitively (`strum`), so an invalid value is rejected at the command
//! boundary rather than sent on the wire. Primitives shared with the subscribe
//! side ([`OrderSide`](crate::OrderSide), [`OrderType`](crate::OrderType)) live
//! in [`crate`].

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

/// How a price is interpreted (absolute, percentage, or quote-relative).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum PriceType {
    Static,
    Pct,
    Quote,
}

/// Which reference price a trigger watches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum TriggerReference {
    Index,
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum TimeInForce {
    Gtc,
    Gtd,
    Ioc,
    Fok,
}

/// Self-trade-prevention mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case", ascii_case_insensitive)]
#[allow(clippy::enum_variant_names)]
pub enum StpType {
    CancelNewest,
    CancelOldest,
    CancelBoth,
}

/// Fee currency preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum FeePreference {
    Base,
    Quote,
}
