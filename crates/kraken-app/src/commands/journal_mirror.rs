//! Local evidence mirror for live venue activity.
//!
//! The venue remains authoritative; transaction-ID reconciliation repairs
//! missed local observations.

use super::rest_bridge;
use crate::{cli::AppContext, config, errors::Result, output::CommandOutput};
use chrono::{DateTime, Utc};
use kraken_core::OrderSide;
use kraken_paper::{
    AccountEvent, PaperOrder, PaperOrderType, VenueSnapshot,
    account::{Origin, PaperAccount},
    parse_pair,
};
use kraken_rest::{
    Id,
    spot::{
        OrderStatus,
        account::{BalanceQuery, Order, QueriedOrders, QueryOrdersQuery},
    },
};
use rust_decimal::Decimal;
use std::collections::BTreeMap;

/// The real account's journal: `<config_dir>/journal.jsonl` — the same
/// file name a workspace journal carries, at the unscoped root.
pub(crate) fn master_journal_path() -> Result<std::path::PathBuf> {
    Ok(config::config_dir()?.join(kraken_workspace::JOURNAL_FILE))
}

/// Open the mirror, anchoring a fresh journal with the venue's balance
/// snapshot. Opens (and locks) BEFORE the venue call by design: a busy
/// journal refuses before any money moves, and the writer lock serializes
/// concurrent trades. The anchor fetch is best-effort — an offline snapshot
/// is disclosed as incomplete, never guessed and never blocking.
pub(crate) async fn open_attached(ctx: &AppContext) -> Result<PaperAccount> {
    open_attached_at(master_journal_path()?, ctx).await
}

/// [`open_attached`] with a caller-chosen journal — the seam unit tests
/// drive, since the real path is fixed under the user's config dir.
pub(crate) async fn open_attached_at(
    journal: std::path::PathBuf,
    ctx: &AppContext,
) -> Result<PaperAccount> {
    let mut account = PaperAccount::open_at(journal, Origin::from_mcp_mode(ctx.mcp_mode))?;
    if !account.is_initialized() {
        let snapshot = fetch_snapshot(ctx).await;
        let records = account.stamp(vec![AccountEvent::Attached(snapshot)]);
        account.append_if_uninitialized(&records)?;
    }
    Ok(account)
}

async fn fetch_snapshot(ctx: &AppContext) -> VenueSnapshot {
    match venue_balances(ctx).await {
        Ok(balances) => VenueSnapshot {
            balances,
            complete: true,
            anchor: None,
        },
        Err(err) => {
            tracing::warn!(%err, "could not fetch the venue balance snapshot; anchoring incomplete");
            VenueSnapshot {
                balances: BTreeMap::new(),
                complete: false,
                anchor: None,
            }
        }
    }
}

/// Account balances keyed by modern asset code. The venue answers in legacy
/// X/Z codes (`ZUSD`, `XXBT`) while every ledger event uses the modern form,
/// so normalizing here keeps one spelling per asset in the journal.
pub(crate) async fn venue_balances(ctx: &AppContext) -> Result<BTreeMap<String, Decimal>> {
    let balances = ctx.spot_account()?.sign(&BalanceQuery::default()).await?;
    Ok(balances
        .iter()
        .map(|(asset, amount)| (asset.to_string(), *amount))
        .collect())
}

/// The account's balance in one asset, or `None` when the venue could not be
/// read. Callers must treat `None` as "unknown", never as zero.
pub(crate) async fn account_balance(ctx: &AppContext, asset: &str) -> Option<Decimal> {
    match venue_balances(ctx).await {
        // The venue answers sparsely: an asset the account holds none of is
        // absent rather than zero. That absence is still a figure the
        // allocation can be checked against, so it must not collapse into the
        // `None` that means "unread" — an over-allocated workspace would read
        // as merely unverified and keep spending.
        Ok(balances) => Some(balances.get(asset).copied().unwrap_or(Decimal::ZERO)),
        Err(err) => {
            tracing::warn!(%err, asset, "could not read the account balance");
            None
        }
    }
}

/// A live workspace's ledger levelled against the venue, plus what stands
/// behind its allocation.
#[derive(Debug, Clone)]
pub(crate) struct LiveSync {
    /// False when the venue could not confirm every mirrored order.
    reconciled: bool,
    backing: kraken_workspace::backing::Backing,
    /// The workspace ledger as the reconciliation pass left it — the figure
    /// the order gate checks spending against.
    state: kraken_paper::PaperState,
}

