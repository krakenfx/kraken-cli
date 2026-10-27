use super::{Execute, rest_bridge};
use crate::{
    cli::AppContext,
    errors::{KrakenError, Result},
    output::{CommandOutput, rounded},
    session,
    session::decision::DecisionKind,
};
use kraken_contract::paper::{
    Balance, Buy, Cancel, CancelAll, History, Init, OrderRequest, Orders, PaperCommand,
    PaperTradeCommand, Reset, Sell, Status,
};
use kraken_core::OrderSide;
use kraken_paper::{
    AccountEvent, BALANCE_DUST, CommandEntry, CommandOutcome, FillSource, OrderRejection,
    PaperState, PaperTrade,
    account::{Origin, PaperAccount},
    parse_pair,
};
use kraken_rest::spot::SpotRest;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

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
        "note: no workspace set — using the global paper account ('{GLOBAL_PAPER_WORKSPACE}'); \
         name one or set KRAKEN_WORKSPACE"
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
                "No workspace set and no global paper account yet. Run 'kraken paper init' first, \
                 or set KRAKEN_WORKSPACE."
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
    // The scope's active session owns the pricing context: a replay session's
    // trades fill off its tape, never live REST.
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

/// A workspace's open orders. `FillSource::Venue` reads a live workspace's
/// reconciled mirror, so the simulator never gets to layer a fill on top.
pub(crate) async fn orders_on_workspace(
    ctx: &AppContext,
    manifest: &kraken_workspace::WorkspaceManifest,
    source: FillSource,
) -> Result<CommandOutput> {
    let (base, mut account) = routed_account(ctx, manifest)?;
    let entry = CommandEntry {
        name: "orders".into(),
        ..CommandEntry::default()
    };
    execute_orders(
        &mut account,
        entry,
        &mut Venue::resolve(ctx, &base)?,
        source,
    )
    .await
}

/// A workspace's realised trades. Under `FillSource::Venue` every row is a real
/// execution reconciliation folded in, so nothing here was invented locally.
pub(crate) async fn history_on_workspace(
    ctx: &AppContext,
    manifest: &kraken_workspace::WorkspaceManifest,
    source: FillSource,
) -> Result<CommandOutput> {
    let (base, mut account) = routed_account(ctx, manifest)?;
    let entry = CommandEntry {
        name: "history".into(),
        ..CommandEntry::default()
    };
    execute_history(
        &mut account,
        entry,
        &mut Venue::resolve(ctx, &base)?,
        source,
    )
    .await
}

/// The header marker and `Mode` row this ledger's tables carry.
///
/// Derived from the fill source rather than patched in afterwards: a live
/// workspace shares these renderers, and a real venue order printed under
/// `[PAPER]` is the worst mislabel this CLI can produce.
fn marker(source: FillSource) -> &'static str {
    crate::output::mode_marker(source.into())
}

fn mode_label(source: FillSource) -> &'static str {
    crate::output::mode_label(source.into())
}

/// Whether the simulator's own reconcile pass — resting orders settled against
/// market data — may run before a read. It may not against a venue ledger:
/// those fills arrive by transaction ID from `journal_mirror`.
async fn reconcile_if_simulated(
    account: &mut PaperAccount,
    venue: &mut Venue<'_>,
    source: FillSource,
) -> Result<()> {
    match source {
        FillSource::Simulated => venue.reconcile(account).await,
        FillSource::Venue => Ok(()),
    }
}

/// Build the engine request for `order`. A free function rather than an
/// inherent method: `OrderRequest` is `kraken_contract`'s type now.
fn paper_request(order: &OrderRequest, side: OrderSide) -> PaperOrderRequest<'_> {
    PaperOrderRequest {
        side,
        pair: &order.pair,
        volume: &order.volume,
        order_type: &order.r#type,
        price: order.price.as_deref(),
        reason: order.reason.as_deref(),
    }
}

impl Execute for Init {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        kraken_contract::workspace::Create {
            name: paper_target(self.name, ctx),
            capital: self.capital,
            currency: self.currency,
            mode: kraken_contract::domain_values::WorkspaceMode::Paper,
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
        kraken_contract::workspace::Reset {
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
        kraken_contract::workspace::Balance {
            name: Some(paper_target(self.name, ctx)),
        }
        .execute(ctx)
        .await
    }
}

impl Execute for Status {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        kraken_contract::workspace::Status {
            name: Some(paper_target(self.name, ctx)),
        }
        .execute(ctx)
        .await
    }
}

