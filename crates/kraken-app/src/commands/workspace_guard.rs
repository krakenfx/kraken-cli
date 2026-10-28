//! Shared workspace-mode policy for CLI, REPL, and MCP execution.

use crate::{cli::AppContext, config, errors::Result};
use chrono::{DateTime, Utc};
use kraken_contract::{
    command::Command,
    earn::EarnCommand,
    funding::WithdrawalCommand,
    futures::{FuturesCommand, FuturesTradingCommand},
    futures_paper::FuturesPaperCommand,
    lab::LabCommand,
    order::OrderCommand,
    paper::{PaperCommand, PaperTradeCommand},
    session::SessionCommand,
    streaming::StreamingCommand,
    subaccount::SubaccountCommand,
    ws::{MethodCommand, WsCommand},
};
use kraken_workspace::{WorkspaceManifest, WorkspaceMode, Workspaces};

/// The execution context a command runs in, resolved from the active
/// workspace's contract.
pub(crate) enum ActiveMode {
    /// No workspace: the real Kraken account on the master credentials.
    Master,
    Paper(WorkspaceManifest),
    /// A named allocation ledger over the configured real Kraken account.
    Live(WorkspaceManifest),
}

/// The active workspace's contract, loaded fresh per call: the MCP server is
/// long-lived and must observe edits and deletions (`scoped_base` parity).
pub fn active_manifest(ctx: &AppContext) -> Result<Option<WorkspaceManifest>> {
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

/// The instant this scope's fills stopped being simulated — the last
/// `FillSourceChanged → Venue` marker in its journal — or `None` when the
/// scope was never promoted.
pub(crate) fn live_since(scope: &std::path::Path) -> Result<Option<DateTime<Utc>>> {
    use kraken_paper::account::AccountRecord;
    use kraken_recording::{JsonlSource, Source};

    let journal = scope.join(kraken_workspace::JOURNAL_FILE);
    let Some(source) = JsonlSource::<AccountRecord>::open(journal) else {
        return Ok(None);
    };
    let mut live_since = None;
    for record in source.read()? {
        if matches!(
            record.event,
            kraken_paper::AccountEvent::FillSourceChanged {
                to: kraken_paper::FillSource::Venue,
                ..
            }
        ) {
            live_since = Some(record.ts);
        }
    }
    Ok(live_since)
}

/// True when a session window anchored before the scope went live: its fills
/// were simulated. The anchor is the window's end — or its start for a window
/// never sealed, because promotion refuses only *recording* sessions, so an
/// unsealed pre-promotion window is an aborted paper run, never current-era
/// evidence.
pub(crate) fn predates_live(
    window_anchor: Option<DateTime<Utc>>,
    live_since: Option<DateTime<Utc>>,
) -> bool {
    match (window_anchor, live_since) {
        (Some(anchor), Some(live)) => anchor <= live,
        _ => false,
    }
}

/// Stamp a single-session output of a live workspace with the money its fills
/// actually used: the live scope after promotion, the paper era before it.
/// `window_anchor` follows [`predates_live`]: the window's end, or its start
/// when never sealed.
pub(crate) fn stamp_session_money(
    out: &mut crate::output::CommandOutput,
    manifest: &WorkspaceManifest,
    scope: &std::path::Path,
    window_anchor: Option<DateTime<Utc>>,
) -> Result<()> {
    warn_live_workspace(manifest);
    if predates_live(window_anchor, live_since(scope)?) {
        out.stamp_paper_era_scope(&manifest.name);
    } else {
        out.stamp_live_scope(&manifest.name);
    }
    Ok(())
}

/// The stderr line on every live-workspace command.
///
/// Leads with the same pinned banner the no-workspace live path prints, so one
/// string means "real money" everywhere, then says what the workspace is. The
/// older copy led with "VIRTUAL", which in a CLI with a simulator in it read
/// as "not real".
pub(crate) fn warn_live_workspace(manifest: &WorkspaceManifest) {
    eprintln!("{}", super::order::LIVE_BANNER);
    eprintln!(
        "workspace '{}' is an allocation ledger over that account, not a separate balance",
        manifest.name
    );
}

/// The full explanation, printed at the two moments a user opts into real
/// money: `workspace create --mode live` and `workspace promote`.
///
/// The per-command line above is deliberately one line — it prints on every
/// command and would become noise. This prints once, where the decision is
/// actually being made, because "allocation" is the one word in this feature a
/// user cannot be expected to already understand. Takes the fields rather
/// than a manifest so `create` can print it BEFORE anything exists — the
/// explanation must precede the confirmation prompt, not follow the deed.
pub(crate) fn explain_live_workspace(name: &str, capital: rust_decimal::Decimal, currency: &str) {
    const RULE: &str = "────────────────────────────────────────────────────────────────────────";
    eprint!(
        "\
{RULE}
 LIVE WORKSPACE '{name}' — REAL MONEY on your Kraken account
{RULE}
 The commands do not change. `order buy`, `open-orders`, `balance`,
 `workspace status` and `workspace report` work exactly as they did on
 paper — but they now place and report REAL orders.

 The allocation: {capital} {currency} is this workspace's slice of the account.
 This CLI checks it before every order and computes the workspace's P&L
 against it, the same way paper capital works.

 Kraken does NOT enforce the allocation. Funds, permissions and positions
 stay shared with the whole account, so anything else using the same API
 key can spend this allocation. It is not a subaccount.

 Fills are the venue's own, reconciled by transaction ID — nothing here
 is simulated. Every command shows `backing`: the allocation set against
 your real balance. If it reads `over_allocated` — or `unverified`, when
 the real balance cannot be read — this workspace stops spending the
 allocation until the account covers it again.
 Closing a position always works, so you are never held in one.
{RULE}
",
    );
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
    if refusal.live_only() && manifest.mode == WorkspaceMode::Paper {
        return Ok(());
    }
    Err(match refusal {
        Refusal::NoPaperEquivalent(what) => {
            kraken_workspace::policy::unsupported(what, &manifest.name, manifest.mode).into()
        }
        Refusal::FuturesPaperMutation => kraken_workspace::WorkspaceError::FuturesUnscoped {
            workspace: manifest.name,
        }
        .into(),
        Refusal::SimulatedOnLiveLedger { what, live_verb } => {
            kraken_workspace::WorkspaceError::SimulationOnLiveLedger {
                what: what.to_string(),
                workspace: manifest.name,
                live_verb: live_verb.to_string(),
            }
            .into()
        }
        Refusal::NotReproducibleOnLiveLedger(what) => {
            kraken_workspace::WorkspaceError::NotReproducibleOnLiveLedger {
                what: what.to_string(),
                workspace: manifest.name,
            }
            .into()
        }
    })
}

enum Refusal {
    NoPaperEquivalent(&'static str),
    FuturesPaperMutation,
    /// A simulator verb aimed at a workspace whose ledger records real fills.
    /// Refused only in live mode — the same verb is the point in paper mode.
    SimulatedOnLiveLedger {
        what: &'static str,
        live_verb: &'static str,
    },
    /// A verb that settles fills from recorded prices, aimed at a ledger of
    /// real executions. Live-only for the same reason.
    NotReproducibleOnLiveLedger(&'static str),
}

impl Refusal {
    /// Whether only a ledger of real venue fills needs protecting from this.
    ///
    /// Both variants below it exist to keep invented numbers out of a live
    /// journal; in a paper workspace the very same verbs are what the workspace
    /// is for, so refusing them there breaks the paper→live path this guard
    /// exists to protect. Stated once, here, rather than per match arm: an arm
    /// that silently omits a mode reads as deliberate.
    fn live_only(&self) -> bool {
        matches!(
            self,
            Self::SimulatedOnLiveLedger { .. } | Self::NotReproducibleOnLiveLedger(_)
        )
    }
}

fn classify(command: &Command) -> Option<Refusal> {
    if let Some(what) = no_paper_equivalent(command) {
        return Some(Refusal::NoPaperEquivalent(what));
    }
    if futures_paper_mutation(command) {
        return Some(Refusal::FuturesPaperMutation);
    }
    simulated_verb(command).or_else(|| not_reproducible_verb(command))
}

/// What settles a fill from recorded prices: replaying a tape, and sealing a
/// plan that commits this workspace to replaying one later. A journal of real
/// venue executions is not a function of a tape.
///
/// Scoring is deliberately absent. `lab score`/`compare` read fills verbatim
/// out of a session's journal window and use its tape only to mark holdings, so
/// the evidence that justified a promotion still reads in the workspace it
/// promoted — which `workspace promote` itself relies on, grading its checklist
/// through `Lab::compare` on that same scope.
fn not_reproducible_verb(command: &Command) -> Option<Refusal> {
    let what = match command {
        Command::Session(SessionCommand::Start(args)) if args.is_replay() => "session start --from",
        // Not `lab new` as such: a live-only plan runs here fine. Only the
        // replay leg is unrunnable, and sealing is irreversible, so refuse at
        // the seal rather than stranding the plan at `lab next`.
        Command::Lab(LabCommand::New(args)) if args.plans_a_replay_leg() => "a sealed replay leg",
        _ => return None,
    };
    Some(Refusal::NotReproducibleOnLiveLedger(what))
}

/// The spot simulator's own verbs. Every one of them writes to the journal it
/// is pointed at — the reads too, because they reconcile resting orders into
/// simulated fills first — so against a live workspace they would forge fills
/// into a ledger that is supposed to hold only what the venue did.
/// `paper init/reset/status/balance` are excluded: they are aliases of the
/// `workspace` verbs and carry that family's own mode handling.
fn simulated_verb(command: &Command) -> Option<Refusal> {
    let Command::Paper(PaperCommand::Trade(cmd)) = command else {
        return None;
    };
    let (what, live_verb) = match cmd {
        PaperTradeCommand::Buy(_) => ("paper buy", "order buy"),
        PaperTradeCommand::Sell(_) => ("paper sell", "order sell"),
        PaperTradeCommand::Orders(_) => ("paper orders", "open-orders"),
        PaperTradeCommand::Cancel(_) => ("paper cancel", "order cancel"),
        PaperTradeCommand::CancelAll(_) => ("paper cancel-all", "order cancel-all"),
        PaperTradeCommand::History(_) => ("paper history", "trades-history"),
    };
    Some(Refusal::SimulatedOnLiveLedger { what, live_verb })
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
        MethodCommand::CancelAll(_) => Some("ws cancel-all"),
        MethodCommand::CancelAfter(_) => Some("ws cancel-after"),
        MethodCommand::BatchAdd(_) => Some("ws batch-add"),
        MethodCommand::BatchCancel(_) => Some("ws batch-cancel"),
        MethodCommand::Ping(_) => None,
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