impl LiveSync {
    /// A sync whose backing the caller already measured. `workspace create` and
    /// `workspace promote` check the allocation before committing to it, and
    /// the venue balance behind that check is a round-trip worth paying for
    /// once.
    pub(crate) async fn with_measured_backing(
        ctx: &AppContext,
        manifest: &kraken_workspace::WorkspaceManifest,
        backing: kraken_workspace::backing::Backing,
    ) -> Result<Self> {
        let (reconciled, state) = reconcile_live_workspace(ctx, manifest).await?;
        Ok(Self {
            reconciled,
            backing,
            state,
        })
    }

    /// Disclose this sync on the output of the command that ran it. Every live
    /// workspace command ends here, so what a caller must reveal about a
    /// real-money read is decided once rather than per command.
    ///
    /// `spendable` is the number an agent sizes the next order against —
    /// the ledger's allocation-asset balance free of reservations — surfaced
    /// here so budgeting never needs a refusal round-trip to discover it.
    pub(crate) fn stamp(&self, out: &mut CommandOutput, workspace: &str) {
        self.stamp_with_spendable(
            out,
            workspace,
            self.state.available_balance(&self.backing.asset),
        );
    }

    /// [`Self::stamp`] with a fresher spendable figure than this sync's own —
    /// the order path re-reads the ledger it just wrote, because a receipt
    /// quoting the pre-order figure would tell an agent the money it just
    /// committed is still free.
    pub(crate) fn stamp_with_spendable(
        &self,
        out: &mut CommandOutput,
        workspace: &str,
        spendable: Decimal,
    ) {
        out.stamp_live_workspace(
            workspace,
            &self.backing,
            self.reconciled,
            spendable.normalize(),
        );
    }

    /// What stands behind the allocation, for the order gate that refuses to
    /// open new risk against money the account no longer holds.
    pub(crate) fn backing(&self) -> &kraken_workspace::backing::Backing {
        &self.backing
    }

    /// The ledger this sync levelled, for the order gate that refuses to spend
    /// beyond what the allocation still holds free of reservations.
    pub(crate) fn state(&self) -> &kraken_paper::PaperState {
        &self.state
    }

    pub(crate) fn reconciled(&self) -> bool {
        self.reconciled
    }
}

/// Level a live workspace's ledger with the venue, then measure its backing.
pub(crate) async fn sync_live_workspace(
    ctx: &AppContext,
    manifest: &kraken_workspace::WorkspaceManifest,
) -> Result<LiveSync> {
    sync_live_workspace_at(&config::config_dir()?, ctx, manifest).await
}

/// [`sync_live_workspace`] against a caller-chosen config base — the seam
/// unit tests drive, since the real path is fixed under the user's config dir.
pub(crate) async fn sync_live_workspace_at(
    base: &std::path::Path,
    ctx: &AppContext,
    manifest: &kraken_workspace::WorkspaceManifest,
) -> Result<LiveSync> {
    let name: kraken_workspace::WorkspaceName = manifest.name.parse()?;
    let (reconciled, state) = reconcile_workspace_at(base, ctx, manifest).await?;
    // The workspace's standing claim on the account is what its ledger still
    // holds of the allocation asset, not the nominal capital: money a fill
    // converted into position left the account with that fill, and demanding
    // the account still hold it would re-refuse trades the allocation already
    // funded. Reservations stay inside the claim — a resting buy's quote is
    // still in the account until it fills. An uninitialized ledger claims
    // nothing; its own funds gate refuses every spend anyway.
    let claim = state
        .balances
        .get(&manifest.currency)
        .copied()
        .unwrap_or(Decimal::ZERO);
    Ok(LiveSync {
        reconciled,
        backing: measure_backing_at(base, ctx, &name, claim, &manifest.currency).await?,
        state,
    })
}

/// Fold the venue's verdict on a live workspace's orders into its journal,
/// answering whether the venue confirmed all of them and how the ledger
/// stands after the fold.
pub(crate) async fn reconcile_live_workspace(
    ctx: &AppContext,
    manifest: &kraken_workspace::WorkspaceManifest,
) -> Result<(bool, kraken_paper::PaperState)> {
    reconcile_workspace_at(&config::config_dir()?, ctx, manifest).await
}

