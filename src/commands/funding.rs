/// Funding commands: deposits, withdrawals, wallet transfers.
use std::collections::HashMap;

use clap::Subcommand;

use super::helpers::{confirm_destructive, parse_generic};
use crate::client::SpotClient;
use crate::config::SpotCredentials;
use crate::errors::Result;
use crate::output::CommandOutput;

#[derive(Debug, Subcommand)]
pub(crate) enum FundingCommand {
    /// Get available deposit methods for an asset.
    DepositMethods {
        /// Asset being deposited (e.g. BTC, ETH).
        asset: String,
        /// Asset class (currency or tokenized_asset for xstocks).
        #[arg(long, alias = "aclass")]
        asset_class: Option<String>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get deposit addresses.
    DepositAddresses {
        /// Asset being deposited.
        asset: String,
        /// Name of the deposit method.
        method: String,
        /// Generate a new address.
        #[arg(long)]
        new: bool,
        /// Asset class (currency or tokenized_asset for xstocks).
        #[arg(long, alias = "aclass")]
        asset_class: Option<String>,
        /// Amount to deposit (required for Bitcoin Lightning).
        #[arg(long)]
        amount: Option<String>,
    },
    /// Get recent deposit status.
    DepositStatus {
        /// Filter by asset.
        #[arg(long)]
        asset: Option<String>,
        /// Asset class (currency or tokenized_asset for xstocks).
        #[arg(long, alias = "aclass")]
        asset_class: Option<String>,
        /// Filter by deposit method name.
        #[arg(long)]
        method: Option<String>,
        /// Start timestamp (deposits before this are excluded).
        #[arg(long)]
        start: Option<String>,
        /// End timestamp (deposits after this are excluded).
        #[arg(long)]
        end: Option<String>,
        /// Enable pagination (true/false) or cursor string for next page.
        #[arg(long)]
        cursor: Option<String>,
        /// Number of results per page.
        #[arg(long)]
        limit: Option<u32>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get available withdrawal methods.
    WithdrawalMethods {
        /// Filter by asset.
        #[arg(long)]
        asset: Option<String>,
        /// Asset class (currency or tokenized_asset for xstocks).
        #[arg(long, alias = "aclass")]
        asset_class: Option<String>,
        /// Filter by network.
        #[arg(long)]
        network: Option<String>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get withdrawal addresses.
    WithdrawalAddresses {
        /// Filter by asset.
        #[arg(long)]
        asset: Option<String>,
        /// Asset class (currency or tokenized_asset for xstocks).
        #[arg(long, alias = "aclass")]
        asset_class: Option<String>,
        /// Filter by withdrawal method.
        #[arg(long)]
        method: Option<String>,
        /// Find address by withdrawal key name.
        #[arg(long)]
        key: Option<String>,
        /// Filter by verification status.
        #[arg(long)]
        verified: Option<bool>,
    },
    /// Get withdrawal fee info.
    WithdrawalInfo {
        /// Asset.
        asset: String,
        /// Withdrawal key name.
        key: String,
        /// Amount.
        amount: String,
    },
    /// Make a withdrawal.
    Withdraw {
        /// Asset being withdrawn.
        asset: String,
        /// Withdrawal key name, as set up on your account.
        key: String,
        /// Amount to withdraw.
        amount: String,
        /// Asset class (currency or tokenized_asset for xstocks).
        #[arg(long, alias = "aclass")]
        asset_class: Option<String>,
        /// Crypto address to confirm it matches the key (returns error if different).
        #[arg(long)]
        address: Option<String>,
        /// Max fee — withdrawal fails with EFunding:Max fee exceeded if fee is higher.
        #[arg(long)]
        max_fee: Option<String>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get recent withdrawal status.
    WithdrawalStatus {
        /// Filter by asset.
        #[arg(long)]
        asset: Option<String>,
        /// Asset class (currency or tokenized_asset for xstocks).
        #[arg(long, alias = "aclass")]
        asset_class: Option<String>,
        /// Filter by withdrawal method name.
        #[arg(long)]
        method: Option<String>,
        /// Start timestamp (withdrawals before this are excluded).
        #[arg(long)]
        start: Option<String>,
        /// End timestamp (withdrawals after this are excluded).
        #[arg(long)]
        end: Option<String>,
        /// Enable pagination (true/false) or cursor string for next page.
        #[arg(long)]
        cursor: Option<String>,
        /// Number of results per page.
        #[arg(long)]
        limit: Option<u32>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Cancel a pending withdrawal.
    WithdrawalCancel {
        /// Asset being withdrawn.
        asset: String,
        /// Withdrawal reference ID.
        refid: String,
    },
    /// Transfer between wallets (spot <-> futures).
    WalletTransfer {
        /// Asset to transfer.
        asset: String,
        /// Amount to transfer.
        amount: String,
        /// Source wallet.
        #[arg(long)]
        from: String,
        /// Destination wallet.
        #[arg(long)]
        to: String,
    },
}

pub(crate) async fn execute(
    cmd: &FundingCommand,
    client: &SpotClient,
    creds: &SpotCredentials,
    otp: Option<&str>,
    force: bool,
    verbose: bool,
) -> Result<CommandOutput> {
    match cmd {
        FundingCommand::DepositMethods {
            asset,
            asset_class,
            rebase_multiplier,
        } => {
            let mut params = HashMap::new();
            params.insert("asset".into(), asset.clone());
            if let Some(ac) = asset_class {
                params.insert("aclass".into(), ac.clone());
            }
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("DepositMethods", params, creds, otp, true, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FundingCommand::DepositAddresses {
            asset,
            method,
            new,
            asset_class,
            amount,
        } => {
            let mut params = HashMap::new();
            params.insert("asset".into(), asset.clone());
            params.insert("method".into(), method.clone());
            if *new {
                params.insert("new".into(), "true".into());
            }
            if let Some(ac) = asset_class {
                params.insert("aclass".into(), ac.clone());
            }
            if let Some(amt) = amount {
                params.insert("amount".into(), amt.clone());
            }
            let idempotent = !*new;
            let data = client
                .private_post("DepositAddresses", params, creds, otp, idempotent, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FundingCommand::DepositStatus {
            asset,
            asset_class,
            method,
            start,
            end,
            cursor,
            limit,
            rebase_multiplier,
        } => {
            let mut params = HashMap::new();
            if let Some(a) = asset {
                params.insert("asset".into(), a.clone());
            }
            if let Some(ac) = asset_class {
                params.insert("aclass".into(), ac.clone());
            }
            if let Some(m) = method {
                params.insert("method".into(), m.clone());
            }
            if let Some(s) = start {
                params.insert("start".into(), s.clone());
            }
            if let Some(e) = end {
                params.insert("end".into(), e.clone());
            }
            if let Some(c) = cursor {
                params.insert("cursor".into(), c.clone());
            }
            if let Some(l) = limit {
                params.insert("limit".into(), l.to_string());
            }
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("DepositStatus", params, creds, otp, true, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FundingCommand::WithdrawalMethods {
            asset,
            asset_class,
            network,
            rebase_multiplier,
        } => {
            let mut params = HashMap::new();
            if let Some(a) = asset {
                params.insert("asset".into(), a.clone());
            }
            if let Some(ac) = asset_class {
                params.insert("aclass".into(), ac.clone());
            }
            if let Some(n) = network {
                params.insert("network".into(), n.clone());
            }
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("WithdrawMethods", params, creds, otp, true, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FundingCommand::WithdrawalAddresses {
            asset,
            asset_class,
            method,
            key,
            verified,
        } => {
            let mut params = HashMap::new();
            if let Some(a) = asset {
                params.insert("asset".into(), a.clone());
            }
            if let Some(ac) = asset_class {
                params.insert("aclass".into(), ac.clone());
            }
            if let Some(m) = method {
                params.insert("method".into(), m.clone());
            }
            if let Some(k) = key {
                params.insert("key".into(), k.clone());
            }
            if let Some(v) = verified {
                params.insert("verified".into(), v.to_string());
            }
            let data = client
                .private_post("WithdrawAddresses", params, creds, otp, true, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FundingCommand::WithdrawalInfo { asset, key, amount } => {
            let mut params = HashMap::new();
            params.insert("asset".into(), asset.clone());
            params.insert("key".into(), key.clone());
            params.insert("amount".into(), amount.clone());
            let data = client
                .private_post("WithdrawInfo", params, creds, otp, true, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FundingCommand::Withdraw {
            asset,
            key,
            amount,
            asset_class,
            address,
            max_fee,
            rebase_multiplier,
        } => {
            if !force {
                confirm_destructive(&format!("Withdraw {amount} {asset} to key '{key}'?"))?;
            }
            let mut params = HashMap::new();
            params.insert("asset".into(), asset.clone());
            params.insert("key".into(), key.clone());
            params.insert("amount".into(), amount.clone());
            if let Some(ac) = asset_class {
                params.insert("aclass".into(), ac.clone());
            }
            if let Some(addr) = address {
                params.insert("address".into(), addr.clone());
            }
            if let Some(mf) = max_fee {
                params.insert("max_fee".into(), mf.clone());
            }
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("Withdraw", params, creds, otp, false, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FundingCommand::WithdrawalStatus {
            asset,
            asset_class,
            method,
            start,
            end,
            cursor,
            limit,
            rebase_multiplier,
        } => {
            let mut params = HashMap::new();
            if let Some(a) = asset {
                params.insert("asset".into(), a.clone());
            }
            if let Some(ac) = asset_class {
                params.insert("aclass".into(), ac.clone());
            }
            if let Some(m) = method {
                params.insert("method".into(), m.clone());
            }
            if let Some(s) = start {
                params.insert("start".into(), s.clone());
            }
            if let Some(e) = end {
                params.insert("end".into(), e.clone());
            }
            if let Some(c) = cursor {
                params.insert("cursor".into(), c.clone());
            }
            if let Some(l) = limit {
                params.insert("limit".into(), l.to_string());
            }
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("WithdrawStatus", params, creds, otp, true, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FundingCommand::WithdrawalCancel { asset, refid } => {
            let mut params = HashMap::new();
            params.insert("asset".into(), asset.clone());
            params.insert("refid".into(), refid.clone());
            let data = client
                .private_post("WithdrawCancel", params, creds, otp, false, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FundingCommand::WalletTransfer {
            asset,
            amount,
            from,
            to,
        } => {
            if !force {
                confirm_destructive(&format!("Transfer {amount} {asset} from {from} to {to}?"))?;
            }
            let mut params = HashMap::new();
            params.insert("asset".into(), asset.clone());
            params.insert("amount".into(), amount.clone());
            params.insert("from".into(), from.clone());
            params.insert("to".into(), to.clone());
            let data = client
                .private_post("WalletTransfer", params, creds, otp, false, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
    }
}
