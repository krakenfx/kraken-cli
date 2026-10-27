//! The streaming command surface: ws, streamd, record, replay, setup.

use crate::{record::RecordCommand, replay::ReplayCommand, streamd::StreamdCommand, ws::WsCommand};
use clap::Subcommand;
use std::num::NonZeroU64;

// One value exists per process, parsed once at startup, so the size gap
// between `Ws` and the argless variants costs nothing worth boxing for.
#[allow(clippy::large_enum_variant)]
/// The commands that run until stopped and render their own output, so their
/// result is an empty receipt rather than a value to print.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum StreamingCommand {
    /// WebSocket streaming commands.
    Ws(Ws),
    /// Long-lived multi-channel, multi-symbol streaming daemon.
    #[command(subcommand)]
    Streamd(StreamdCommand),
    /// Capture live market data (trades/book/ohlc) into a local tape.
    Record(RecordCommand),
    /// Replay a recorded session's timeline to stdout at a chosen speed.
    Replay(ReplayCommand),
    /// Guided first-time setup wizard.
    Setup(Setup),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Ws {
    #[command(subcommand)]
    pub cmd: WsCommand,
    /// Client request ID echoed back for reply correlation, > 0. When omitted,
    /// subscriptions send no ID and one-shot methods default to 1.
    #[arg(long, global = true)]
    pub req_id: Option<NonZeroU64>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Setup {}

crate::tools! {
    /// The `kraken streaming` commands.
    fn streaming in ["streaming"] of StreamingCommand {
        RecordCommand as "record" => Workspace, Local, None, Stream
            example "kraken record --symbols BTC/USD --channels trades,book --tape my-tape --to jsonl";
        ReplayCommand as "replay" => Workspace, Local, None, Stream
            example "kraken replay --session latest --speed 10";
        Setup         as "setup"  => Utility,   Local, None, Interactive
            example "kraken setup";
    }
}
