//! `kraken tape`: the read-only catalog of market memory — every standalone

use clap::Subcommand;

/// The `tape` family: the catalog of recorded market data available to replay.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TapeCommand {
    /// List every tape in the library: replayable ref, backend, capture
    /// state, symbols, channels, and whether a writer holds it now.
    List(List),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct List {}

crate::tools! {
    /// The `kraken tape` commands.
    fn tape in ["tape"] of TapeCommand {
        List as "list" => Market, ReadOnly, None, Tool
            example "kraken tape list -o json";
    }
}
