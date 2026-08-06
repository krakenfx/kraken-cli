/// Subaccount management commands.
use std::collections::HashMap;

use clap::Subcommand;
use enum_dispatch::enum_dispatch;

use super::Execute;
use crate::cli::AppContext;
use crate::errors::Result;
use crate::output::CommandOutput;

#[enum_dispatch(Execute)]
#[derive(Debug, Subcommand)]
pub(crate) enum SubaccountCommand {
    /// Create a new subaccount.
    Create(Create),
    /// Transfer funds between accounts.
    Transfer(Transfer),
}

#[derive(Debug, clap::Args)]
pub(crate) struct Create {
    /// Username for the subaccount.
    username: String,
    /// Email address for the subaccount.
    email: String,
}

impl Execute for Create {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self { username, email } = self;
        let mut params = HashMap::new();
        params.insert("username".into(), username);
        params.insert("email".into(), email);
        let data = client
            .private_post("CreateSubaccount", params, creds, otp, false)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Transfer {
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
}

impl Execute for Transfer {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            asset,
            amount,
            from,
            to,
            asset_class,
        } = self;
        ctx.confirm_destructive(&format!("Transfer {amount} {asset} from {from} to {to}?"))?;
        let mut params = HashMap::new();
        params.insert("asset".into(), asset);
        params.insert("amount".into(), amount);
        params.insert("from".into(), from);
        params.insert("to".into(), to);
        if let Some(ac) = asset_class {
            params.insert("asset_class".into(), ac);
        }
        let data = client
            .private_post("AccountTransfer", params, creds, otp, false)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}
