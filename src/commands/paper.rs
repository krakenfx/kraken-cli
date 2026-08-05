use std::collections::{HashMap, HashSet};
use std::path::Path;

use clap::Subcommand;
use enum_dispatch::enum_dispatch;
use kraken_core::OrderSide;
use kraken_paper::account::{Origin, PaperAccount};
use kraken_paper::{
    AccountEvent, BALANCE_DUST, CommandEntry, CommandOutcome, OrderRejection, PaperState,
    PaperTrade, parse_pair,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::{Value, json};

use super::Execute;
use crate::cli::AppContext;
use crate::client::SpotClient;
use crate::errors::{KrakenError, Result};
use crate::output::{CommandOutput, rounded};
use crate::session;
use crate::session::decision::DecisionKind;

#[derive(Debug, Subcommand)]
pub(crate) enum PaperCommand {
    /// Create AND fund a paper workspace — `kraken workspace create --mode
    /// paper` under another name.
    Init(Init),
    /// Return the workspace to its starting capital — `kraken workspace
    /// reset` under another name.
    Reset(Reset),
    /// Workspace balances — `kraken workspace balance` under another name.
    Balance(Balance),
    /// Workspace summary with P&L — `kraken workspace status` under another
    /// name.
    Status(Status),
    #[command(flatten)]
    Trade(PaperTradeCommand),
}

// The lifecycle verbs above are aliases kept for muscle memory: each Execute
// impl constructs the corresponding `workspace` command and runs it, so there
// is exactly one implementation. They live outside `Transact` deliberately —
// the workspace machinery opens the journal itself, and running it under the
// trade path's already-held writer lock would self-reject.

#[enum_dispatch(Transact)]
#[derive(Debug, Subcommand)]
pub(crate) enum PaperTradeCommand {
    /// Place a paper buy order.
    Buy(Buy),
    /// Place a paper sell order.
    Sell(Sell),
    /// Show open paper limit orders.
    Orders(Orders),
    /// Cancel a paper limit order.
    Cancel(Cancel),
    /// Cancel all paper limit orders.
    CancelAll(CancelAll),
    /// Show paper trade history.
    History(History),
}

#[derive(Debug, clap::Args)]
pub(crate) struct Init {
    /// Workspace name to create (defaults to the active workspace, else
    /// the global paper account).
    name: Option<String>,
    /// Starting capital (default: 10000).
    #[arg(long, alias = "balance", default_value = "10000")]
    capital: Decimal,
    /// Capital currency.
    #[arg(long, default_value = "USD")]
    currency: String,
    /// Fee rate as a decimal (default: 0.0026 = 0.26% Kraken Starter tier).
    #[arg(long)]
    fee_rate: Option<Decimal>,
    /// Slippage rate as a decimal (default: 0.0 = no slippage simulation).
    #[arg(long, alias = "slippage")]
    slippage_rate: Option<Decimal>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Reset {
    /// Workspace name (defaults to the active workspace, else the global
    /// paper account).
    name: Option<String>,
    /// Re-parameterize on reset: new starting capital (public-compatible
    /// alias: --balance).
    #[arg(long, alias = "balance")]
    capital: Option<Decimal>,
    /// New capital currency.
    #[arg(long)]
    currency: Option<String>,
    /// New fee rate as a decimal.
    #[arg(long)]
    fee_rate: Option<Decimal>,
    /// New slippage rate as a decimal.
    #[arg(long, alias = "slippage")]
    slippage_rate: Option<Decimal>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Balance {
    /// Workspace name (defaults to the active workspace, else the global
    /// paper account).
    name: Option<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Status {
    /// Workspace name (defaults to the active workspace, else the global
    /// paper account).
    name: Option<String>,
}

/// The conventional workspace behind the bare paper family — a normal
/// workspace, just implicitly named, so the global account stays a real
/// contract-carrying account.
pub(crate) const GLOBAL_PAPER_WORKSPACE: &str = "global";

/// The paper family always addresses a paper workspace — unlike the
/// `workspace` verbs it never falls back to the real account. With no name
/// and no active workspace it uses the global paper account, said out loud:
/// an implicit target must never be a silent one.
fn paper_target(name: Option<String>, ctx: &AppContext) -> String {
    name.or_else(|| ctx.workspace.clone()).unwrap_or_else(|| {
        warn_global_account();
        GLOBAL_PAPER_WORKSPACE.to_string()
    })
}

/// The contract whose pair policy governs this trade: the active workspace,
/// or the global paper account for bare invocations. Every paper trade now
/// carries a contract — the policy-free session accounts retired with runs.
fn trade_policy(ctx: &AppContext) -> Result<Option<kraken_workspace::WorkspaceManifest>> {
    let name = ctx.workspace.as_deref().unwrap_or(GLOBAL_PAPER_WORKSPACE);
    let name: kraken_workspace::WorkspaceName = name.parse()?;
    let workspaces = kraken_workspace::Workspaces::new(crate::config::config_dir()?);
    Ok(Some(workspaces.manifest(&name)?))
}

fn warn_global_account() {
    eprintln!(
        "note: no workspace set — using the global paper account \
         ('{GLOBAL_PAPER_WORKSPACE}'); name one or set KRAKEN_WORKSPACE"
    );
}

/// The scope root for bare paper trading: the global paper account's tree,
/// verified — trading never auto-creates an account, so an unfunded global
/// account refuses with the init hint instead of conjuring a journal.
fn global_paper_base() -> Result<std::path::PathBuf> {
    let base = crate::config::config_dir()?;
    let name: kraken_workspace::WorkspaceName = GLOBAL_PAPER_WORKSPACE.parse()?;
    if let Err(err) = kraken_workspace::Workspaces::new(&base).ensure_exists(&name) {
        return Err(match err {
            kraken_workspace::WorkspaceError::NotFound(_) => KrakenError::Validation(
                "No workspace set and no global paper account yet. Run 'kraken paper init' \
                 first, or set KRAKEN_WORKSPACE."
                    .into(),
            ),
            other => other.into(),
        });
    }
    warn_global_account();
    Ok(kraken_workspace::workspace_dir(&base, &name))
}

// --- The mode-routed surface: the order verbs run these same
// engine transactions under the workspace the guard resolved, so one
// implementation serves both vocabularies.

/// The order facts the routed verbs carry — a borrow-shaped mirror of the
/// paper `OrderRequest`, constructed from `trade.rs` args.
pub(crate) struct RoutedOrder<'a> {
    pub(crate) side: OrderSide,
    pub(crate) pair: &'a str,
    pub(crate) volume: &'a str,
    pub(crate) order_type: &'a str,
    pub(crate) price: Option<&'a str>,
    pub(crate) reason: Option<&'a str>,
}

fn routed_base(manifest: &kraken_workspace::WorkspaceManifest) -> Result<std::path::PathBuf> {
    let name: kraken_workspace::WorkspaceName = manifest.name.parse()?;
    Ok(kraken_workspace::workspace_dir(
        &crate::config::config_dir()?,
        &name,
    ))
}

fn routed_account(
    ctx: &AppContext,
    manifest: &kraken_workspace::WorkspaceManifest,
) -> Result<(std::path::PathBuf, PaperAccount)> {
    let base = routed_base(manifest)?;
    let account = PaperAccount::open_at(
        global_events_path(&base),
        Origin::from_mcp_mode(ctx.mcp_mode),
    )?;
    Ok((base, account))
}

fn routed_entry(order: &RoutedOrder<'_>) -> CommandEntry {
    CommandEntry {
        name: order.side.to_string(),
        pair: Some(order.pair.to_uppercase()),
        side: Some(order.side),
        volume: order.volume.parse().ok(),
        order_type: Some(order.order_type.to_lowercase()),
        price: order.price.and_then(|p| p.parse().ok()),
        ..CommandEntry::default()
    }
}

/// One routed order against a paper workspace: the same open →
/// place → failure-audit transaction the paper verbs run, returning the
/// typed placement for the caller's envelope.
pub(crate) async fn place_routed_order(
    ctx: &AppContext,
    manifest: &kraken_workspace::WorkspaceManifest,
    order: RoutedOrder<'_>,
) -> Result<PaperPlacement> {
    let (base, mut account) = routed_account(ctx, manifest)?;
    warn_when_no_active_session(&base);
    let request = PaperOrderRequest {
        side: order.side,
        pair: order.pair,
        volume: order.volume,
        order_type: order.order_type,
        price: order.price,
        reason: order.reason,
    };
    let entry = routed_entry(&order);
    // The scope's active session owns the pricing context: a replay session's trades
    // fill off its tape, never live REST.
    let mut venue = Venue::resolve(ctx, &base)?;
    let decisions = order.reason.map(|_| reason_target(&base)).transpose()?;
    let result = place_order_on_account(
        request,
        Some(manifest),
        decisions.as_deref(),
        &mut account,
        entry,
        &mut venue,
    )
    .await;
    // The same failure audit the paper verbs leave: a rejected order is an
    // event on the record, best-effort so it never shadows the error.
    if let Err(err) = &result {
        let mut entry = routed_entry(&order);
        entry.outcome = CommandOutcome::Err {
            category: err.category().to_string(),
        };
        let mut events = Vec::new();
        if let Ok(volume) = order.volume.parse::<Decimal>() {
            events.push(AccountEvent::OrderRejected(OrderRejection {
                side: order.side,
                pair: order.pair.to_uppercase(),
                volume,
                price: order.price.and_then(|p| p.parse().ok()),
                category: err.category().to_string(),
            }));
        }
        events.push(AccountEvent::Command(entry));
        account.audit(&account.stamp(events));
    }
    result
}

/// `--validate` in a paper workspace: the exact decide path — params, pair
/// policy, funds, market pricing — committing nothing.
pub(crate) async fn validate_routed_order(
    ctx: &AppContext,
    manifest: &kraken_workspace::WorkspaceManifest,
    order: RoutedOrder<'_>,
) -> Result<()> {
    let (_, account) = routed_account(ctx, manifest)?;
    let (normalized, base_asset, quote) = parse_pair(order.pair)?;
    kraken_workspace::policy::ensure_pair_allowed(manifest, &format!("{base_asset}/{quote}"))?;
    let volume: Decimal = order
        .volume
        .parse()
        .map_err(|_| KrakenError::Validation(format!("Invalid volume: {}", order.volume)))?;
    let state = account.state()?.clone();
    if validate_order_type(order.order_type)? {
        let price_val = order
            .price
            .ok_or_else(|| KrakenError::Validation("Limit orders require --price".into()))?;
        let price: Decimal = price_val
            .parse()
            .map_err(|_| KrakenError::Validation(format!("Invalid price: {price_val}")))?;
        state.decide_limit_order(order.side, order.pair, volume, price)?;
    } else {
        let venue = Venue::resolve(ctx, &routed_base(manifest)?)?;
        let (ask, bid) = venue.ticker_price(&normalized).await?;
        state.decide_market_order(order.side, order.pair, volume, ask, bid)?;
    }
    Ok(())
}

pub(crate) async fn cancel_on_workspace(
    ctx: &AppContext,
    manifest: &kraken_workspace::WorkspaceManifest,
    order_id: &str,
    reason: Option<&str>,
) -> Result<CommandOutput> {
    let (base, mut account) = routed_account(ctx, manifest)?;
    let entry = CommandEntry {
        name: "cancel".into(),
        order_id: Some(order_id.to_string()),
        ..CommandEntry::default()
    };
    let decisions = reason.map(|_| reason_target(&base)).transpose()?;
    execute_cancel(
        order_id,
        &mut account,
        entry,
        &mut Venue::resolve(ctx, &base)?,
        decisions.as_deref(),
        reason,
    )
    .await
}

pub(crate) async fn cancel_all_on_workspace(
    ctx: &AppContext,
    manifest: &kraken_workspace::WorkspaceManifest,
) -> Result<CommandOutput> {
    let (base, mut account) = routed_account(ctx, manifest)?;
    let entry = CommandEntry {
        name: "cancel_all".into(),
        ..CommandEntry::default()
    };
    execute_cancel_all(&mut account, entry, &mut Venue::resolve(ctx, &base)?).await
}

pub(crate) async fn orders_on_workspace(
    ctx: &AppContext,
    manifest: &kraken_workspace::WorkspaceManifest,
) -> Result<CommandOutput> {
    let (base, mut account) = routed_account(ctx, manifest)?;
    let entry = CommandEntry {
        name: "orders".into(),
        ..CommandEntry::default()
    };
    execute_orders(&mut account, entry, &mut Venue::resolve(ctx, &base)?).await
}

pub(crate) async fn history_on_workspace(
    ctx: &AppContext,
    manifest: &kraken_workspace::WorkspaceManifest,
) -> Result<CommandOutput> {
    let (base, mut account) = routed_account(ctx, manifest)?;
    let entry = CommandEntry {
        name: "history".into(),
        ..CommandEntry::default()
    };
    execute_history(&mut account, entry, &mut Venue::resolve(ctx, &base)?).await
}

impl Execute for Init {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        super::workspace::Create {
            name: paper_target(self.name, ctx),
            capital: self.capital,
            currency: self.currency,
            mode: kraken_workspace::WorkspaceMode::Paper,
            fee_rate: self.fee_rate,
            slippage_rate: self.slippage_rate,
            allow_pairs: None,
        }
        .execute(ctx)
        .await
    }
}

impl Execute for Reset {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        super::workspace::Reset {
            name: Some(paper_target(self.name, ctx)),
            capital: self.capital,
            currency: self.currency,
            fee_rate: self.fee_rate,
            slippage_rate: self.slippage_rate,
        }
        .execute(ctx)
        .await
    }
}

impl Execute for Balance {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        super::workspace::Balance {
            name: Some(paper_target(self.name, ctx)),
        }
        .execute(ctx)
        .await
    }
}

