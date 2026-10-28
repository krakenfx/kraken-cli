//! `kraken funding`.

use crate::value_types::{AssetClass, RebaseOpts};
use clap::Subcommand;

/// The `deposit` family: how funds can arrive, and where.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DepositCommand {
    /// Get deposit methods for an asset.
    Methods(DepositMethods),
    /// Get deposit addresses.
    Addresses(DepositAddresses),
    /// Get deposit status.
    Status(DepositStatus),
}

/// The `withdrawal` family: how funds can leave, and the status of ones
/// already sent. The withdrawal itself is `withdraw`, not part of this family.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum WithdrawalCommand {
    /// Get withdrawal methods.
    Methods(WithdrawalMethods),
    /// Get withdrawal addresses.
    Addresses(WithdrawalAddresses),
    /// Get withdrawal fee info.
    Info(WithdrawalInfo),
    /// Get withdrawal status.
    Status(WithdrawalStatus),
    /// Cancel a pending withdrawal.
    Cancel(WithdrawalCancel),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct DepositMethods {
    pub asset: String,
    /// Asset class (currency or tokenized_asset for xstocks).
    #[arg(long, alias = "aclass")]
    pub asset_class: Option<AssetClass>,
    #[command(flatten)]
    #[serde(flatten)]
    pub rebase: RebaseOpts,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct DepositAddresses {
    pub asset: String,
    pub method: String,
    #[arg(long)]
    #[serde(default)]
    pub new: bool,
    /// Asset class (currency or tokenized_asset for xstocks).
    #[arg(long, alias = "aclass")]
    pub asset_class: Option<AssetClass>,
    /// Amount to deposit (required for Bitcoin Lightning).
    #[arg(long)]
    pub amount: Option<String>,
}

// Shared by `deposit status` and `withdrawal status`, which differ only in the
// endpoint they call.
#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct FundingStatusFilter {
    #[arg(long)]
    pub asset: Option<String>,
    /// Asset class (currency or tokenized_asset for xstocks).
    #[arg(long, alias = "aclass")]
    pub asset_class: Option<AssetClass>,
    /// Filter by transfer method name.
    #[arg(long)]
    pub method: Option<String>,
    /// Start timestamp (transfers before this are excluded).
    #[arg(long)]
    pub start: Option<String>,
    /// End timestamp (transfers after this are excluded).
    #[arg(long)]
    pub end: Option<String>,
    /// Enable pagination (true/false) or cursor for next page.
    #[arg(long)]
    pub cursor: Option<String>,
    /// Number of results per page.
    #[arg(long)]
    pub limit: Option<u32>,
    #[command(flatten)]
    #[serde(flatten)]
    pub rebase: RebaseOpts,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct DepositStatus {
    #[command(flatten)]
    #[serde(flatten)]
    pub filter: FundingStatusFilter,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct WithdrawalMethods {
    #[arg(long)]
    pub asset: Option<String>,
    /// Asset class (currency or tokenized_asset for xstocks).
    #[arg(long, alias = "aclass")]
    pub asset_class: Option<AssetClass>,
    /// Filter by network.
    #[arg(long)]
    pub network: Option<String>,
    #[command(flatten)]
    #[serde(flatten)]
    pub rebase: RebaseOpts,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct WithdrawalAddresses {
    #[arg(long)]
    pub asset: Option<String>,
    /// Asset class (currency or tokenized_asset for xstocks).
    #[arg(long, alias = "aclass")]
    pub asset_class: Option<AssetClass>,
    /// Filter by withdrawal method.
    #[arg(long)]
    pub method: Option<String>,
    /// Find address by withdrawal key name.
    #[arg(long)]
    pub key: Option<String>,
    /// Filter by verification status.
    #[arg(long)]
    pub verified: Option<bool>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct WithdrawalInfo {
    pub asset: String,
    pub key: String,
    pub amount: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Withdraw {
    /// Asset being withdrawn.
    pub asset: String,
    /// Withdrawal key name, as set up on your account.
    pub key: String,
    /// Amount to withdraw.
    pub amount: String,
    /// Asset class (currency or tokenized_asset for xstocks).
    #[arg(long, alias = "aclass")]
    pub asset_class: Option<AssetClass>,
    /// Crypto address to confirm it matches the key.
    #[arg(long)]
    pub address: Option<String>,
    /// Max fee — fails if processed fee exceeds this.
    #[arg(long)]
    pub max_fee: Option<String>,
    #[command(flatten)]
    #[serde(flatten)]
    pub rebase: RebaseOpts,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct WithdrawalStatus {
    #[command(flatten)]
    #[serde(flatten)]
    pub filter: FundingStatusFilter,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct WithdrawalCancel {
    /// Asset being withdrawn.
    pub asset: String,
    /// Withdrawal reference ID.
    pub refid: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct WalletTransfer {
    /// Asset.
    pub asset: String,
    /// Amount.
    pub amount: String,
    /// Source wallet.
    #[arg(long)]
    pub from: String,
    /// Destination wallet.
    #[arg(long)]
    pub to: String,
}

crate::tools! {
    /// The `kraken deposit` commands.
    fn deposit in ["deposit"] of DepositCommand {
        DepositMethods   as "methods"   => Funding, ReadOnly, Spot, Tool
            example "kraken deposit methods BTC";
        DepositAddresses as "addresses" => Funding, ReadOnly, Spot, Tool
            example "kraken deposit addresses BTC \"Bitcoin\"";
        DepositStatus    as "status"    => Funding, ReadOnly, Spot, Tool
            example "kraken deposit status --asset BTC -o json";
    }
}

crate::tools! {
    /// The `kraken withdrawal` commands.
    fn withdrawal in ["withdrawal"] of WithdrawalCommand {
        WithdrawalMethods   as "methods"   => Funding, ReadOnly,  Spot, Tool
            example "kraken withdrawal methods --asset BTC";
        WithdrawalAddresses as "addresses" => Funding, ReadOnly,  Spot, Tool
            example "kraken withdrawal addresses --asset ETH";
        WithdrawalInfo      as "info"      => Funding, ReadOnly,  Spot, Tool
            example "kraken withdrawal info BTC my-btc-wallet 0.5";
        WithdrawalStatus    as "status"    => Funding, ReadOnly,  Spot, Tool
            example "kraken withdrawal status --asset BTC -o json";
        WithdrawalCancel    as "cancel"    => Funding, Dangerous, Spot, Tool
            example "kraken withdrawal cancel BTC AGBSO6T-WGH4X-CQXB5J";
    }
}
