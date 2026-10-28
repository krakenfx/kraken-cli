//! Funding commands: deposits, withdrawals, wallet transfers.

use super::{Execute, rest_bridge};
use crate::{cli::AppContext, errors::Result, output::CommandOutput};
use kraken_contract::funding::{
    DepositAddresses, DepositCommand, DepositMethods, DepositStatus, FundingStatusFilter,
    WalletTransfer, Withdraw, WithdrawalAddresses, WithdrawalCancel, WithdrawalCommand,
    WithdrawalInfo, WithdrawalMethods, WithdrawalStatus,
};
use kraken_rest::spot::funding as rest;

impl Execute for DepositMethods {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            asset,
            asset_class,
            rebase,
        } = self;
        let query = rest::DepositMethodsQuery {
            asset,
            aclass: asset_class.map(rest_bridge::asset_class),
            rebase_multiplier: rest_bridge::rebase(&rebase),
        };
        let methods = ctx.spot_account()?.sign(&query).await?;
        CommandOutput::published(&methods)
    }
}

impl Execute for DepositAddresses {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            asset,
            method,
            new,
            asset_class,
            amount,
        } = self;
        let query = rest::DepositAddressesQuery {
            asset,
            method,
            new,
            aclass: asset_class.map(rest_bridge::asset_class),
            amount,
        };
        let addresses = ctx.spot_account()?.sign(&query).await?;
        CommandOutput::published(&addresses)
    }
}

fn transfer_filter(filter: FundingStatusFilter) -> rest::TransferFilter {
    let FundingStatusFilter {
        asset,
        asset_class,
        method,
        start,
        end,
        cursor,
        limit,
        rebase,
    } = filter;
    rest::TransferFilter {
        asset,
        aclass: asset_class.map(rest_bridge::asset_class),
        method,
        start,
        end,
        cursor,
        limit,
        rebase_multiplier: rest_bridge::rebase(&rebase),
    }
}

impl Execute for DepositStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let query = rest::DepositStatusQuery {
            filter: transfer_filter(self.filter),
        };
        let deposits = ctx.spot_account()?.sign(&query).await?;
        CommandOutput::published(&deposits)
    }
}

impl Execute for WithdrawalMethods {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            asset,
            asset_class,
            network,
            rebase,
        } = self;
        let query = rest::WithdrawMethodsQuery {
            asset,
            aclass: asset_class.map(rest_bridge::asset_class),
            network,
            rebase_multiplier: rest_bridge::rebase(&rebase),
        };
        let methods = ctx.spot_account()?.sign(&query).await?;
        CommandOutput::published(&methods)
    }
}

impl Execute for WithdrawalAddresses {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            asset,
            asset_class,
            method,
            key,
            verified,
        } = self;
        let query = rest::WithdrawAddressesQuery {
            asset,
            aclass: asset_class.map(rest_bridge::asset_class),
            method,
            key,
            verified,
        };
        let addresses = ctx.spot_account()?.sign(&query).await?;
        CommandOutput::published(&addresses)
    }
}

impl Execute for WithdrawalInfo {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { asset, key, amount } = self;
        let query = rest::WithdrawInfoQuery {
            asset,
            key,
            amount: rest_bridge::amount("amount", &amount)?,
        };
        let quote = ctx.spot_account()?.sign(&query).await?;
        CommandOutput::published(&quote)
    }
}

impl Execute for Withdraw {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            asset,
            key,
            amount,
            asset_class,
            address,
            max_fee,
            rebase,
        } = self;
        let request = rest::Withdraw {
            amount: rest_bridge::amount("amount", &amount)?,
            max_fee: max_fee
                .as_deref()
                .map(|fee| rest_bridge::amount("--max-fee", fee))
                .transpose()?,
            aclass: asset_class.map(rest_bridge::asset_class),
            address,
            rebase_multiplier: rest_bridge::rebase(&rebase),
            asset,
            key,
        };
        let account = ctx.spot_account()?;
        ctx.confirm_destructive(&format!(
            "Withdraw {amount} {} to key '{}'?",
            request.asset, request.key
        ))?;
        let withdrawal = account.sign(&request).await?;
        CommandOutput::published(&withdrawal)
    }
}

impl Execute for WithdrawalStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let query = rest::WithdrawStatusQuery {
            filter: transfer_filter(self.filter),
        };
        let withdrawals = ctx.spot_account()?.sign(&query).await?;
        CommandOutput::published(&withdrawals)
    }
}

impl Execute for WithdrawalCancel {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { asset, refid } = self;
        let request = rest::WithdrawCancel { asset, refid };
        let cancelled = ctx.spot_account()?.sign(&request).await?;
        CommandOutput::published(&cancelled)
    }
}

impl Execute for WalletTransfer {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            asset,
            amount,
            from,
            to,
        } = self;
        let request = rest::WalletTransfer {
            amount: rest_bridge::amount("amount", &amount)?,
            asset,
            from,
            to,
        };
        let account = ctx.spot_account()?;
        ctx.confirm_destructive(&format!(
            "Transfer {amount} {} from {} to {}?",
            request.asset, request.from, request.to
        ))?;
        let transfer = account.sign(&request).await?;
        CommandOutput::published(&transfer)
    }
}

delegate_execute!(DepositCommand {
    Methods,
    Addresses,
    Status
});
delegate_execute!(WithdrawalCommand {
    Methods,
    Addresses,
    Info,
    Status,
    Cancel
});