impl Execute for Status {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        super::workspace::Status {
            name: Some(paper_target(self.name, ctx)),
        }
        .execute(ctx)
        .await
    }
}

// The Args structs below carry no struct-level rustdoc on purpose: clap folds
// type-level docs into `--help`, and the about lines must keep coming from the
// enum variants alone so the help text stays exactly as it was.

// Buy and sell take the same order parameters, but `enum_dispatch` needs a
// distinct type per variant — so thin wrappers flatten this one definition.
#[derive(Debug, clap::Args)]
pub(crate) struct OrderRequest {
    /// Trading pair (e.g., BTCUSD).
    pair: String,
    /// Order volume.
    volume: String,
    /// Order type: market (default) or limit.
    #[arg(long, default_value = "market")]
    r#type: String,
    /// Limit price (required for limit orders).
    #[arg(long)]
    price: Option<String>,
    /// Rationale to log to the playground session's decision log (requires --session).
    #[arg(long)]
    reason: Option<String>,
}

impl OrderRequest {
    fn request(&self, side: OrderSide) -> PaperOrderRequest<'_> {
        PaperOrderRequest {
            side,
            pair: &self.pair,
            volume: &self.volume,
            order_type: &self.r#type,
            price: self.price.as_deref(),
            reason: self.reason.as_deref(),
        }
    }
}

