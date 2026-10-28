//! Earn (staking) commands: allocate, deallocate, status, strategies.

use super::{Execute, rest_bridge};
use crate::{cli::AppContext, errors::Result, output::CommandOutput};
use kraken_contract::earn::{
    Allocate, AllocateStatus, Allocations, Deallocate, DeallocateStatus, EarnCommand, Strategies,
};
use kraken_rest::spot::earn as rest;

impl Execute for Allocate {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            strategy_id,
            amount,
        } = self;
        let request = rest::Allocate {
            amount: rest_bridge::amount("amount", &amount)?,
            strategy_id,
        };
        let account = ctx.spot_account()?;
        ctx.confirm_destructive(&format!(
            "Allocate {amount} to earn strategy {}?",
            request.strategy_id
        ))?;
        let queued = account.sign(&request).await?;
        CommandOutput::published(&queued)
    }
}

impl Execute for Deallocate {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            strategy_id,
            amount,
        } = self;
        let request = rest::Deallocate {
            amount: rest_bridge::amount("amount", &amount)?,
            strategy_id,
        };
        let account = ctx.spot_account()?;
        ctx.confirm_destructive(&format!(
            "Deallocate {amount} from earn strategy {}?",
            request.strategy_id
        ))?;
        let queued = account.sign(&request).await?;
        CommandOutput::published(&queued)
    }
}

impl Execute for AllocateStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let query = rest::AllocateStatusQuery {
            strategy_id: self.strategy_id,
        };
        let status = ctx.spot_account()?.sign(&query).await?;
        CommandOutput::published(&status)
    }
}

impl Execute for DeallocateStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let query = rest::DeallocateStatusQuery {
            strategy_id: self.strategy_id,
        };
        let status = ctx.spot_account()?.sign(&query).await?;
        CommandOutput::published(&status)
    }
}

impl Execute for Strategies {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            asset,
            lock_type,
            ascending,
            cursor,
            limit,
        } = self;
        let query = rest::StrategiesQuery {
            asset,
            lock_type: lock_type
                .iter()
                .map(|kind| lock_kind(kind))
                .collect::<Result<_>>()?,
            ascending,
            cursor,
            limit,
        };
        let strategies = ctx.spot_account()?.sign(&query).await?;
        CommandOutput::published(&strategies)
    }
}

fn lock_kind(raw: &str) -> Result<rest::LockKind> {
    rest_bridge::token("--lock-type", raw, "flex, bonded, timed, instant")
}

impl Execute for Allocations {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            ascending,
            converted_asset,
            hide_zero_allocations,
        } = self;
        let query = rest::AllocationsQuery {
            ascending,
            converted_asset,
            hide_zero_allocations,
        };
        let allocations = ctx.spot_account()?.sign(&query).await?;
        CommandOutput::published(&allocations)
    }
}

delegate_execute!(EarnCommand {
    Allocate,
    Deallocate,
    AllocateStatus,
    DeallocateStatus,
    Strategies,
    Allocations
});
