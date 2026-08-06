//! Shared workspace-mode policy for CLI, REPL, and MCP execution.

use kraken_workspace::{WorkspaceManifest, WorkspaceMode, Workspaces};

use super::Command;
use super::earn::EarnCommand;
use super::funding::WithdrawalCommand;
use super::futures::{FuturesCommand, FuturesTradingCommand};
use super::futures_paper::FuturesPaperCommand;
use super::streaming::StreamingCommand;
use super::subaccount::SubaccountCommand;
use super::trade::OrderCommand;
use super::ws::{MethodCommand, WsCommand};
use crate::cli::AppContext;
use crate::config;
use crate::errors::Result;

/// The execution context a command runs in, resolved from the active
/// workspace's contract.
pub(crate) enum ActiveMode {
    /// No workspace: the real Kraken account on the master credentials.
    Master,
    Paper(WorkspaceManifest),
    /// A named live workspace — contract-accepted, runtime-refused until
    /// scoped credentials land.
    Live(WorkspaceManifest),
}

/// The active workspace's contract, loaded fresh per call: the MCP server is
/// long-lived and must observe edits and deletions (`scoped_base` parity).
pub(crate) fn active_manifest(ctx: &AppContext) -> Result<Option<WorkspaceManifest>> {
    let Some(name) = ctx.workspace.as_deref() else {
        return Ok(None);
    };
    let name: kraken_workspace::WorkspaceName = name.parse()?;
    Ok(Some(
        Workspaces::new(config::config_dir()?).manifest(&name)?,
    ))
}

pub(crate) fn active_mode(ctx: &AppContext) -> Result<ActiveMode> {
    Ok(match active_manifest(ctx)? {
        None => ActiveMode::Master,
        Some(manifest) if manifest.mode == WorkspaceMode::Paper => ActiveMode::Paper(manifest),
        Some(manifest) => ActiveMode::Live(manifest),
    })
}

/// Refuse what the active workspace cannot honestly do. The refusal is
/// derived from the command shape FIRST, so unaffected commands never pay
/// a manifest read — and workspace lifecycle verbs (which address accounts
/// by name) can never be blocked by a stale ambient workspace.
pub(crate) fn enforce(ctx: &AppContext, command: &Command) -> Result<()> {
    if ctx.workspace.is_none() {
        return Ok(());
    }
    let refusal = classify(command);
    let Some(refusal) = refusal else {
        return Ok(());
    };
    let manifest = match active_mode(ctx)? {
        // A stale `default` alias never reaches here (normalized to None).
        ActiveMode::Master => return Ok(()),
        ActiveMode::Paper(manifest) | ActiveMode::Live(manifest) => manifest,
    };
    Err(match refusal {
        Refusal::NoPaperEquivalent(what) => {
            kraken_workspace::policy::unsupported(what, &manifest.name, manifest.mode).into()
        }
        Refusal::FuturesPaperMutation => kraken_workspace::WorkspaceError::FuturesUnscoped {
            workspace: manifest.name,
        }
        .into(),
    })
}

enum Refusal {
    NoPaperEquivalent(&'static str),
    FuturesPaperMutation,
}

fn classify(command: &Command) -> Option<Refusal> {
    if let Some(what) = no_paper_equivalent(command) {
        return Some(Refusal::NoPaperEquivalent(what));
    }
    if futures_paper_mutation(command) {
        return Some(Refusal::FuturesPaperMutation);
    }
    None
}

/// The permanent refusal set: money surfaces with no paper equivalent.
/// Matched by family with a `_ => None` floor; the pinned cross-check test
/// against the catalog's danger set forces conscious registration of every
/// new money command.
fn no_paper_equivalent(command: &Command) -> Option<&'static str> {
    match command {
        Command::Order(cmd) => unroutable_order(cmd),
        Command::Withdraw(_) => Some("withdraw"),
        Command::WalletTransfer(_) => Some("wallet-transfer"),
        Command::Withdrawal(WithdrawalCommand::Cancel(_)) => Some("withdrawal cancel"),
        Command::Earn(EarnCommand::Allocate(_)) => Some("earn allocate"),
        Command::Earn(EarnCommand::Deallocate(_)) => Some("earn deallocate"),
        Command::Subaccount(SubaccountCommand::Transfer(_)) => Some("subaccount transfer"),
        Command::Futures(FuturesCommand::Trading(cmd)) => futures_money(cmd),
        Command::Streaming(StreamingCommand::Ws(ws)) => ws_order_method(&ws.cmd),
        _ => None,
    }
}

fn futures_money(cmd: &FuturesTradingCommand) -> Option<&'static str> {
    match cmd {
        FuturesTradingCommand::Order(_) => Some("futures order"),
        FuturesTradingCommand::EditOrder(_) => Some("futures edit-order"),
        FuturesTradingCommand::Cancel(_) => Some("futures cancel"),
        FuturesTradingCommand::CancelAll(_) => Some("futures cancel-all"),
        FuturesTradingCommand::CancelAfter(_) => Some("futures cancel-after"),
        FuturesTradingCommand::BatchOrder(_) => Some("futures batch-order"),
        FuturesTradingCommand::Transfer(_) => Some("futures transfer"),
        FuturesTradingCommand::WalletTransfer(_) => Some("futures wallet-transfer"),
        FuturesTradingCommand::SetSubaccountStatus(_) => Some("futures set-subaccount-status"),
        _ => None,
    }
}

fn ws_order_method(cmd: &WsCommand) -> Option<&'static str> {
    let WsCommand::Method(method) = cmd else {
        return None;
    };
    match method {
        MethodCommand::AddOrder(_) => Some("ws add-order"),
        MethodCommand::AmendOrder(_) => Some("ws amend-order"),
        MethodCommand::CancelOrder(_) => Some("ws cancel-order"),
        MethodCommand::CancelAll => Some("ws cancel-all"),
        MethodCommand::CancelAfter(_) => Some("ws cancel-after"),
        MethodCommand::BatchAdd(_) => Some("ws batch-add"),
        MethodCommand::BatchCancel(_) => Some("ws batch-cancel"),
        MethodCommand::Ping => None,
    }
}

/// Futures paper state is one global snapshot: reads pass —
/// they are honest about what they show — but a mutation from inside a
/// workspace would corrupt state shared with every other scope.
fn futures_paper_mutation(command: &Command) -> bool {
    let Command::Futures(FuturesCommand::Paper(cmd)) = command else {
        return false;
    };
    !matches!(
        cmd,
        FuturesPaperCommand::Balance(_)
            | FuturesPaperCommand::Status(_)
            | FuturesPaperCommand::Orders(_)
            | FuturesPaperCommand::OrderStatus(_)
            | FuturesPaperCommand::Positions(_)
            | FuturesPaperCommand::Fills(_)
    )
}

/// The order verbs the paper engine cannot simulate. They call the venue
/// directly, so letting one through inside a workspace would be exactly the
/// silent-wrong-account failure this guard exists to remove. Buy, sell,
/// cancel, and cancel-all mode-route instead.
fn unroutable_order(cmd: &OrderCommand) -> Option<&'static str> {
    match cmd {
        OrderCommand::Amend(_) => Some("order amend"),
        OrderCommand::Edit(_) => Some("order edit"),
        OrderCommand::Batch(_) => Some("order batch"),
        OrderCommand::CancelBatch(_) => Some("order cancel-batch"),
        OrderCommand::CancelAfter(_) => Some("order cancel-after"),
        _ => None,
    }
}