/// Resolves the scoped journal; an unfunded flat account remains epoch-less.
pub(crate) fn global_events_path(base: &Path) -> std::path::PathBuf {
    base.join(kraken_workspace::JOURNAL_FILE)
}

#[derive(Debug, clap::Args)]
pub(crate) struct Buy {
    #[command(flatten)]
    trade: OrderRequest,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Sell {
    #[command(flatten)]
    trade: OrderRequest,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Orders {}

#[derive(Debug, clap::Args)]
pub(crate) struct Cancel {
    /// Order ID (e.g., PAPER-00001).
    order_id: String,
}

#[derive(Debug, clap::Args)]
pub(crate) struct CancelAll {}

#[derive(Debug, clap::Args)]
pub(crate) struct History {}

/// Where a paper command's prices come from. Live accounts reconcile and
/// value against public REST; a scope whose ACTIVE run replays a tape folds
/// that recorded source and never touches the network.
pub(crate) enum Venue<'a> {
    Rest(&'a SpotClient),
    Replay(Box<super::replay_venue::ReplayVenue>),
}

impl<'a> Venue<'a> {
    pub(crate) fn resolve(ctx: &'a AppContext, scope: &Path) -> Result<Self> {
        match super::replay_venue::ReplayVenue::resolve_active(scope)? {
            Some(replay) => Ok(Self::Replay(Box::new(replay))),
            None => Ok(Self::Rest(ctx.spot()?)),
        }
    }

    /// The venue for an account addressed by name from outside its scope —
    /// `workspace status <name>` marks the journal off live REST; only the
    /// scope's own commands ride an active replay session's tape.
    pub(crate) fn resolve_ambient(ctx: &'a AppContext) -> Result<Self> {
        Ok(Self::Rest(ctx.spot()?))
    }

    /// Live: best-effort REST reconcile — an offline machine must not fail a
    /// read command. Replay: the fold IS the venue; its failure means the
    /// source tape is unusable, which no command may hide.
    async fn reconcile(&mut self, account: &mut PaperAccount) -> Result<()> {
        match self {
            Self::Rest(client) => {
                reconcile_best_effort(account, client).await;
                Ok(())
            }
            Self::Replay(venue) => venue.fold_due(account),
        }
    }

    async fn ticker_price(&self, pair: &str) -> Result<(Decimal, Decimal)> {
        match self {
            Self::Rest(client) => fetch_ticker_price(client, pair).await,
            Self::Replay(venue) => venue.ticker_price(pair),
        }
    }

    async fn value(&self, state: &PaperState) -> (Decimal, bool) {
        match self {
            Self::Rest(client) => value_account(client, state).await,
            Self::Replay(venue) => venue.value(state),
        }
    }
}

pub(crate) async fn execute(cmd: &PaperTradeCommand, ctx: &AppContext) -> Result<CommandOutput> {
    // The scope root is resolved once here and threaded down: every path the
    // command touches (journal, decision logs, replay tapes) must live in the
    // same tree, so no handler may re-derive it.
    let ambient_global = ctx.workspace.is_none();
    let base = if ambient_global {
        global_paper_base()?
    } else {
        ctx.scoped_base()?
    };
    // Open the account once; the handle holds the log's writer lock for the
    // whole command, so read→decide→append is one transaction across processes.
    let mut account = PaperAccount::open_at(
        global_events_path(&base),
        Origin::from_mcp_mode(ctx.mcp_mode),
    )?;

    // Successful handlers append their own batches; the wrapper owns the
    // failure audit, best-effort so it can never shadow the command's error.
    let mut result = cmd.transact(&mut account, entry_for(cmd), ctx, &base).await;
    if let Err(err) = &result {
        let mut entry = entry_for(cmd);
        entry.outcome = CommandOutcome::Err {
            category: err.category().to_string(),
        };
        let mut events = Vec::new();
        if let Some(rejection) = rejection_for(cmd, err) {
            events.push(AccountEvent::OrderRejected(rejection));
        }
        events.push(AccountEvent::Command(entry));
        account.audit(&account.stamp(events));
    }
    // An implicit target is loud in the payload too, not just on stderr.
    if let (Ok(output), true) = (result.as_mut(), ambient_global) {
        output.stamp_workspace(GLOBAL_PAPER_WORKSPACE);
    }
    result
}

impl Execute for PaperCommand {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        match self {
            Self::Init(cmd) => cmd.execute(ctx).await,
            Self::Reset(cmd) => cmd.execute(ctx).await,
            Self::Balance(cmd) => cmd.execute(ctx).await,
            Self::Status(cmd) => cmd.execute(ctx).await,
            Self::Trade(cmd) => execute(&cmd, ctx).await,
        }
    }
}

/// Runs one command inside the account's single open transaction: [`execute`]
/// opens the account once (writer lock for the whole command), resolves the
/// scope root, and derives the audit entry before delegating here. Each impl
/// resolves its own [`Venue`] because only the commands that reconcile
/// against prices need one — `Init`/`Reset` stay fully offline on every venue.
#[enum_dispatch]
trait Transact {
    async fn transact(
        &self,
        account: &mut PaperAccount,
        entry: CommandEntry,
        ctx: &AppContext,
        base: &Path,
    ) -> Result<CommandOutput>;
}

impl Transact for Buy {
    async fn transact(
        &self,
        account: &mut PaperAccount,
        entry: CommandEntry,
        ctx: &AppContext,
        base: &Path,
    ) -> Result<CommandOutput> {
        execute_trade(
            self.trade.request(OrderSide::Buy),
            trade_policy(ctx)?.as_ref(),
            base,
            account,
            entry,
            &mut Venue::resolve(ctx, base)?,
        )
        .await
    }
}

impl Transact for Sell {
    async fn transact(
        &self,
        account: &mut PaperAccount,
        entry: CommandEntry,
        ctx: &AppContext,
        base: &Path,
    ) -> Result<CommandOutput> {
        execute_trade(
            self.trade.request(OrderSide::Sell),
            trade_policy(ctx)?.as_ref(),
            base,
            account,
            entry,
            &mut Venue::resolve(ctx, base)?,
        )
        .await
    }
}

impl Transact for Orders {
    async fn transact(
        &self,
        account: &mut PaperAccount,
        entry: CommandEntry,
        ctx: &AppContext,
        base: &Path,
    ) -> Result<CommandOutput> {
        execute_orders(account, entry, &mut Venue::resolve(ctx, base)?).await
    }
}

impl Transact for Cancel {
    async fn transact(
        &self,
        account: &mut PaperAccount,
        entry: CommandEntry,
        ctx: &AppContext,
        base: &Path,
    ) -> Result<CommandOutput> {
        execute_cancel(
            &self.order_id,
            account,
            entry,
            &mut Venue::resolve(ctx, base)?,
            None,
            None,
        )
        .await
    }
}

impl Transact for CancelAll {
    async fn transact(
        &self,
        account: &mut PaperAccount,
        entry: CommandEntry,
        ctx: &AppContext,
        base: &Path,
    ) -> Result<CommandOutput> {
        execute_cancel_all(account, entry, &mut Venue::resolve(ctx, base)?).await
    }
}

impl Transact for History {
    async fn transact(
        &self,
        account: &mut PaperAccount,
        entry: CommandEntry,
        ctx: &AppContext,
        base: &Path,
    ) -> Result<CommandOutput> {
        execute_history(account, entry, &mut Venue::resolve(ctx, base)?).await
    }
}

/// The audit entry for a command, prefilled from its typed args. The explicit
/// per-variant match is the structural allowlist: only these named fields can
/// ever reach the log, and free text (`--reason`) is deliberately not among
/// them.
fn entry_for(cmd: &PaperTradeCommand) -> CommandEntry {
    match cmd {
        PaperTradeCommand::Buy(Buy { trade }) => trade_entry(trade, OrderSide::Buy),
        PaperTradeCommand::Sell(Sell { trade }) => trade_entry(trade, OrderSide::Sell),
        PaperTradeCommand::Cancel(Cancel { order_id }) => CommandEntry {
            name: "cancel".into(),
            order_id: Some(order_id.clone()),
            ..CommandEntry::default()
        },
        PaperTradeCommand::CancelAll(CancelAll {}) => CommandEntry {
            name: "cancel_all".into(),
            ..CommandEntry::default()
        },
        PaperTradeCommand::Orders(Orders {}) => CommandEntry {
            name: "orders".into(),
            ..CommandEntry::default()
        },
        PaperTradeCommand::History(History {}) => CommandEntry {
            name: "history".into(),
            ..CommandEntry::default()
        },
    }
}

/// The audit entry for a buy/sell, side bound explicitly at the variant arm so
/// no future trade-like variant can inherit a guessed side.
fn trade_entry(trade: &OrderRequest, side: OrderSide) -> CommandEntry {
    // `..` skips exactly one field: `reason`, the free text kept off
    // the log by design.
    let OrderRequest {
        pair,
        volume,
        r#type,
        price,
        ..
    } = trade;
    CommandEntry {
        name: side.to_string(),
        pair: Some(pair.to_uppercase()),
        side: Some(side),
        volume: volume.parse().ok(),
        order_type: Some(r#type.to_lowercase()),
        price: price.as_deref().and_then(|p| p.parse().ok()),
        ..CommandEntry::default()
    }
}

/// The rejection event a failed buy/sell leaves on the record, when the
/// request was well-formed enough to describe one.
fn rejection_for(cmd: &PaperTradeCommand, err: &KrakenError) -> Option<OrderRejection> {
    let (trade, side) = match cmd {
        PaperTradeCommand::Buy(Buy { trade }) => (trade, OrderSide::Buy),
        PaperTradeCommand::Sell(Sell { trade }) => (trade, OrderSide::Sell),
        _ => return None,
    };
    Some(OrderRejection {
        side,
        pair: trade.pair.to_uppercase(),
        volume: trade.volume.parse().ok()?,
        price: trade.price.as_deref().and_then(|p| p.parse().ok()),
        category: err.category().to_string(),
    })
}

async fn reconcile_best_effort(account: &mut PaperAccount, client: &SpotClient) -> Vec<PaperTrade> {
    match reconcile_and_persist(account, client).await {
        Ok(fills) => fills,
        Err(err) => {
            tracing::warn!(%err, "could not sync pending orders");
            Vec::new()
        }
    }
}

/// Append a read-only command's audit entry, best-effort: observation must
/// never fail the command it observes.
fn audit_read(account: &mut PaperAccount, state: &PaperState, mut entry: CommandEntry) {
    entry.balances = Some(state.balances.clone());
    account.audit(&account.stamp(vec![AccountEvent::Command(entry)]));
}

fn validate_order_type(order_type_str: &str) -> Result<bool> {
    if order_type_str.eq_ignore_ascii_case("limit") {
        Ok(true)
    } else if order_type_str.eq_ignore_ascii_case("market") {
        Ok(false)
    } else {
        Err(KrakenError::Validation(format!(
            "Invalid order type '{order_type_str}'. Must be 'market' or 'limit'."
        )))
    }
}

/// A spot paper order as the user expressed it on the command line, before
/// parsing or validation — one value for the `Buy`/`Sell` impls to build.
pub(crate) struct PaperOrderRequest<'a> {
    side: OrderSide,
    pair: &'a str,
    volume: &'a str,
    order_type: &'a str,
    price: Option<&'a str>,
    /// Rationale to record in the session's decision log, if given.
    reason: Option<&'a str>,
}

/// A free function, not `From`: both types now live in other crates, so the
/// orphan rule forbids the impl — and the mapping is command-layer policy
/// anyway (which decision a trade records is this front-end's call).
fn decision_kind(side: OrderSide) -> DecisionKind {
    match side {
        OrderSide::Buy => DecisionKind::Buy,
        OrderSide::Sell => DecisionKind::Sell,
    }
}

/// Where a rationale lands in this scope: the ACTIVE session's own decision log
/// when one is recording — the log `read_window` joins to fills — else
/// beside the journal, disclosed as run-less rationale. Never refuses: a
/// reason always has somewhere honest to go.
pub(crate) fn reason_target(scope: &Path) -> Result<std::path::PathBuf> {
    match kraken_workspace::session::active(scope)? {
        Some((session_id, _)) => Ok(kraken_session::session::session_decisions_path(
            scope,
            session_id.ordinal(),
        )),
        None => Ok(scope.join(kraken_workspace::DECISIONS_FILE)),
    }
}

/// A paper buy/sell with no session recording lands on the account but inside no
/// session window, so `run show`, `explain pnl`, and `lab score` will never
/// attribute it. Warn — a trade meant for a session must not be silently orphaned
/// onto a stopped or aborted one. Advisory only: trading the account without a
/// run is a supported flow, so this never refuses.
fn warn_when_no_active_session(scope: &Path) {
    if let Ok(None) = kraken_workspace::session::active(scope) {
        tracing::warn!(
            "no active session; this trade is recorded on the account but not in any session window — \
             start one with 'kraken session start' to capture it"
        );
    }
}

/// Decision logging for the routed live path (the real account's scope is
/// the config root), keyed by the venue txid.
pub(crate) fn log_routed_decision(
    side: OrderSide,
    pair: &str,
    order_id: &str,
    reason: &str,
) -> Result<()> {
    let scope = crate::config::config_dir()?;
    log_decision_at(&reason_target(&scope)?, side, pair, order_id, reason)
}

fn log_decision_at(
    path: &Path,
    side: OrderSide,
    pair: &str,
    order_id: &str,
    reason: &str,
) -> Result<()> {
    session::decision::append(
        path,
        decision_kind(side),
        Some(pair.to_string()),
        reason.to_string(),
        Some(order_id.to_string()),
    )?;
    Ok(())
}

/// A cancel's rationale (`DecisionKind::Cancel`), keyed by the cancelled
/// order's id and pair — the analogue of [`log_decision_at`] for pulling a
/// resting order.
fn log_cancel_at(path: &Path, pair: &str, order_id: &str, reason: &str) -> Result<()> {
    session::decision::append(
        path,
        DecisionKind::Cancel,
        Some(pair.to_string()),
        reason.to_string(),
        Some(order_id.to_string()),
    )?;
    Ok(())
}

/// Decision logging for a cancel on the routed live path (the real account's
/// scope is the config root). A master cancel names a txid, not a pair, so the
/// symbol is unknown.
pub(crate) fn log_routed_cancel_decision(order_id: &str, reason: &str) -> Result<()> {
    let scope = crate::config::config_dir()?;
    session::decision::append(
        &reason_target(&scope)?,
        DecisionKind::Cancel,
        None,
        reason.to_string(),
        Some(order_id.to_string()),
    )?;
    Ok(())
}

/// The typed outcome of one order placed on the engine — the shared
/// transaction result behind the paper verbs and the mode-routed order
/// verbs. The caller chooses the envelope.
pub(crate) enum PaperPlacement {
    Limit { order: kraken_paper::PaperOrder },
    Market { trade: PaperTrade },
}

/// Decide, commit, and decision-log one order against an open account: the
/// engine transaction both trading surfaces share. Rendering stays with the
/// caller, so the paper verbs keep their receipts byte-identical while the
/// routed verbs wrap the same facts in the unified envelope.
pub(crate) async fn place_order_on_account(
    request: PaperOrderRequest<'_>,
    policy: Option<&kraken_workspace::WorkspaceManifest>,
    decisions: Option<&Path>,
    account: &mut PaperAccount,
    mut entry: CommandEntry,
    venue: &mut Venue<'_>,
) -> Result<PaperPlacement> {
    let PaperOrderRequest {
        side,
        pair,
        volume: volume_str,
        order_type: order_type_str,
        price: price_str,
        reason,
    } = request;

    // Account permission policy: a denied pair fails before any
    // pricing or engine work, in the engine's own pair grammar so the check
    // can never disagree with fills.
    if let Some(manifest) = policy {
        let (_, base_asset, quote) = parse_pair(pair)?;
        kraken_workspace::policy::ensure_pair_allowed(manifest, &format!("{base_asset}/{quote}"))?;
    }

    venue.reconcile(account).await?;

    let volume: Decimal = volume_str
        .parse()
        .map_err(|_| KrakenError::Validation(format!("Invalid volume: {volume_str}")))?;

    let is_limit = validate_order_type(order_type_str)?;

    if is_limit {
        let price_val = price_str
            .ok_or_else(|| KrakenError::Validation("Limit orders require --price".into()))?;
        let price: Decimal = price_val
            .parse()
            .map_err(|_| KrakenError::Validation(format!("Invalid price: {price_val}")))?;

        let order = account
            .state()?
            .decide_limit_order(side, pair, volume, price)?;
        let order_id = order.id.clone();
        entry.outcome = CommandOutcome::Ok {
            order_ids: vec![order_id.clone()],
            trade_ids: Vec::new(),
        };
        account.commit(
            vec![AccountEvent::OrderSubmitted {
                order: order.clone(),
            }],
            entry,
        )?;

        if let (Some(path), Some(reason)) = (decisions, reason) {
            // Best-effort: the order is already durably logged, and reporting
            // a placed order as an error would invite a duplicate retry.
            if let Err(e) = log_decision_at(path, side, pair, &order_id, reason) {
                tracing::warn!(order_id = %order_id, error = %e, "limit order placed but decision-log append failed");
            }
        }
        Ok(PaperPlacement::Limit { order })
    } else {
        let (normalized, _, _) = parse_pair(pair)?;
        let (ask, bid) = venue.ticker_price(&normalized).await?;
        let trade = account
            .state()?
            .decide_market_order(side, pair, volume, ask, bid)?;
        entry.outcome = CommandOutcome::Ok {
            order_ids: vec![trade.order_id.clone()],
            trade_ids: vec![trade.id.clone()],
        };
        account.commit(
            vec![AccountEvent::OrderFilled {
                trade: trade.clone(),
            }],
            entry,
        )?;

        if let (Some(path), Some(reason)) = (decisions, reason) {
            // Best-effort: the fill is already durably logged, and reporting
            // a placed order as an error would invite a duplicate retry.
            if let Err(e) = log_decision_at(path, side, pair, &trade.order_id, reason) {
                tracing::warn!(order_id = %trade.order_id, error = %e, "fill logged but decision-log append failed");
            }
        }
        Ok(PaperPlacement::Market { trade })
    }
}

async fn execute_trade(
    request: PaperOrderRequest<'_>,
    policy: Option<&kraken_workspace::WorkspaceManifest>,
    base: &Path,
    account: &mut PaperAccount,
    entry: CommandEntry,
    venue: &mut Venue<'_>,
) -> Result<CommandOutput> {
    let side = request.side;
    warn_when_no_active_session(base);
    // Resolve the decision target up front, so a failing run probe surfaces
    // before a trade executes whose rationale would then be orphaned.
    let decisions = request.reason.map(|_| reason_target(base)).transpose()?;
    let placement =
        place_order_on_account(request, policy, decisions.as_deref(), account, entry, venue)
            .await?;
    let mut out = match placement {
        PaperPlacement::Limit { order } => {
            let pairs = vec![
                ("Mode".into(), "[PAPER] Simulated Trading".into()),
                ("Action".into(), format!("Limit {side} order placed")),
                ("Order ID".into(), order.id.clone()),
                ("Pair".into(), order.pair.clone()),
                ("Volume".into(), format!("{:.8}", rounded(order.volume, 8))),
                ("Price".into(), format!("{:.2}", rounded(order.price, 2))),
            ];
            CommandOutput::key_value(
                pairs,
                paper_json(json!({
                    "action": "limit_order_placed",
                    "order_id": order.id,
                    "side": side,
                    "pair": order.pair,
                    "volume": order.volume,
                    "price": order.price,
                })),
            )
        }
        PaperPlacement::Market { trade } => {
            let pairs = vec![
                ("Mode".into(), "[PAPER] Simulated Trading".into()),
                ("Action".into(), format!("Market {side} executed")),
                ("Trade ID".into(), trade.id.clone()),
                ("Pair".into(), trade.pair.clone()),
                ("Volume".into(), format!("{:.8}", rounded(trade.volume, 8))),
                ("Price".into(), format!("{:.2}", rounded(trade.price, 2))),
                ("Fee".into(), format!("{:.4}", rounded(trade.fee, 4))),
                ("Cost".into(), format!("{:.2}", rounded(trade.cost, 2))),
            ];
            CommandOutput::key_value(
                pairs,
                paper_json(json!({
                    "action": "market_order_filled",
                    "trade_id": trade.id,
                    "order_id": trade.order_id,
                    "side": side,
                    "pair": trade.pair,
                    "volume": trade.volume,
                    "price": trade.price,
                    "fee": trade.fee,
                    "cost": trade.cost,
                })),
            )
        }
    };
    // Show where the fill landed — the same Run/Workspace rows `explain pnl`
    // stamps: the active session (or `none` when the trade is outside any session),
    // and the workspace account it hit.
    let run = match kraken_workspace::session::active(base) {
        Ok(Some((session_id, _))) => session_id.to_string(),
        _ => "none".to_string(),
    };
    out.stamp_session(&run);
    if let Some(manifest) = policy {
        out.stamp_workspace(&manifest.name);
    }
    Ok(out)
}

async fn execute_orders(
    account: &mut PaperAccount,
    entry: CommandEntry,
    venue: &mut Venue<'_>,
) -> Result<CommandOutput> {
    venue.reconcile(account).await?;
    let state = account.state()?.clone();
    audit_read(account, &state, entry);

    let headers = vec![
        "[PAPER] Order ID".into(),
        "Pair".into(),
        "OrderSide".into(),
        "Type".into(),
        "Volume".into(),
        "Price".into(),
        "Reserved".into(),
        "Created".into(),
    ];

    let rows: Vec<Vec<String>> = if state.open_orders.is_empty() {
        vec![vec![
            "—".into(),
            "No open orders".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
        ]]
    } else {
        state
            .open_orders
            .iter()
            .map(|o| {
                vec![
                    o.id.clone(),
                    o.pair.clone(),
                    format!("{}", o.side),
                    format!("{}", o.order_type),
                    format!("{:.8}", rounded(o.volume, 8)),
                    format!("{:.2}", rounded(o.price, 2)),
                    format!("{:.8} {}", rounded(o.reserved_amount, 8), o.reserved_asset),
                    kraken_recording::format_instant(o.created_at),
                ]
            })
            .collect()
    };

    let json_orders: Vec<Value> = state
        .open_orders
        .iter()
        .map(|o| {
            json!({
                "id": o.id,
                "pair": o.pair,
                "side": o.side,
                "type": o.order_type,
                "volume": o.volume,
                "price": o.price,
                "reserved_amount": o.reserved_amount,
                "reserved_asset": o.reserved_asset,
                "created_at": o.created_at,
            })
        })
        .collect();

    Ok(CommandOutput::new(
        paper_json(json!({
            "open_orders": json_orders,
            "count": state.open_orders.len(),
        })),
        headers,
        rows,
    ))
}

async fn execute_cancel(
    order_id: &str,
    account: &mut PaperAccount,
    mut entry: CommandEntry,
    venue: &mut Venue<'_>,
    decisions: Option<&Path>,
    reason: Option<&str>,
) -> Result<CommandOutput> {
    venue.reconcile(account).await?;
    let order = account.state()?.decide_cancel(order_id)?;
    entry.outcome = CommandOutcome::Ok {
        order_ids: vec![order.id.clone()],
        trade_ids: Vec::new(),
    };
    account.commit(
        vec![AccountEvent::OrderCancelled {
            order: order.clone(),
        }],
        entry,
    )?;

    if let (Some(path), Some(reason)) = (decisions, reason) {
        // Best-effort, matching the order path: the cancel is already durably
        // committed, and reporting it as an error would invite a retry.
        if let Err(e) = log_cancel_at(path, &order.pair, &order.id, reason) {
            tracing::warn!(order_id = %order.id, error = %e, "order cancelled but decision-log append failed");
        }
    }

    let pairs = vec![
        ("Mode".into(), "[PAPER] Simulated Trading".into()),
        ("Action".into(), "Order cancelled".into()),
        ("Order ID".into(), order.id.clone()),
        ("Pair".into(), order.pair.clone()),
        ("OrderSide".into(), format!("{}", order.side)),
        (
            "Released".into(),
            format!(
                "{:.8} {}",
                rounded(order.reserved_amount, 8),
                order.reserved_asset
            ),
        ),
    ];

    Ok(CommandOutput::key_value(
        pairs,
        paper_json(json!({
            "action": "order_cancelled",
            "order_id": order.id,
            "pair": order.pair,
            "side": order.side,
            "released_amount": order.reserved_amount,
            "released_asset": order.reserved_asset,
        })),
    ))
}

async fn execute_cancel_all(
    account: &mut PaperAccount,
    mut entry: CommandEntry,
    venue: &mut Venue<'_>,
) -> Result<CommandOutput> {
    venue.reconcile(account).await?;
    let cancelled = account.state()?.open_orders.clone();
    entry.outcome = CommandOutcome::Ok {
        order_ids: cancelled.iter().map(|o| o.id.clone()).collect(),
        trade_ids: Vec::new(),
    };
    let events = cancelled
        .iter()
        .map(|order| AccountEvent::OrderCancelled {
            order: order.clone(),
        })
        .collect();
    account.commit(events, entry)?;

    let count = cancelled.len();
    let pairs = vec![
        ("Mode".into(), "[PAPER] Simulated Trading".into()),
        ("Action".into(), "All orders cancelled".into()),
        ("Cancelled".into(), count.to_string()),
    ];

    Ok(CommandOutput::key_value(
        pairs,
        paper_json(json!({
            "action": "all_orders_cancelled",
            "cancelled_count": count,
        })),
    ))
}

async fn execute_history(
    account: &mut PaperAccount,
    entry: CommandEntry,
    venue: &mut Venue<'_>,
) -> Result<CommandOutput> {
    venue.reconcile(account).await?;
    let state = account.state()?.clone();
    audit_read(account, &state, entry);

    let headers = vec![
        "[PAPER] ID".into(),
        "Order ID".into(),
        "Status".into(),
        "Pair".into(),
        "OrderSide".into(),
        "Volume".into(),
        "Price".into(),
        "Fee".into(),
        "Cost".into(),
        "Time".into(),
    ];

    let has_fills = !state.filled_trades.is_empty();
    let has_cancels = !state.cancelled_orders.is_empty();

    let mut rows: Vec<Vec<String>> = Vec::new();

    for t in &state.filled_trades {
        rows.push(vec![
            t.id.clone(),
            t.order_id.clone(),
            "Filled".into(),
            t.pair.clone(),
            format!("{}", t.side),
            format!("{:.8}", rounded(t.volume, 8)),
            format!("{:.2}", rounded(t.price, 2)),
            format!("{:.4}", rounded(t.fee, 4)),
            format!("{:.2}", rounded(t.cost, 2)),
            kraken_recording::format_instant(t.filled_at),
        ]);
    }

    for o in &state.cancelled_orders {
        rows.push(vec![
            o.id.clone(),
            "—".into(),
            "Cancelled".into(),
            o.pair.clone(),
            format!("{}", o.side),
            format!("{:.8}", rounded(o.volume, 8)),
            format!("{:.2}", rounded(o.price, 2)),
            "—".into(),
            "—".into(),
            kraken_recording::format_instant(o.created_at),
        ]);
    }

    if !has_fills && !has_cancels {
        rows.push(vec![
            "—".into(),
            "—".into(),
            "No history yet".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
        ]);
    }

    let json_trades: Vec<Value> = state
        .filled_trades
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "order_id": t.order_id,
                "status": "filled",
                "pair": t.pair,
                "side": t.side,
                "volume": t.volume,
                "price": t.price,
                "fee": t.fee,
                "cost": t.cost,
                "time": t.filled_at,
            })
        })
        .collect();

    let json_cancelled: Vec<Value> = state
        .cancelled_orders
        .iter()
        .map(|o| {
            json!({
                "id": o.id,
                "status": "cancelled",
                "pair": o.pair,
                "side": o.side,
                "volume": o.volume,
                "price": o.price,
                "time": o.created_at,
            })
        })
        .collect();

    Ok(CommandOutput::new(
        paper_json(json!({
            "trades": json_trades,
            "cancelled": json_cancelled,
            "filled_count": state.filled_trades.len(),
            "cancelled_count": state.cancelled_orders.len(),
        })),
        headers,
        rows,
    ))
}

