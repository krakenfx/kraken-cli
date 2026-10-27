//! `kraken explain`: arguments for the plain-language P&L walkthrough.

use clap::Subcommand;

/// The `explain` family: plain-language walkthroughs of what a recorded
/// session did.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ExplainCommand {
    /// Decompose a stopped session's P&L into plain-terms components.
    ///
    /// Price movement, fees, spread, slippage and residual, with missed-fill
    /// counterfactuals and per-origin attribution. Needs `kraken session stop`
    /// to have run first.
    Pnl(Pnl),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Pnl {
    /// Which session: `latest`, an ordinal (`s3`), or a label.
    #[arg(long, default_value = "latest")]
    #[serde(default = "crate::value_types::latest_session")]
    pub session: String,
}

crate::tools! {
    /// The `kraken explain` commands.
    fn explain in ["explain"] of ExplainCommand {
        Pnl as "pnl" => Paper, ReadOnly, None, Tool
            example "kraken explain pnl --session latest -o json";
    }
}