/// The journal's writer lock is taken and released here, before the caller's
/// own read needs it — holding it across the read would self-reject.
async fn reconcile_workspace_at(
    base: &std::path::Path,
    ctx: &AppContext,
    manifest: &kraken_workspace::WorkspaceManifest,
) -> Result<(bool, kraken_paper::PaperState)> {
    let name: kraken_workspace::WorkspaceName = manifest.name.parse()?;
    let journal = kraken_workspace::Workspaces::new(base).journal_path(&name);
    let mut account = PaperAccount::open_at(journal, Origin::from_mcp_mode(ctx.mcp_mode))?;
    let complete = reconcile_open_orders(&mut account, ctx).await?.complete;
    let state = account.state()?.clone();
    Ok((complete, state))
}

/// What stands behind a claim of `claim` `currency` by workspace `name` —
/// the nominal capital at creation and promotion time, the ledger's remaining
/// allocation-asset balance thereafter.
///
/// Separate from reconciliation so the two commands that check an allocation
/// before committing to it — `workspace create --mode live` and `workspace
/// promote` — can pass the result on rather than pay for the venue balance
/// round-trip twice.
pub(crate) async fn measure_backing(
    ctx: &AppContext,
    name: &kraken_workspace::WorkspaceName,
    claim: Decimal,
    currency: &str,
) -> Result<kraken_workspace::backing::Backing> {
    measure_backing_at(&config::config_dir()?, ctx, name, claim, currency).await
}

async fn measure_backing_at(
    base: &std::path::Path,
    ctx: &AppContext,
    name: &kraken_workspace::WorkspaceName,
    claim: Decimal,
    currency: &str,
) -> Result<kraken_workspace::backing::Backing> {
    let held = account_balance(ctx, currency).await;
    // Neighbours are counted at their nominal capital, not their ledgers:
    // reading every live journal here would contend their writer locks on
    // every order. A neighbour that invested part of its allocation is
    // over-counted — the conservative direction.
    let allocated_elsewhere =
        kraken_workspace::Workspaces::new(base).allocated_elsewhere(name, currency)?;
    Ok(kraken_workspace::backing::Backing::evaluate(
        currency,
        claim,
        allocated_elsewhere,
        held,
    ))
}

/// Record a venue-accepted order. Best-effort by design: the venue holds
/// the truth, and a local disk fault must never unreport a real order —
/// reconciliation heals the gap by txid.
///
/// `valuation` funds a buy's reservation: a limit order's own price, or the
/// ask estimate its market order was admitted at — `None` books none. A
/// sell's reservation is its base volume and needs no valuation.
pub(crate) fn mirror_submitted(
    account: &mut PaperAccount,
    txid: &str,
    side: OrderSide,
    pair: &str,
    order_type: PaperOrderType,
    volume: Decimal,
    valuation: Option<Decimal>,
) {
    let order = match observed_order(txid, side, pair, order_type, volume, valuation) {
        Ok(order) => order,
        Err(err) => {
            tracing::warn!(txid, %err, "venue order accepted but not mirrorable");
            return;
        }
    };
    let records = account.stamp(vec![AccountEvent::OrderSubmitted { order }]);
    account.audit(&records);
}

/// Record a venue-accepted cancel for an order the mirror knows. An unknown
/// txid (an order predating the mirror) is skipped — reconciliation owns it.
///
/// Master-journal cancels only: it books a plain cancel, so a partly-filled
/// order's execution is left for the session healer, which folds the account's
/// venue trades under their own trade ids. Workspace cancels fold the venue's
/// per-txid verdict instead (see `cancel_at`) — folding here too would book
/// the same execution twice under two id schemes.
pub(crate) fn mirror_cancelled(account: &mut PaperAccount, txid: &str) {
    let Some(order) = account
        .state()
        .ok()
        .and_then(|state| state.open_orders.iter().find(|o| o.id == txid).cloned())
    else {
        tracing::warn!(
            txid,
            "cancel accepted for an order the mirror never saw; left to reconciliation"
        );
        return;
    };
    let records = account.stamp(vec![AccountEvent::OrderCancelled { order }]);
    account.audit(&records);
}

/// How much of the venue's view the mirror managed to adopt.
pub(crate) struct Reconciliation {
    /// False when the venue could not be reached, or answered for only some
    /// of the orders asked about. Callers disclose it rather than hide it.
    pub(crate) complete: bool,
}