/// Resolves the scoped journal; an unfunded flat account remains epoch-less.
pub(crate) fn global_events_path(base: &Path) -> std::path::PathBuf {
    base.join(kraken_workspace::JOURNAL_FILE)
}

/// Where a paper command's prices come from. Live accounts reconcile and
/// value against public REST; a scope whose ACTIVE run replays a tape folds
/// that recorded source and never touches the network.
pub(crate) enum Venue<'a> {
    Rest(&'a SpotRest),
    Replay(Box<super::replay_venue::ReplayVenue>),
}

impl<'a> Venue<'a> {
    pub(crate) fn resolve(ctx: &'a AppContext, scope: &Path) -> Result<Self> {
        match super::replay_venue::ReplayVenue::resolve_active(scope)? {
            Some(replay) => Ok(Self::Replay(Box::new(replay))),
            None => Ok(Self::Rest(ctx.spot())),
        }
    }

    /// The venue for an account addressed by name from outside its scope —
    /// `workspace status <name>` marks the journal off live REST; only the
    /// scope's own commands ride an active replay session's tape.
    pub(crate) fn resolve_ambient(ctx: &'a AppContext) -> Result<Self> {
        Ok(Self::Rest(ctx.spot()))
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
            paper_request(&self.trade, OrderSide::Buy),
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
            paper_request(&self.trade, OrderSide::Sell),
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
        // Always simulated: `paper orders` is refused against a live ledger.
        execute_orders(
            account,
            entry,
            &mut Venue::resolve(ctx, base)?,
            FillSource::Simulated,
        )
        .await
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
        // Always simulated: `paper history` is refused against a live ledger.
        execute_history(
            account,
            entry,
            &mut Venue::resolve(ctx, base)?,
            FillSource::Simulated,
        )
        .await
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

async fn reconcile_best_effort(account: &mut PaperAccount, client: &SpotRest) -> Vec<PaperTrade> {
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
fn decision_for_side(side: OrderSide) -> DecisionKind {
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

/// A paper buy/sell with no session recording lands on the account but inside
/// no session window, so `run show`, `explain pnl`, and `lab score` will never
/// attribute it. Warn — a trade meant for a session must not be silently
/// orphaned onto a stopped or aborted one. Advisory only: trading the account
/// without a run is a supported flow, so this never refuses.
fn warn_when_no_active_session(scope: &Path) {
    if let Ok(None) = kraken_workspace::session::active(scope) {
        tracing::warn!(
            "no active session; this trade is recorded on the account but not in any session \
             window — start one with 'kraken session start' to capture it"
        );
    }
}

pub(crate) fn log_routed_decision_at(
    scope: &Path,
    side: OrderSide,
    pair: &str,
    order_id: &str,
    reason: &str,
) -> Result<()> {
    log_decision_at(&reason_target(scope)?, side, pair, order_id, reason)
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
        decision_for_side(side),
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

/// A routed live cancel names a transaction ID but no pair.
pub(crate) fn log_routed_cancel_decision_at(
    scope: &Path,
    order_id: &str,
    reason: &str,
) -> Result<()> {
    session::decision::append(
        &reason_target(scope)?,
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
    source: FillSource,
) -> Result<CommandOutput> {
    reconcile_if_simulated(account, venue, source).await?;
    let state = account.state()?.clone();
    audit_read(account, &state, entry);

    let headers = vec![
        format!("{} Order ID", marker(source)),
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
                    // The workspace's own commitment either way — the simulator
                    // debits it, the mirror books it from the resting order's
                    // notional. Kraken's separate hold against the account is
                    // not this figure and is not knowable here; `backing` is
                    // where the account-wide picture is reported.
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
        ledger_json(
            json!({
                "open_orders": json_orders,
                "count": state.open_orders.len(),
            }),
            source,
        ),
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
    source: FillSource,
) -> Result<CommandOutput> {
    reconcile_if_simulated(account, venue, source).await?;
    let state = account.state()?.clone();
    audit_read(account, &state, entry);

    let headers = vec![
        format!("{} ID", marker(source)),
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
        ledger_json(
            json!({
                "trades": json_trades,
                "cancelled": json_cancelled,
                "filled_count": state.filled_trades.len(),
                "cancelled_count": state.cancelled_orders.len(),
            }),
            source,
        ),
        headers,
        rows,
    ))
}

/// Value a ledger. Under `FillSource::Venue` price movement is not turned into
/// simulated fills — a live workspace's fills come from reconciliation.
pub(crate) async fn execute_status(
    account: &mut PaperAccount,
    entry: CommandEntry,
    venue: &mut Venue<'_>,
    source: FillSource,
) -> Result<CommandOutput> {
    reconcile_if_simulated(account, venue, source).await?;
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

    let opening = match source {
        FillSource::Simulated => "Starting Balance",
        // Not a balance the venue holds: it is what this workspace claims of
        // the account, and every P&L below is measured against it.
        FillSource::Venue => "Allocation",
    };
    let mut pairs = vec![
        ("Mode".into(), mode_label(source).into()),
        (
            opening.into(),
            format!("{:.2} {sc}", rounded(state.starting_balance, 2)),
        ),
        (
            "Current Value".into(),
            format!("{:.2} {sc}{partial_marker}", rounded(portfolio_value, 2)),
        ),
        (
            "Unrealized P&L".into(),
            format!(
                "{} {sc} ({}%){partial_marker}",
                crate::output::signed_or_sub_resolution(pnl, 2),
                crate::output::signed_or_sub_resolution(pnl_pct, 2)
            ),
        ),
    ];
    // The fee and slippage rates are the simulator's fill model. A live ledger's
    // fills carry the venue's own fee, so printing the contract's rates there
    // would read as a promise about real executions.
    match source {
        FillSource::Simulated => pairs.extend([
            (
                "Fee Rate".to_string(),
                format!("{:.2}%", rounded(state.fee_rate * dec!(100), 2)),
            ),
            (
                "Slippage Rate".to_string(),
                format!("{:.2}%", rounded(state.slippage_rate * dec!(100), 2)),
            ),
        ]),
        FillSource::Venue => pairs.push((
            "Fees".to_string(),
            "Kraken's own, charged per fill".to_string(),
        )),
    }
    pairs.extend([
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
    ]);

    Ok(CommandOutput::key_value(
        pairs,
        ledger_json(
            json!({
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
            }),
            source,
        ),
    ))
}

/// Value a paper account at current market prices, returning
/// `(portfolio_value, complete)` in the starting currency. Best-effort: a
/// failed price fetch falls back to the cash balance with `complete = false`.
/// Shared by `paper status` and `playground stop`/`status`.
pub(crate) async fn value_account(client: &SpotRest, state: &PaperState) -> (Decimal, bool) {
    let sc = &state.starting_currency;
    let assets: Vec<&str> = state
        .balances
        .iter()
        .filter(|&(a, &amt)| a != sc && amt > BALANCE_DUST)
        .map(|(a, _)| a.as_str())
        .collect();
    if assets.is_empty() {
        return (
            state.balances.get(sc).copied().unwrap_or(Decimal::ZERO),
            true,
        );
    }
    let direct: Vec<String> = assets.iter().map(|a| format!("{a}{sc}")).collect();
    let prices = match fetch_ticker_prices(client, &direct).await {
        Ok(prices) => prices,
        Err(err) => {
            // One unlisted pair errors the whole batch, so fall back to asset
            // by asset rather than blanking the valuation. Whatever cannot be
            // marked is reported unmarked, never as zero. Concurrently: these
            // are public reads, and a serial pass would cost two round-trips
            // per asset held.
            tracing::debug!(%err, "batch valuation failed; marking asset by asset");
            futures_util::future::join_all(
                assets
                    .iter()
                    .map(|&asset| async move { (asset, resolve_mark(client, asset, sc).await) }),
            )
            .await
            .into_iter()
            .filter_map(|(asset, mark)| Some((format!("{asset}{sc}"), mark?)))
            .collect()
        }
    };
    state.compute_portfolio_value(&prices)
}

/// The `(ask, bid)` mark for `asset` in `quote`, from whichever way round the
/// venue lists the market.
///
/// A workspace denominated in BTC holds USD as soon as it sells, and Kraken
/// lists XBTUSD, not USDXBT. Without inverting the quoted pair that USD stays
/// unmarked and the workspace reports P&L on its BTC leg alone.
async fn resolve_mark(client: &SpotRest, asset: &str, quote: &str) -> Option<(Decimal, Decimal)> {
    if let Ok(direct) = fetch_ticker_price(client, &format!("{asset}{quote}")).await {
        return Some(direct);
    }
    let (ask, bid) = fetch_ticker_price(client, &format!("{quote}{asset}"))
        .await
        .ok()?;
    // Turning `asset` into `quote` means buying `quote` on that market, which
    // fills at its ask — so the conservative mark for the holding is 1/ask.
    Some((invert(bid)?, invert(ask)?))
}

fn invert(price: Decimal) -> Option<Decimal> {
    Decimal::ONE
        .checked_div(price)
        .filter(|_| price > Decimal::ZERO)
}

/// Stamp the `mode` every paper payload carries. The simulator's own verbs are
/// refused against a live workspace, so their payloads are always simulated.
fn paper_json(data: Value) -> Value {
    ledger_json(data, FillSource::Simulated)
}

/// As [`paper_json`], for the read verbs a live workspace shares.
fn ledger_json(mut data: Value, source: FillSource) -> Value {
    if let Some(obj) = data.as_object_mut() {
        let mode = match source {
            FillSource::Simulated => "paper",
            FillSource::Venue => "live",
        };
        obj.insert("mode".to_string(), json!(mode));
    }
    data
}

/// `(ask, bid)` for one pair.
pub(crate) async fn fetch_ticker_price(
    client: &SpotRest,
    pair: &str,
) -> Result<(Decimal, Decimal)> {
    let tickers = client
        .query(&rest_bridge::ticker_query([pair], None))
        .await?;
    let ticker = tickers
        .values()
        .next()
        .ok_or_else(|| KrakenError::Parse(format!("Empty ticker response for {pair}")))?;
    Ok((ticker.ask_price, ticker.bid_price))
}

async fn fetch_ticker_prices(
    client: &SpotRest,
    pairs: &[String],
) -> Result<HashMap<String, (Decimal, Decimal)>> {
    if pairs.is_empty() {
        return Ok(HashMap::new());
    }
    let tickers = client
        .query(&rest_bridge::ticker_query(pairs, None))
        .await?;
    let response_prices: Vec<(String, String, Decimal, Decimal)> = tickers
        .iter()
        .filter_map(|(key, ticker)| {
            let (_, base, quote) = parse_pair(key).ok()?;
            Some((base, quote, ticker.ask_price, ticker.bid_price))
        })
        .collect();

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
    client: &SpotRest,
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

/// `Transact` delegation for the moved enum — same reason as
/// `delegate_execute!`: the type is `kraken_contract`'s, so a derive cannot
/// reach it.
impl Transact for PaperTradeCommand {
    async fn transact(
        &self,
        account: &mut PaperAccount,
        entry: CommandEntry,
        ctx: &AppContext,
        base: &Path,
    ) -> Result<CommandOutput> {
        match self {
            Self::Buy(cmd) => cmd.transact(account, entry, ctx, base).await,
            Self::Sell(cmd) => cmd.transact(account, entry, ctx, base).await,
            Self::Orders(cmd) => cmd.transact(account, entry, ctx, base).await,
            Self::Cancel(cmd) => cmd.transact(account, entry, ctx, base).await,
            Self::CancelAll(cmd) => cmd.transact(account, entry, ctx, base).await,
            Self::History(cmd) => cmd.transact(account, entry, ctx, base).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mark for a holding is what selling it would fetch, so inverting a
    /// market quoted the other way round has to cross the spread the same
    /// direction: the holding's bid comes from the quoted pair's ask.
    #[test]
    fn an_inverted_mark_crosses_the_spread_the_same_way() {
        let (ask, bid) = (dec!(64100), dec!(64000));
        let inverted_bid = invert(ask).unwrap();
        let inverted_ask = invert(bid).unwrap();
        assert!(
            inverted_bid < inverted_ask,
            "the inverted bid must stay below the inverted ask: {inverted_bid} / {inverted_ask}"
        );
        // 1 USD marked in BTC against a 64100 ask.
        assert_eq!(inverted_bid, dec!(1) / dec!(64100));
    }

    /// A zero or negative quote has no reciprocal; it must be reported unmarked
    /// rather than valued at infinity.
    #[test]
    fn a_nonpositive_quote_has_no_inverse() {
        assert!(invert(dec!(0)).is_none());
        assert!(invert(dec!(-5)).is_none());
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

    /// A real venue order must never render under a `[PAPER]` header. The
    /// renderers a live workspace shares are the ones that used to hardcode it,
    /// so pin that every label the fill source drives moves together.
    #[test]
    fn a_venue_ledger_never_labels_itself_as_paper() {
        for (source, expected) in [
            (FillSource::Simulated, "[PAPER]"),
            (FillSource::Venue, "[LIVE]"),
        ] {
            assert_eq!(marker(source), expected);
            assert!(
                mode_label(source).starts_with(expected),
                "the Mode row and the header marker must agree: {}",
                mode_label(source)
            );
        }
        assert_eq!(
            ledger_json(json!({}), FillSource::Venue)["mode"],
            json!("live"),
            "the JSON payload cannot report a venue read as paper"
        );
    }
}