pub(crate) async fn execute_status(
    account: &mut PaperAccount,
    entry: CommandEntry,
    venue: &mut Venue<'_>,
) -> Result<CommandOutput> {
    venue.reconcile(account).await?;
    let state = account.state()?.clone();
    audit_read(account, &state, entry);

    let sc = state.starting_currency.clone();
    let (portfolio_value, valuation_complete) = venue.value(&state).await;

    let pnl = portfolio_value - state.starting_balance;
    let pnl_pct = if state.starting_balance > Decimal::ZERO {
        (pnl / state.starting_balance) * dec!(100)
    } else {
        Decimal::ZERO
    };

    let partial_marker = if valuation_complete { "" } else { " (partial)" };

    let pairs = vec![
        ("Mode".into(), "[PAPER] Simulated Trading".into()),
        (
            "Starting Balance".into(),
            format!("{:.2} {sc}", rounded(state.starting_balance, 2)),
        ),
        (
            "Current Value".into(),
            format!("{:.2} {sc}{partial_marker}", rounded(portfolio_value, 2)),
        ),
        (
            "Unrealized P&L".into(),
            format!(
                "{:+.2} {sc} ({:+.2}%){partial_marker}",
                rounded(pnl, 2),
                rounded(pnl_pct, 2)
            ),
        ),
        (
            "Fee Rate".into(),
            format!("{:.2}%", rounded(state.fee_rate * dec!(100), 2)),
        ),
        (
            "Slippage Rate".into(),
            format!("{:.2}%", rounded(state.slippage_rate * dec!(100), 2)),
        ),
        ("Total Trades".into(), state.filled_trades.len().to_string()),
        ("Open Orders".into(), state.open_orders.len().to_string()),
        (
            "Assets Held".into(),
            state
                .balances
                .iter()
                .filter(|&(_, &v)| v > BALANCE_DUST)
                .count()
                .to_string(),
        ),
    ];

    Ok(CommandOutput::key_value(
        pairs,
        paper_json(json!({
            "starting_balance": state.starting_balance,
            "starting_currency": sc,
            "current_value": portfolio_value,
            "unrealized_pnl": pnl,
            "unrealized_pnl_pct": pnl_pct,
            "valuation_complete": valuation_complete,
            "fee_rate": state.fee_rate,
            "slippage_rate": state.slippage_rate,
            "total_trades": state.filled_trades.len(),
            "open_orders": state.open_orders.len(),
        })),
    ))
}

