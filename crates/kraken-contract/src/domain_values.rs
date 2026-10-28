//! Argument values whose domain twin lives in a crate this one must not depend
//! on.
//!
//! `kraken-workspace` and `kraken-session` pull in the paper engine and the
//! recording backends. So the command surface declares its own vocabulary here
//! and the executing crate bridges to the domain type with `From` — the same
//! wire-type/domain-type split the rest of the codebase uses.
//!
//! Each enum's serde form is deliberately identical to its domain twin's, so a
//! proposal and a stored record spell the same value the same way. The `From`
//! impls in `kraken-cli` are exhaustive, so a variant added on either side
//! without the other fails to compile.

use clap::ValueEnum;

/// Which backend an account's trading verbs execute against.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMode {
    /// Simulated fills from the event-sourced engine, live market data.
    Paper,
    /// Real orders attributed to this workspace through the configured account.
    Live,
}

/// Where price sits relative to a strategy's band.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Zone {
    Above,
    Within,
    Below,
}

/// What a decision-log entry records.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Buy,
    Sell,
    /// A decision to pull a resting order.
    Cancel,
    /// A deliberate no-op (considered and declined to act).
    Skip,
    /// A noteworthy observation with no trade.
    Alert,
}
