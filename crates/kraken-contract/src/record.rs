//! `kraken record`.

use crate::{
    subscription::{CHANNEL_OPTIONS_NOTE, SubscriptionOptions},
    value_types::SinkTarget,
};
use clap::Args;
use kraken_core::SubscribableChannel;

#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[command(after_help = CHANNEL_OPTIONS_NOTE)]
pub struct RecordCommand {
    /// Comma-separated trading pairs to record (e.g. BTC/USD,ETH/USD);
    /// repeatable.
    #[arg(long, value_delimiter = ',')]
    #[serde(default)]
    pub symbols: Vec<String>,
    /// Comma-separated channels to capture (e.g. trades,book,ohlc); repeatable.
    #[arg(long, value_delimiter = ',', ignore_case = true)]
    #[serde(default)]
    pub channels: Vec<SubscribableChannel>,
    /// Per-channel subscribe options, shared with `kraken streamd`.
    #[command(flatten)]
    #[serde(flatten)]
    pub options: SubscriptionOptions,
    /// Sink targets: any combination of duckdb, jsonl, stdout
    /// (comma-separated).
    ///
    /// `duckdb`/`jsonl` always write their native on-disk format;
    /// `-o`/`--output` (table|json) controls only the `stdout` echo, never
    /// the durable backends.
    #[arg(long, value_enum, value_delimiter = ',', default_value = "jsonl")]
    #[serde(default = "jsonl_sink")]
    pub to: Vec<SinkTarget>,
    /// Tape name — names the single tape file all symbols are recorded into.
    /// (`--dataset` remains as a compatibility alias.)
    #[arg(long, alias = "dataset", default_value = "default")]
    #[serde(default = "default_tape")]
    pub tape: String,
}

/// Where a recording lands when the caller names no sink: the one backend every
/// build can write. `duckdb` needs the `record-duckdb` feature, which is off by
/// default, so it cannot be what a caller gets without asking for it.
fn jsonl_sink() -> Vec<SinkTarget> {
    vec![SinkTarget::Jsonl]
}

/// The tape a recording appends to when the caller names none.
fn default_tape() -> String {
    "default".to_owned()
}
