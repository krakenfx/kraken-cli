//! `kraken streamd`: one long-lived process that follows many channels across

use crate::subscription::{CHANNEL_OPTIONS_NOTE, SubscriptionOptions};
use clap::Subcommand;
use kraken_core::SubscribableChannel;

/// The `streamd` family: one long-lived stream across many channels and
/// symbols.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum StreamdCommand {
    /// Start a long-lived multi-channel, multi-symbol stream.
    #[command(after_help = CHANNEL_OPTIONS_NOTE)]
    Start(Start),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Start {
    /// Comma-separated trading pairs (e.g. BTC/USD,ETH/USD); repeatable.
    #[arg(long, value_delimiter = ',')]
    #[serde(default)]
    pub symbols: Vec<String>,
    /// Comma-separated channels (e.g. ticker,trades,book,executions);
    /// repeatable.
    #[arg(long, value_delimiter = ',', ignore_case = true)]
    #[serde(default)]
    pub channels: Vec<SubscribableChannel>,
    /// Per-channel subscribe options, shared with `kraken record`.
    #[command(flatten)]
    #[serde(flatten)]
    pub options: SubscriptionOptions,
}

crate::tools! {
    /// The `kraken streaming streamd` commands.
    fn streaming_streamd in ["streaming", "streamd"] of StreamdCommand {
        Start as "start" => Websocket, Local, Spot, Stream
            example "kraken streamd start --symbols BTC/USD --channels trades";
    }
}