/// Value a paper account at current market prices, returning
/// `(portfolio_value, complete)` in the starting currency. Best-effort: a
/// failed price fetch falls back to the cash balance with `complete = false`.
/// Shared by `paper status` and `playground stop`/`status`.
pub(crate) async fn value_account(client: &SpotClient, state: &PaperState) -> (Decimal, bool) {
    let sc = &state.starting_currency;
    let val_pairs: Vec<String> = state
        .balances
        .iter()
        .filter(|&(a, &amt)| a != sc && amt > BALANCE_DUST)
        .map(|(a, _)| format!("{a}{sc}"))
        .collect();
    if val_pairs.is_empty() {
        return (
            state.balances.get(sc).copied().unwrap_or(Decimal::ZERO),
            true,
        );
    }
    match fetch_ticker_prices(client, &val_pairs).await {
        Ok(prices) => state.compute_portfolio_value(&prices),
        Err(err) => {
            tracing::warn!(%err, "could not fetch prices for valuation");
            (
                state.balances.get(sc).copied().unwrap_or(Decimal::ZERO),
                false,
            )
        }
    }
}

fn paper_json(mut data: Value) -> Value {
    if let Some(obj) = data.as_object_mut() {
        obj.insert("mode".to_string(), json!("paper"));
    }
    data
}

