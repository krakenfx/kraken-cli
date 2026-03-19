/// Earn (staking) commands: allocate, deallocate, status, strategies.
use std::collections::HashMap;

use clap::Subcommand;

use super::helpers::{confirm_destructive, parse_generic};
use crate::client::SpotClient;
use crate::config::SpotCredentials;
use crate::errors::Result;
use crate::output::CommandOutput;

#[derive(Debug, Subcommand)]
pub(crate) enum EarnCommand {
    /// Allocate funds to an earn strategy.
    Allocate {
        /// Strategy ID.
        strategy_id: String,
        /// Amount to allocate.
        amount: String,
    },
    /// Deallocate funds from an earn strategy.
    Deallocate {
        /// Strategy ID.
        strategy_id: String,
        /// Amount to deallocate.
        amount: String,
    },
    /// Check allocation status.
    AllocateStatus {
        /// Strategy ID.
        strategy_id: String,
    },
    /// Check deallocation status.
    DeallocateStatus {
        /// Strategy ID.
        strategy_id: String,
    },
    /// List available earn strategies.
    Strategies {
        /// Filter by asset.
        #[arg(long)]
        asset: Option<String>,
        /// Filter by lock type (flex, bonded, timed, instant). Can specify multiple.
        #[arg(long)]
        lock_type: Vec<String>,
        /// Sort ascending (default: descending).
        #[arg(long)]
        ascending: bool,
        /// Cursor for next page of results.
        #[arg(long)]
        cursor: Option<String>,
        /// Number of items per page.
        #[arg(long)]
        limit: Option<u16>,
    },
    /// Get current allocations.
    Allocations {
        /// Sort ascending (default: descending).
        #[arg(long)]
        ascending: bool,
        /// Secondary currency to express allocation value (default: USD).
        #[arg(long)]
        converted_asset: Option<String>,
        /// Omit strategies with zero allocations.
        #[arg(long)]
        hide_zero_allocations: bool,
    },
}

pub(crate) async fn execute(
    cmd: &EarnCommand,
    client: &SpotClient,
    creds: &SpotCredentials,
    otp: Option<&str>,
    force: bool,
    verbose: bool,
) -> Result<CommandOutput> {
    match cmd {
        EarnCommand::Allocate {
            strategy_id,
            amount,
        } => {
            if !force {
                confirm_destructive(&format!(
                    "Allocate {amount} to earn strategy {strategy_id}?"
                ))?;
            }
            let mut params = HashMap::new();
            params.insert("strategy_id".into(), strategy_id.clone());
            params.insert("amount".into(), amount.clone());
            let data = client
                .private_post("Earn/Allocate", params, creds, otp, false, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        EarnCommand::Deallocate {
            strategy_id,
            amount,
        } => {
            if !force {
                confirm_destructive(&format!(
                    "Deallocate {amount} from earn strategy {strategy_id}?"
                ))?;
            }
            let mut params = HashMap::new();
            params.insert("strategy_id".into(), strategy_id.clone());
            params.insert("amount".into(), amount.clone());
            let data = client
                .private_post("Earn/Deallocate", params, creds, otp, false, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        EarnCommand::AllocateStatus { strategy_id } => {
            let mut params = HashMap::new();
            params.insert("strategy_id".into(), strategy_id.clone());
            let data = client
                .private_post("Earn/AllocateStatus", params, creds, otp, true, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        EarnCommand::DeallocateStatus { strategy_id } => {
            let mut params = HashMap::new();
            params.insert("strategy_id".into(), strategy_id.clone());
            let data = client
                .private_post("Earn/DeallocateStatus", params, creds, otp, true, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        EarnCommand::Strategies {
            asset,
            lock_type,
            ascending,
            cursor,
            limit,
        } => {
            let mut params = HashMap::new();
            if let Some(a) = asset {
                params.insert("asset".into(), a.clone());
            }
            if !lock_type.is_empty() {
                params.insert(
                    "lock_type".into(),
                    serde_json::to_string(lock_type).unwrap_or_default(),
                );
            }
            if *ascending {
                params.insert("ascending".into(), "true".into());
            }
            if let Some(c) = cursor {
                params.insert("cursor".into(), c.clone());
            }
            if let Some(l) = limit {
                params.insert("limit".into(), l.to_string());
            }
            let data = client
                .private_post("Earn/Strategies", params, creds, otp, true, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        EarnCommand::Allocations {
            ascending,
            converted_asset,
            hide_zero_allocations,
        } => {
            let mut params = HashMap::new();
            if *ascending {
                params.insert("ascending".into(), "true".into());
            }
            if let Some(ca) = converted_asset {
                params.insert("converted_asset".into(), ca.clone());
            }
            if *hide_zero_allocations {
                params.insert("hide_zero_allocations".into(), "true".into());
            }
            let data = client
                .private_post("Earn/Allocations", params, creds, otp, true, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
    }
}
