/// Funding commands: deposits, withdrawals, wallet transfers.
use std::collections::HashMap;

use clap::Subcommand;
use enum_dispatch::enum_dispatch;

use super::Execute;
use super::value_types::{AssetClass, CliWire, RebaseOpts};
use crate::cli::AppContext;
use crate::errors::Result;
use crate::output::CommandOutput;

#[enum_dispatch(Execute)]
#[derive(Debug, Subcommand)]
pub(crate) enum DepositCommand {
    /// Get deposit methods for an asset.
    Methods(DepositMethods),
    /// Get deposit addresses.
    Addresses(DepositAddresses),
    /// Get deposit status.
    Status(DepositStatus),
}

#[enum_dispatch(Execute)]
#[derive(Debug, Subcommand)]
pub(crate) enum WithdrawalCommand {
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

#[derive(Debug, clap::Args)]
pub(crate) struct DepositMethods {
    asset: String,
    /// Asset class (currency or tokenized_asset for xstocks).
    #[arg(long, alias = "aclass")]
    asset_class: Option<AssetClass>,
    #[command(flatten)]
    rebase: RebaseOpts,
}

impl Execute for DepositMethods {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            asset,
            asset_class,
            rebase,
        } = self;
        let mut params = HashMap::new();
        params.insert("asset".into(), asset);
        if let Some(ac) = asset_class {
            params.insert("aclass".into(), ac.as_wire());
        }
        rebase.apply(&mut params);
        let data = client
            .private_post("DepositMethods", params, creds, otp, true)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct DepositAddresses {
    asset: String,
    method: String,
    #[arg(long)]
    new: bool,
    /// Asset class (currency or tokenized_asset for xstocks).
    #[arg(long, alias = "aclass")]
    asset_class: Option<AssetClass>,
    /// Amount to deposit (required for Bitcoin Lightning).
    #[arg(long)]
    amount: Option<String>,
}

impl Execute for DepositAddresses {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            asset,
            method,
            new,
            asset_class,
            amount,
        } = self;
        let mut params = HashMap::new();
        params.insert("asset".into(), asset);
        params.insert("method".into(), method);
        if new {
            params.insert("new".into(), "true".into());
        }
        if let Some(ac) = asset_class {
            params.insert("aclass".into(), ac.as_wire());
        }
        if let Some(amt) = amount {
            params.insert("amount".into(), amt);
        }
        let idempotent = !new;
        let data = client
            .private_post("DepositAddresses", params, creds, otp, idempotent)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

/// Filter surface shared verbatim by `deposit status` and `withdrawal status`;
/// the wire endpoint is the only divergence between the two commands.
#[derive(Debug, clap::Args)]
pub(crate) struct FundingStatusFilter {
    #[arg(long)]
    asset: Option<String>,
    /// Asset class (currency or tokenized_asset for xstocks).
    #[arg(long, alias = "aclass")]
    asset_class: Option<AssetClass>,
    /// Filter by transfer method name.
    #[arg(long)]
    method: Option<String>,
    /// Start timestamp (transfers before this are excluded).
    #[arg(long)]
    start: Option<String>,
    /// End timestamp (transfers after this are excluded).
    #[arg(long)]
    end: Option<String>,
    /// Enable pagination (true/false) or cursor for next page.
    #[arg(long)]
    cursor: Option<String>,
    /// Number of results per page.
    #[arg(long)]
    limit: Option<u32>,
    #[command(flatten)]
    rebase: RebaseOpts,
}

async fn fetch_funding_status(
    ctx: &AppContext,
    endpoint: &str,
    filter: FundingStatusFilter,
) -> Result<CommandOutput> {
    let (client, creds, otp) = ctx.spot_authed()?;
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
    let mut params = HashMap::new();
    if let Some(a) = asset {
        params.insert("asset".into(), a);
    }
    if let Some(ac) = asset_class {
        params.insert("aclass".into(), ac.as_wire());
    }
    if let Some(m) = method {
        params.insert("method".into(), m);
    }
    if let Some(s) = start {
        params.insert("start".into(), s);
    }
    if let Some(e) = end {
        params.insert("end".into(), e);
    }
    if let Some(c) = cursor {
        params.insert("cursor".into(), c);
    }
    if let Some(l) = limit {
        params.insert("limit".into(), l.to_string());
    }
    rebase.apply(&mut params);
    let data = client
        .private_post(endpoint, params, creds, otp, true)
        .await?;
    Ok(CommandOutput::from_untyped(&data))
}

#[derive(Debug, clap::Args)]
pub(crate) struct DepositStatus {
    #[command(flatten)]
    filter: FundingStatusFilter,
}

impl Execute for DepositStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        fetch_funding_status(ctx, "DepositStatus", self.filter).await
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct WithdrawalMethods {
    #[arg(long)]
    asset: Option<String>,
    /// Asset class (currency or tokenized_asset for xstocks).
    #[arg(long, alias = "aclass")]
    asset_class: Option<AssetClass>,
    /// Filter by network.
    #[arg(long)]
    network: Option<String>,
    #[command(flatten)]
    rebase: RebaseOpts,
}

impl Execute for WithdrawalMethods {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            asset,
            asset_class,
            network,
            rebase,
        } = self;
        let mut params = HashMap::new();
        if let Some(a) = asset {
            params.insert("asset".into(), a);
        }
        if let Some(ac) = asset_class {
            params.insert("aclass".into(), ac.as_wire());
        }
        if let Some(n) = network {
            params.insert("network".into(), n);
        }
        rebase.apply(&mut params);
        let data = client
            .private_post("WithdrawMethods", params, creds, otp, true)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct WithdrawalAddresses {
    #[arg(long)]
    asset: Option<String>,
    /// Asset class (currency or tokenized_asset for xstocks).
    #[arg(long, alias = "aclass")]
    asset_class: Option<AssetClass>,
    /// Filter by withdrawal method.
    #[arg(long)]
    method: Option<String>,
    /// Find address by withdrawal key name.
    #[arg(long)]
    key: Option<String>,
    /// Filter by verification status.
    #[arg(long)]
    verified: Option<bool>,
}

impl Execute for WithdrawalAddresses {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            asset,
            asset_class,
            method,
            key,
            verified,
        } = self;
        let mut params = HashMap::new();
        if let Some(a) = asset {
            params.insert("asset".into(), a);
        }
        if let Some(ac) = asset_class {
            params.insert("aclass".into(), ac.as_wire());
        }
        if let Some(m) = method {
            params.insert("method".into(), m);
        }
        if let Some(k) = key {
            params.insert("key".into(), k);
        }
        if let Some(v) = verified {
            params.insert("verified".into(), v.to_string());
        }
        let data = client
            .private_post("WithdrawAddresses", params, creds, otp, true)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct WithdrawalInfo {
    asset: String,
    key: String,
    amount: String,
}

impl Execute for WithdrawalInfo {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self { asset, key, amount } = self;
        let mut params = HashMap::new();
        params.insert("asset".into(), asset);
        params.insert("key".into(), key);
        params.insert("amount".into(), amount);
        let data = client
            .private_post("WithdrawInfo", params, creds, otp, true)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Withdraw {
    /// Asset being withdrawn.
    asset: String,
    /// Withdrawal key name, as set up on your account.
    key: String,
    /// Amount to withdraw.
    amount: String,
    /// Asset class (currency or tokenized_asset for xstocks).
    #[arg(long, alias = "aclass")]
    asset_class: Option<AssetClass>,
    /// Crypto address to confirm it matches the key.
    #[arg(long)]
    address: Option<String>,
    /// Max fee — fails if processed fee exceeds this.
    #[arg(long)]
    max_fee: Option<String>,
    #[command(flatten)]
    rebase: RebaseOpts,
}

impl Execute for Withdraw {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            asset,
            key,
            amount,
            asset_class,
            address,
            max_fee,
            rebase,
        } = self;
        ctx.confirm_destructive(&format!("Withdraw {amount} {asset} to key '{key}'?"))?;
        let mut params = HashMap::new();
        params.insert("asset".into(), asset);
        params.insert("key".into(), key);
        params.insert("amount".into(), amount);
        if let Some(ac) = asset_class {
            params.insert("aclass".into(), ac.as_wire());
        }
        if let Some(addr) = address {
            params.insert("address".into(), addr);
        }
        if let Some(mf) = max_fee {
            params.insert("max_fee".into(), mf);
        }
        rebase.apply(&mut params);
        let data = client
            .private_post("Withdraw", params, creds, otp, false)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct WithdrawalStatus {
    #[command(flatten)]
    filter: FundingStatusFilter,
}

impl Execute for WithdrawalStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        fetch_funding_status(ctx, "WithdrawStatus", self.filter).await
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct WithdrawalCancel {
    /// Asset being withdrawn.
    asset: String,
    /// Withdrawal reference ID.
    refid: String,
}

impl Execute for WithdrawalCancel {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self { asset, refid } = self;
        let mut params = HashMap::new();
        params.insert("asset".into(), asset);
        params.insert("refid".into(), refid);
        let data = client
            .private_post("WithdrawCancel", params, creds, otp, false)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct WalletTransfer {
    /// Asset.
    asset: String,
    /// Amount.
    amount: String,
    /// Source wallet.
    #[arg(long)]
    from: String,
    /// Destination wallet.
    #[arg(long)]
    to: String,
}

impl Execute for WalletTransfer {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            asset,
            amount,
            from,
            to,
        } = self;
        ctx.confirm_destructive(&format!("Transfer {amount} {asset} from {from} to {to}?"))?;
        let mut params = HashMap::new();
        params.insert("asset".into(), asset);
        params.insert("amount".into(), amount);
        params.insert("from".into(), from);
        params.insert("to".into(), to);
        let data = client
            .private_post("WalletTransfer", params, creds, otp, false)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}