async fn fetch_ticker_price(client: &SpotClient, pair: &str) -> Result<(Decimal, Decimal)> {
    let data = client.public_get("Ticker", &[("pair", pair)]).await?;
    let obj = data
        .as_object()
        .ok_or_else(|| KrakenError::Parse(format!("Unexpected ticker response for {pair}")))?;
    let (_, val) = obj
        .iter()
        .next()
        .ok_or_else(|| KrakenError::Parse(format!("Empty ticker response for {pair}")))?;
    let ask = extract_price(val, "a", pair)?;
    let bid = extract_price(val, "b", pair)?;
    Ok((ask, bid))
}

fn extract_price(val: &Value, field: &str, pair: &str) -> Result<Decimal> {
    val.get(field)
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| KrakenError::Parse(format!("Missing {field} price in ticker for {pair}")))?
        .parse::<Decimal>()
        .map_err(|e| KrakenError::Parse(format!("Invalid {field} price in ticker for {pair}: {e}")))
}

async fn fetch_ticker_prices(
    client: &SpotClient,
    pairs: &[String],
) -> Result<HashMap<String, (Decimal, Decimal)>> {
    if pairs.is_empty() {
        return Ok(HashMap::new());
    }
    let pair_param = pairs.join(",");
    let data = client
        .public_get("Ticker", &[("pair", pair_param.as_str())])
        .await?;
    let obj = data
        .as_object()
        .ok_or_else(|| KrakenError::Parse("Unexpected ticker response format".into()))?;

    let mut response_prices: Vec<(String, String, Decimal, Decimal)> = Vec::new();
    for (key, val) in obj {
        let ask = extract_price(val, "a", key)?;
        let bid = extract_price(val, "b", key)?;
        if let Ok((_, base, quote)) = parse_pair(key) {
            response_prices.push((base, quote, ask, bid));
        }
    }

    let mut prices = HashMap::new();
    for input_pair in pairs {
        let (_, input_base, input_quote) = parse_pair(input_pair)?;
        let matched = response_prices
            .iter()
            .find(|(b, q, _, _)| *b == input_base && *q == input_quote);
        if let Some((_, _, ask, bid)) = matched {
            prices.insert(input_pair.clone(), (*ask, *bid));
        } else {
            return Err(KrakenError::Parse(format!(
                "No ticker data for {input_pair} in batch response"
            )));
        }
    }
    Ok(prices)
}