/// Kraken accepts up to 50 transaction IDs per `QueryOrders` call.
const QUERY_ORDERS_BATCH: usize = 50;

/// Fold the venue's verdict on this journal's open orders back into it.
///
/// The mirror records only what the CLI itself did, so an order that filled,
/// expired, or was cancelled by another tool stays open here forever. That is
/// what makes an unreconciled live journal report a stale balance, a P&L of
/// zero, and open orders that no longer exist. `QueryOrders` is the repair:
/// it names the terminal state of every txid the mirror still believes open,
/// and the real fill price and fee that go with it.
pub(crate) async fn reconcile_open_orders(
    account: &mut PaperAccount,
    ctx: &AppContext,
) -> Result<Reconciliation> {
    let open: Vec<PaperOrder> = account.state()?.open_orders.clone();
    if open.is_empty() {
        return Ok(Reconciliation { complete: true });
    }

    let mut outcomes = Vec::new();
    let mut complete = true;

    for chunk in open.chunks(QUERY_ORDERS_BATCH) {
        let ids = chunk
            .iter()
            .map(|order| Id::from(order.id.as_str()))
            .collect();
        let answered = match query_orders(ctx, ids).await {
            Ok(answered) => answered,
            Err(err) => {
                // A read must not fail because the venue is unreachable; the
                // caller discloses the gap instead.
                tracing::warn!(%err, "could not reconcile open orders against the venue");
                complete = false;
                continue;
            }
        };
        for order in chunk {
            let Some(state) = answered.get(&Id::from(order.id.as_str())) else {
                // The venue does not know this txid at all. Inventing a
                // terminal state here would be a guess about real money.
                tracing::warn!(txid = %order.id, "venue returned no state for a mirrored order");
                complete = false;
                continue;
            };
            if let Some(outcome) = venue_outcome(order, state) {
                outcomes.push(outcome);
            }
        }
    }

    if !outcomes.is_empty() {
        // Each outcome is stamped at the venue's close instant, not the fold's
        // wall clock: a late reconciliation must not render a fill at command
        // time on the timeline (window membership itself stays positional).
        let records: Vec<_> = outcomes
            .into_iter()
            .flat_map(|(event, closed_at)| account.stamp_at(vec![event], closed_at))
            .collect();
        account.append(&records)?;
    }
    Ok(Reconciliation { complete })
}

async fn query_orders(ctx: &AppContext, txid: Vec<Id<Order>>) -> Result<QueriedOrders> {
    let query = QueryOrdersQuery {
        txid: txid.into(),
        ..QueryOrdersQuery::default()
    };
    Ok(ctx.spot_account()?.sign(&query).await?)
}

/// Translate one `QueryOrders` entry into the ledger event it implies, paired
/// with the venue's close instant (the fold's wall clock when `closetm` is
/// absent). `None` means "still working" — leave the order open and ask again
/// later.
fn venue_outcome(order: &PaperOrder, state: &Order) -> Option<(AccountEvent, DateTime<Utc>)> {
    let executed = state.volume_executed;
    if !TERMINAL_STATUSES.contains(&state.status) {
        // Still working. An open order that has partly executed is not folded
        // yet: the ledger has no partial-fill event, and recording the ordered
        // volume would overstate the position.
        return None;
    }
    let closed_at = state
        .closetm
        .as_ref()
        .and_then(rest_bridge::instant)
        .unwrap_or_else(Utc::now);
    if executed > Decimal::ZERO {
        // Any terminal status carrying executed volume is a real fill, for the
        // amount that executed. A limit that partly filled and was then
        // cancelled comes back as `canceled` with `vol_exec` set, so treating
        // terminal-plus-executed as a plain cancel would drop a real
        // execution. `OrderFilled` also closes the order and releases the whole
        // reservation, so the unfilled remainder needs no separate cancel.
        return Some((
            AccountEvent::OrderFilled {
                trade: venue_fill(order, state, executed, closed_at),
            },
            closed_at,
        ));
    }
    Some((
        AccountEvent::OrderCancelled {
            order: order.clone(),
        },
        closed_at,
    ))
}

/// Statuses after which Kraken reports nothing further about an order.
const TERMINAL_STATUSES: [OrderStatus; 3] = [
    OrderStatus::Closed,
    OrderStatus::Canceled,
    OrderStatus::Expired,
];

