//! `kraken earn`.

use clap::Subcommand;

/// The `earn` family: staking allocations and the strategies behind them.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum EarnCommand {
    /// Allocate funds to an earn strategy.
    Allocate(Allocate),
    /// Deallocate funds from an earn strategy.
    Deallocate(Deallocate),
    /// Check allocation status.
    AllocateStatus(AllocateStatus),
    /// Check deallocation status.
    DeallocateStatus(DeallocateStatus),
    /// List available earn strategies.
    Strategies(Strategies),
    /// Get current allocations.
    Allocations(Allocations),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Allocate {
    /// Strategy ID.
    pub strategy_id: String,
    /// Amount to allocate.
    pub amount: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Deallocate {
    /// Strategy ID.
    pub strategy_id: String,
    /// Amount to deallocate.
    pub amount: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct AllocateStatus {
    /// Strategy ID.
    pub strategy_id: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct DeallocateStatus {
    /// Strategy ID.
    pub strategy_id: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Strategies {
    /// Filter by asset.
    #[arg(long)]
    pub asset: Option<String>,
    /// Filter by lock type (flex, bonded, timed, instant). Can specify
    /// multiple.
    #[arg(long)]
    #[serde(default)]
    pub lock_type: Vec<String>,
    /// Sort ascending (default: descending).
    #[arg(long)]
    #[serde(default)]
    pub ascending: bool,
    /// Cursor for next page of results.
    #[arg(long)]
    pub cursor: Option<String>,
    /// Number of items per page.
    #[arg(long)]
    pub limit: Option<u16>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Allocations {
    /// Sort ascending (default: descending).
    #[arg(long)]
    #[serde(default)]
    pub ascending: bool,
    /// Secondary currency to express allocation value (default: USD).
    #[arg(long)]
    pub converted_asset: Option<String>,
    /// Omit strategies with zero allocations.
    #[arg(long)]
    #[serde(default)]
    pub hide_zero_allocations: bool,
}

crate::tools! {
    /// The `kraken earn` commands.
    fn earn in ["earn"] of EarnCommand {
        Allocate         as "allocate"          => Earn, Dangerous, Spot, Tool
            example "kraken earn allocate ESXUM7H-SJHQ6-KOBAL7 1.5";
        Deallocate       as "deallocate"        => Earn, Dangerous, Spot, Tool
            example "kraken earn deallocate ESXUM7H-SJHQ6-KOBAL7 0.5";
        AllocateStatus   as "allocate-status"   => Earn, ReadOnly,  Spot, Tool
            example "kraken earn allocate-status ESXUM7H-SJHQ6-KOBAL7";
        DeallocateStatus as "deallocate-status" => Earn, ReadOnly,  Spot, Tool
            example "kraken earn deallocate-status ESXUM7H-SJHQ6-KOBAL7";
        Strategies       as "strategies"        => Earn, ReadOnly,  Spot, Tool
            example "kraken earn strategies --asset ETH -o json";
        Allocations      as "allocations"       => Earn, ReadOnly,  Spot, Tool
            example "kraken earn allocations -o json";
    }
}