async fn reconcile_and_persist(
    account: &mut PaperAccount,
    client: &SpotClient,
) -> Result<Vec<PaperTrade>> {
    // Nothing to reconcile before an epoch — and "not initialized" belongs to
    // the handler's state gate, not to a best-effort sync warning.
    if !account.is_initialized() {
        return Ok(Vec::new());
    }
    let draft = account.state()?;
    if draft.open_orders.is_empty() {
        return Ok(Vec::new());
    }
    let unique_pairs: Vec<String> = draft
        .open_orders
        .iter()
        .map(|o| o.pair.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let prices = fetch_ticker_prices(client, &unique_pairs).await?;
    let fills = draft.decide_pending_fills(&prices);
    if fills.is_empty() {
        return Ok(fills);
    }
    // Log before folding: a fill enters the account's state only once it is
    // durable (append advances the projection after the fsync). On append
    // failure the orders simply stay open and reconcile retries later.
    let records = account.stamp(
        fills
            .iter()
            .map(|trade| AccountEvent::OrderFilled {
                trade: trade.clone(),
            })
            .collect(),
    );
    account.append(&records)?;
    tracing::debug!(filled = fills.len(), "limit orders filled");
    Ok(fills)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_price_valid() {
        let val = json!({
            "a": ["50000.00", "1", "1.000"],
            "b": ["49999.00", "1", "1.000"],
        });
        let ask = extract_price(&val, "a", "BTCUSD").unwrap();
        let bid = extract_price(&val, "b", "BTCUSD").unwrap();
        assert!((ask - dec!(50000.0)).abs() < dec!(0.0000000001));
        assert!((bid - dec!(49999.0)).abs() < dec!(0.0000000001));
    }

    #[test]
    fn test_extract_price_missing_field() {
        let val = json!({ "b": ["49999.00"] });
        let result = extract_price(&val, "a", "BTCUSD");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing a price"));
    }

    #[test]
    fn test_extract_price_non_numeric() {
        let val = json!({ "a": ["not_a_number"] });
        let result = extract_price(&val, "a", "BTCUSD");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid a price"));
    }

    #[test]
    fn test_extract_price_empty_array() {
        let val = json!({ "a": [] });
        let result = extract_price(&val, "a", "BTCUSD");
        assert!(result.is_err());
    }

    #[test]
    fn test_paper_json_injects_mode() {
        let data = paper_json(json!({"foo": "bar"}));
        assert_eq!(data.get("mode").and_then(|v| v.as_str()), Some("paper"));
        assert_eq!(data.get("foo").and_then(|v| v.as_str()), Some("bar"));
    }

    #[test]
    fn test_validate_order_type_rejects_garbage() {
        let result = validate_order_type("garbage");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Invalid order type"),
            "error must describe the problem"
        );
        assert!(
            err.contains("garbage"),
            "error must include the invalid value"
        );
        assert!(err.contains("market"), "error must list valid values");
        assert!(err.contains("limit"), "error must list valid values");
    }

    #[test]
    fn test_validate_order_type_accepts_valid() {
        assert!(!validate_order_type("market").unwrap());
        assert!(validate_order_type("limit").unwrap());
        assert!(!validate_order_type("MARKET").unwrap());
        assert!(validate_order_type("Limit").unwrap());
        assert!(validate_order_type("LIMIT").unwrap());
        assert!(!validate_order_type("Market").unwrap());
    }

    #[test]
    fn non_finite_args_never_reach_the_log() {
        // "inf"/"NaN" were representable as f64 and needed an explicit gate;
        // `Decimal` refuses them at parse time, so the log stays loadable by
        // construction.
        assert!("inf".parse::<Decimal>().is_err());
        assert!("NaN".parse::<Decimal>().is_err());
        assert_eq!("0.5".parse::<Decimal>().ok(), Some(dec!(0.5)));

        let cmd = PaperTradeCommand::Buy(Buy {
            trade: OrderRequest {
                pair: "BTCUSD".into(),
                volume: "inf".into(),
                r#type: "market".into(),
                price: None,
                reason: None,
            },
        });
        let err = KrakenError::Validation("volume".into());
        assert!(rejection_for(&cmd, &err).is_none());
        assert_eq!(entry_for(&cmd).volume, None);
    }

    #[test]
    fn audit_entry_side_matches_the_variant() {
        // Regression: the side was once inferred as "sell" for any non-Buy
        // variant; it is now bound at each variant arm. The lowercase tokens
        // are the journal contract.
        let trade = || OrderRequest {
            pair: "BTCUSD".into(),
            volume: "0.5".into(),
            r#type: "market".into(),
            price: None,
            reason: None,
        };
        let buy = entry_for(&PaperTradeCommand::Buy(Buy { trade: trade() }));
        assert_eq!(buy.side, Some(OrderSide::Buy));
        assert_eq!(buy.name, "buy");
        let sell = entry_for(&PaperTradeCommand::Sell(Sell { trade: trade() }));
        assert_eq!(sell.side, Some(OrderSide::Sell));
        assert_eq!(sell.name, "sell");

        let err = KrakenError::Validation("x".into());
        let rejection = rejection_for(&PaperTradeCommand::Sell(Sell { trade: trade() }), &err)
            .expect("well-formed sell describes a rejection");
        assert_eq!(rejection.side, OrderSide::Sell);
    }

    #[test]
    fn test_status_partial_marker_formatting() {
        let valuation_complete_false = false;
        let partial_marker = if valuation_complete_false {
            ""
        } else {
            " (partial)"
        };
        assert_eq!(partial_marker, " (partial)");

        let valuation_complete_true = true;
        let no_marker = if valuation_complete_true {
            ""
        } else {
            " (partial)"
        };
        assert_eq!(no_marker, "");

        let mode_label = "[PAPER] Simulated Trading";
        assert!(mode_label.starts_with("[PAPER]"));
    }
}