fn venue_fill(
    order: &PaperOrder,
    state: &Order,
    executed: Decimal,
    closed_at: DateTime<Utc>,
) -> kraken_paper::PaperTrade {
    let cost = state.cost;
    // Kraken reports `price` as the average fill price on a closed order;
    // fall back to cost/volume, then to the order's own limit.
    let price = Some(state.price)
        .filter(|p| *p > Decimal::ZERO)
        .or_else(|| cost.checked_div(executed))
        .unwrap_or(order.price);
    kraken_paper::PaperTrade {
        id: format!("{}:fill", order.id),
        order_id: order.id.clone(),
        pair: order.pair.clone(),
        base: order.base.clone(),
        quote: order.quote.clone(),
        side: order.side,
        volume: executed,
        price,
        fee: state.fee,
        cost,
        filled_at: closed_at,
        // A venue fill has no local top-of-book observation behind it.
        reference_quote: None,
    }
}

/// A venue order in the journal's order shape, honestly partial: a market
/// order's price is unknown until reconciliation observes the fill.
fn observed_order(
    txid: &str,
    side: OrderSide,
    pair: &str,
    order_type: PaperOrderType,
    volume: Decimal,
    valuation: Option<Decimal>,
) -> Result<PaperOrder> {
    let (api_pair, base, quote) = parse_pair(pair)?;
    let (reserved_asset, reserved_amount) = commitment(side, &base, &quote, volume, valuation);
    Ok(PaperOrder {
        id: txid.to_string(),
        pair: api_pair,
        base,
        quote,
        side,
        volume,
        // A market order's valuation funds the reservation above but must not
        // pose as the price it will fill at — that stays unknown until the
        // fill folds.
        price: match order_type {
            PaperOrderType::Market => Decimal::ZERO,
            _ => valuation.unwrap_or(Decimal::ZERO),
        },
        order_type,
        reserved_asset,
        reserved_amount,
        created_at: Utc::now(),
    })
}

