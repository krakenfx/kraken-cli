//! `kraken subaccount`.

use clap::Subcommand;

/// The `subaccount` family: creating subaccounts and moving funds between
/// them.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SubaccountCommand {
    /// Create a new subaccount.
    Create(Create),
    /// Transfer funds between accounts.
    Transfer(Transfer),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Create {
    /// Username for the subaccount.
    pub username: String,
    /// Email address for the subaccount.
    pub email: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Transfer {
    /// Asset to transfer.
    pub asset: String,
    /// Amount to transfer.
    pub amount: String,
    /// IIBAN of the source account.
    #[arg(long)]
    pub from: String,
    /// IIBAN of the destination account.
    #[arg(long)]
    pub to: String,
    /// Asset class (currency or tokenized_asset for xstocks).
    #[arg(long)]
    pub asset_class: Option<String>,
}

crate::tools! {
    /// The `kraken subaccount` commands.
    fn subaccount in ["subaccount"] of SubaccountCommand {
        Create   as "create"   => Subaccount, Dangerous, Spot, Tool
            example "kraken subaccount create trading-bot bot@example.com";
        Transfer as "transfer" => Subaccount, Dangerous, Spot, Tool
            example "kraken subaccount transfer USD 5000 --from main --to trading-bot";
    }
}
