/// Subaccount management commands.
use std::collections::HashMap;

use clap::Subcommand;

use super::helpers::{confirm_destructive, parse_generic};
use crate::client::SpotClient;
use crate::config::SpotCredentials;
use crate::errors::Result;
use crate::output::CommandOutput;

#[derive(Debug, Subcommand)]
pub(crate) enum SubaccountCommand {
    /// Create a new subaccount.
    Create {
        /// Username for the subaccount.
        username: String,
        /// Email address for the subaccount.
        email: String,
    },
    /// Transfer funds between accounts.
    Transfer {
        /// Asset to transfer.
        asset: String,
        /// Amount to transfer.
        amount: String,
        /// IIBAN of the source account.
        #[arg(long)]
        from: String,
        /// IIBAN of the destination account.
        #[arg(long)]
        to: String,
        /// Asset class (currency or tokenized_asset for xstocks).
        #[arg(long)]
        asset_class: Option<String>,
    },
}

pub(crate) async fn execute(
    cmd: &SubaccountCommand,
    client: &SpotClient,
    creds: &SpotCredentials,
    otp: Option<&str>,
    force: bool,
    verbose: bool,
) -> Result<CommandOutput> {
    match cmd {
        SubaccountCommand::Create { username, email } => {
            let mut params = HashMap::new();
            params.insert("username".into(), username.clone());
            params.insert("email".into(), email.clone());
            let data = client
                .private_post("CreateSubaccount", params, creds, otp, false, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        SubaccountCommand::Transfer {
            asset,
            amount,
            from,
            to,
            asset_class,
        } => {
            if !force {
                confirm_destructive(&format!("Transfer {amount} {asset} from {from} to {to}?"))?;
            }
            let mut params = HashMap::new();
            params.insert("asset".into(), asset.clone());
            params.insert("amount".into(), amount.clone());
            params.insert("from".into(), from.clone());
            params.insert("to".into(), to.clone());
            if let Some(ac) = asset_class {
                params.insert("asset_class".into(), ac.clone());
            }
            let data = client
                .private_post("AccountTransfer", params, creds, otp, false, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
    }
}