/// What a resting venue order commits of this workspace's own funds.
///
/// Not a claim about Kraken's hold, which the mirror never sees — this is the
/// workspace's own accounting, the same figure the simulator books, so that
/// `balance` reports a spendable amount rather than the whole allocation while
/// an order rests against part of it. The fee is excluded: the venue charges
/// its own, and padding by the contract's simulated rate would overstate the
/// commitment. A sell commits the base volume it names, priced or not — an
/// exit market sell is deliberately unpriced yet still holds real funds. A
/// buy commits `volume × valuation` — a limit's own price, a market order's
/// ask estimate — and books nothing when no valuation could be read, which
/// only an exit market buy survives to. With a zero commitment, every order
/// between submission and reconciliation would pass the funds gate against
/// money already spent.
fn commitment(
    side: OrderSide,
    base: &str,
    quote: &str,
    volume: Decimal,
    price: Option<Decimal>,
) -> (String, Decimal) {
    match side {
        OrderSide::Buy => (
            quote.to_string(),
            price.map_or(Decimal::ZERO, |price| volume * price),
        ),
        OrderSide::Sell => (base.to_string(), volume),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support;
    use rust_decimal_macros::dec;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    fn ctx(api_url: Option<String>, with_creds: bool) -> AppContext {
        AppContext {
            force: true,
            allow_any_url_host: true,
            ..test_support::offline_ctx(
                api_url,
                with_creds.then(|| test_support::test_credentials("test-api-key")),
            )
        }
    }

    async fn balance_venue() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/0/private/Balance"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "error": [],
                "result": { "ZUSD": "1000.00", "XXBT": "0.5" }
            })))
            .mount(&server)
            .await;
        server
    }

    fn journal_events(journal: &std::path::Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(journal)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn attach_anchors_a_fresh_journal_with_the_venue_snapshot() {
        let server = balance_venue().await;
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("journal.jsonl");
        let ctx = ctx(Some(server.uri()), true);
        drop(open_attached_at(journal.clone(), &ctx).await.unwrap());
        let events = journal_events(&journal);
        assert_eq!(events[0]["event"], "attached");
        assert_eq!(events[0]["complete"], true);
        // Anchored under the modern asset code, not the venue's legacy one:
        // every fill event spells it this way, so one spelling per asset.
        assert_eq!(events[0]["balances"]["USD"], "1000.00");
        assert_eq!(events[0]["balances"]["BTC"], "0.5");
        assert!(events[0]["balances"].get("ZUSD").is_none());
    }

    /// The venue omits what the account holds none of, so absence must read as
    /// zero. Conflating it with an unreadable balance downgrades an
    /// over-allocated workspace to `unverified`, which still permits spending.
    #[tokio::test]
    async fn an_asset_absent_from_the_venue_map_is_zero_not_unknown() {
        let server = balance_venue().await;
        let ctx = ctx(Some(server.uri()), true);
        assert_eq!(account_balance(&ctx, "USD").await, Some(dec!(1000.00)));
        assert_eq!(account_balance(&ctx, "SOL").await, Some(dec!(0)));
    }

    /// Only an unreadable venue is unknown.
    #[tokio::test]
    async fn an_unreachable_venue_leaves_the_balance_unknown() {
        let ctx = ctx(None, false);
        assert_eq!(account_balance(&ctx, "USD").await, None);
    }

    #[tokio::test]
    async fn attach_without_a_reachable_venue_discloses_an_incomplete_anchor() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("journal.jsonl");
        // No credentials: the anchor fetch fails, the attach must not.
        let ctx = ctx(None, false);
        drop(open_attached_at(journal.clone(), &ctx).await.unwrap());
        let events = journal_events(&journal);
        assert_eq!(events[0]["event"], "attached");
        assert_eq!(events[0]["complete"], false);
    }

    #[tokio::test]
    async fn reopening_the_mirror_never_reanchors() {
        let server = balance_venue().await;
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("journal.jsonl");
        let ctx = ctx(Some(server.uri()), true);
        drop(open_attached_at(journal.clone(), &ctx).await.unwrap());
        drop(open_attached_at(journal.clone(), &ctx).await.unwrap());
        let attaches = journal_events(&journal)
            .iter()
            .filter(|e| e["event"] == "attached")
            .count();
        assert_eq!(attaches, 1, "one anchor per journal lifetime");
    }

    /// A resting venue order commits the workspace's own funds, so `balance`
    /// reports what is left rather than the whole allocation. Only an
    /// unpriceable buy commits nothing — a sell's commitment is its own base
    /// volume, so no valuation is needed to book it.
    #[test]
    fn a_resting_order_commits_its_funding_asset() {
        assert_eq!(
            commitment(OrderSide::Buy, "BTC", "USD", dec!(0.001), Some(dec!(30000))),
            ("USD".to_string(), dec!(30))
        );
        assert_eq!(
            commitment(
                OrderSide::Sell,
                "BTC",
                "USD",
                dec!(0.0005),
                Some(dec!(99000))
            ),
            ("BTC".to_string(), dec!(0.0005))
        );
        let (_, amount) = commitment(OrderSide::Buy, "BTC", "USD", dec!(0.001), None);
        assert_eq!(amount, Decimal::ZERO, "an unpriceable buy commits nothing");
        assert_eq!(
            commitment(OrderSide::Sell, "BTC", "USD", dec!(0.001), None),
            ("BTC".to_string(), dec!(0.001)),
            "an unpriced sell still commits its volume"
        );
    }

    /// A venue order row: the fields a test names over a complete open order.
    fn venue_state(fields: serde_json::Value) -> kraken_rest::spot::account::Order {
        serde_json::from_value(crate::commands::test_support::order_row(fields)).unwrap()
    }

    fn mirrored_order() -> PaperOrder {
        observed_order(
            "OTXID-000000-0001",
            OrderSide::Sell,
            "BTC/USD",
            PaperOrderType::Limit,
            dec!(0.25),
            Some(dec!(64000)),
        )
        .unwrap()
    }

    /// A closed order with executed volume becomes a real fill at the venue's
    /// own price and fee — not the limit price the CLI asked for.
    #[test]
    fn a_closed_order_folds_the_venue_price_and_fee() {
        let state = json!({
            "status": "closed", "vol": "0.25", "vol_exec": "0.25",
            "cost": "16000.0", "fee": "41.60", "price": "64001.5"
        });
        let Some((AccountEvent::OrderFilled { trade }, _)) =
            venue_outcome(&mirrored_order(), &venue_state(state))
        else {
            panic!("a closed order with executed volume is a fill");
        };
        assert_eq!(trade.price, dec!(64001.5));
        assert_eq!(trade.fee, dec!(41.60));
        assert_eq!(trade.volume, dec!(0.25));
        assert!(
            trade.reference_quote.is_none(),
            "a venue fill has no local quote behind it"
        );
    }

    /// The common partial case: a limit partly fills, the rest is cancelled.
    /// Kraken reports `canceled` with `vol_exec` set, and the executed part is
    /// a real execution that must reach the ledger.
    #[test]
    fn a_cancelled_order_that_partly_filled_still_records_the_fill() {
        let state = json!({
            "status": "canceled", "vol": "0.25", "vol_exec": "0.10",
            "cost": "6400.0", "fee": "16.64", "price": "64000"
        });
        let Some((AccountEvent::OrderFilled { trade }, _)) =
            venue_outcome(&mirrored_order(), &venue_state(state))
        else {
            panic!("the executed portion of a cancelled order is a fill");
        };
        assert_eq!(
            trade.volume,
            dec!(0.10),
            "the executed amount, not the order's"
        );
        assert_eq!(trade.cost, dec!(6400.0));
        assert_eq!(trade.fee, dec!(16.64));
    }

    /// Expiry carries executed volume the same way a cancel does.
    #[test]
    fn an_expired_order_that_partly_filled_still_records_the_fill() {
        let state = json!({
            "status": "expired", "vol": "0.25", "vol_exec": "0.05",
            "cost": "3200.0", "fee": "8.32", "price": "64000"
        });
        assert!(matches!(
            venue_outcome(&mirrored_order(), &venue_state(state)),
            Some((AccountEvent::OrderFilled { .. }, _))
        ));
    }

    /// Closed with nothing executed is a completed cancel, not a zero fill.
    #[test]
    fn a_closed_order_with_no_execution_is_a_cancel() {
        let state = json!({
            "status": "closed", "vol": "0.25", "vol_exec": "0",
            "cost": "0", "fee": "0", "price": "0"
        });
        assert!(matches!(
            venue_outcome(&mirrored_order(), &venue_state(state)),
            Some((AccountEvent::OrderCancelled { .. }, _))
        ));
    }

    /// A fold long after execution must carry the venue's `closetm`, not the
    /// fold's wall clock — the timeline orders records by their stamp.
    #[test]
    fn a_folded_outcome_is_stamped_at_the_venue_close_time() {
        let state = json!({
            "status": "closed", "vol": "0.25", "vol_exec": "0.25",
            "cost": "16000.0", "fee": "41.60", "price": "64001.5",
            "closetm": 1754560000.25f64
        });
        let expected = DateTime::from_timestamp(1754560000, 250_000_000).unwrap();
        let Some((AccountEvent::OrderFilled { trade }, closed_at)) =
            venue_outcome(&mirrored_order(), &venue_state(state))
        else {
            panic!("a closed order with executed volume is a fill");
        };
        assert_eq!(closed_at, expected);
        assert_eq!(
            trade.filled_at, expected,
            "the trade carries the same instant"
        );
    }

    /// A partial fill stays open: recording the whole volume would overstate
    /// the position, and the ledger has no partial-fill event.
    #[test]
    fn a_partially_filled_order_stays_open() {
        let state = json!({
            "status": "open", "vol": "0.25", "vol_exec": "0.10",
            "cost": "6400.0", "fee": "16.64", "price": "64000"
        });
        assert!(venue_outcome(&mirrored_order(), &venue_state(state)).is_none());
    }

    #[tokio::test]
    async fn mirror_cancelled_closes_a_known_order_and_skips_an_unknown_txid() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("journal.jsonl");
        let ctx = ctx(None, false);
        let mut account = open_attached_at(journal.clone(), &ctx).await.unwrap();
        mirror_submitted(
            &mut account,
            "OTXID-KNOWN-000001",
            OrderSide::Buy,
            "BTC/USD",
            PaperOrderType::Limit,
            dec!(0.01),
            Some(dec!(100)),
        );
        mirror_cancelled(&mut account, "OTXID-NEVER-SEEN00");
        mirror_cancelled(&mut account, "OTXID-KNOWN-000001");
        let events = journal_events(&journal);
        let cancelled: Vec<_> = events
            .iter()
            .filter(|e| e["event"] == "order_cancelled")
            .collect();
        assert_eq!(cancelled.len(), 1, "unknown txid left to reconciliation");
        assert_eq!(cancelled[0]["order"]["id"], "OTXID-KNOWN-000001");
    }
}
