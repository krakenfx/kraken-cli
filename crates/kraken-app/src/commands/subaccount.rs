//! Subaccount management commands.

use super::{Execute, rest_bridge};
use crate::{cli::AppContext, errors::Result, output::CommandOutput};
use kraken_contract::subaccount::{Create, SubaccountCommand, Transfer};
use kraken_rest::spot::subaccount as rest;

impl Execute for Create {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { username, email } = self;
        let request = rest::CreateSubaccount { username, email };
        let created = ctx.spot_account()?.sign(&request).await?;
        CommandOutput::published(&created)
    }
}

impl Execute for Transfer {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            asset,
            amount,
            from,
            to,
            asset_class,
        } = self;
        let request = rest::AccountTransfer {
            amount: rest_bridge::amount("amount", &amount)?,
            asset_class: asset_class.as_deref().map(rest_bridge::open_asset_class),
            asset,
            from,
            to,
        };
        let account = ctx.spot_account()?;
        ctx.confirm_destructive(&format!(
            "Transfer {amount} {} from {} to {}?",
            request.asset, request.from, request.to
        ))?;
        let moved = account.sign(&request).await?;
        CommandOutput::published(&moved)
    }
}

delegate_execute!(SubaccountCommand { Create, Transfer });
