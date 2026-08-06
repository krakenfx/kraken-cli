/// Earn (staking) commands: allocate, deallocate, status, strategies.
use std::collections::HashMap;

use clap::Subcommand;
use enum_dispatch::enum_dispatch;

use super::Execute;
use crate::cli::AppContext;
use crate::errors::Result;
use crate::output::CommandOutput;

#[enum_dispatch(Execute)]
#[derive(Debug, Subcommand)]
pub(crate) enum EarnCommand {
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

#[derive(Debug, clap::Args)]
pub(crate) struct Allocate {
    /// Strategy ID.
    strategy_id: String,
    /// Amount to allocate.
    amount: String,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Deallocate {
    /// Strategy ID.
    strategy_id: String,
    /// Amount to deallocate.
    amount: String,
}

#[derive(Debug, clap::Args)]
pub(crate) struct AllocateStatus {
    /// Strategy ID.
    strategy_id: String,
}

#[derive(Debug, clap::Args)]
pub(crate) struct DeallocateStatus {
    /// Strategy ID.
    strategy_id: String,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Strategies {
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
}

#[derive(Debug, clap::Args)]
pub(crate) struct Allocations {
    /// Sort ascending (default: descending).
    #[arg(long)]
    ascending: bool,
    /// Secondary currency to express allocation value (default: USD).
    #[arg(long)]
    converted_asset: Option<String>,
    /// Omit strategies with zero allocations.
    #[arg(long)]
    hide_zero_allocations: bool,
}

/// Allocate and Deallocate differ only by endpoint and prompt wording; both
/// are destructive and gate on confirmation unless `--force`.
async fn change_allocation(
    ctx: &AppContext,
    endpoint: &str,
    prompt: &str,
    strategy_id: String,
    amount: String,
) -> Result<CommandOutput> {
    let (client, creds, otp) = ctx.spot_authed()?;
    ctx.confirm_destructive(prompt)?;
    let mut params = HashMap::new();
    params.insert("strategy_id".into(), strategy_id);
    params.insert("amount".into(), amount);
    let data = client
        .private_post(endpoint, params, creds, otp, false)
        .await?;
    Ok(CommandOutput::from_untyped(&data))
}

async fn fetch_allocation_status(
    ctx: &AppContext,
    endpoint: &str,
    strategy_id: String,
) -> Result<CommandOutput> {
    let (client, creds, otp) = ctx.spot_authed()?;
    let mut params = HashMap::new();
    params.insert("strategy_id".into(), strategy_id);
    let data = client
        .private_post(endpoint, params, creds, otp, true)
        .await?;
    Ok(CommandOutput::from_untyped(&data))
}

impl Execute for Allocate {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            strategy_id,
            amount,
        } = self;
        let prompt = format!("Allocate {amount} to earn strategy {strategy_id}?");
        change_allocation(ctx, "Earn/Allocate", &prompt, strategy_id, amount).await
    }
}

impl Execute for Deallocate {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            strategy_id,
            amount,
        } = self;
        let prompt = format!("Deallocate {amount} from earn strategy {strategy_id}?");
        change_allocation(ctx, "Earn/Deallocate", &prompt, strategy_id, amount).await
    }
}

impl Execute for AllocateStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        fetch_allocation_status(ctx, "Earn/AllocateStatus", self.strategy_id).await
    }
}

impl Execute for DeallocateStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        fetch_allocation_status(ctx, "Earn/DeallocateStatus", self.strategy_id).await
    }
}

impl Execute for Strategies {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            asset,
            lock_type,
            ascending,
            cursor,
            limit,
        } = self;
        let mut params = HashMap::new();
        if let Some(a) = asset {
            params.insert("asset".into(), a);
        }
        if !lock_type.is_empty() {
            params.insert(
                "lock_type".into(),
                serde_json::to_string(&lock_type).unwrap_or_default(),
            );
        }
        if ascending {
            params.insert("ascending".into(), "true".into());
        }
        if let Some(c) = cursor {
            params.insert("cursor".into(), c);
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }
        let data = client
            .private_post("Earn/Strategies", params, creds, otp, true)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

impl Execute for Allocations {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            ascending,
            converted_asset,
            hide_zero_allocations,
        } = self;
        let mut params = HashMap::new();
        if ascending {
            params.insert("ascending".into(), "true".into());
        }
        if let Some(ca) = converted_asset {
            params.insert("converted_asset".into(), ca);
        }
        if hide_zero_allocations {
            params.insert("hide_zero_allocations".into(), "true".into());
        }
        let data = client
            .private_post("Earn/Allocations", params, creds, otp, true)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}
