//! Spot trading commands.
//!
//! The command *shapes* live in [`kraken_contract::order`]; the work of
//! executing them stays here.

use super::{Execute, rest_bridge, workspace_guard};
use crate::{
    cli::AppContext,
    errors::{KrakenError, Result},
    output::{CommandOutput, published_json, rounded},
};
use itertools::Itertools as _;
use kraken_contract::order::{
    Amend, Batch, Buy, Cancel, CancelAfter, CancelAll, CancelBatch, Edit, OrderCommand,
    OrderRequest, Sell,
};
use kraken_core::{OrderSide, OrderType as Type};
use kraken_rest::{
    CommaList, Pair,
    spot::{
        Trigger,
        trading::{self as rest, OrderKind, OrderPrice, OrderRef, OrderTag},
    },
};
use rust_decimal::Decimal;
use strum::IntoEnumIterator as _;

// `OrderCommand`'s variants are foreign types now, so the delegation is written
// rather than derived — see `delegate_execute!`.
delegate_execute!(OrderCommand {
    Buy,
    Sell,
    Batch,
    Amend,
    Edit,
    Cancel,
    CancelBatch,
    CancelAll,
    CancelAfter,
});

impl Execute for Buy {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        route_order(OrderSide::Buy, self.order, ctx).await
    }
}

impl Execute for Sell {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        route_order(OrderSide::Sell, self.order, ctx).await
    }
}

impl Execute for Batch {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            json_file,
            pair,
            asset_class,
            deadline,
            validate,
        } = self;
        let contents = std::fs::read_to_string(json_file)?;
        let orders = batch_orders(&contents)?;
        let pair = pair
            .map(Pair::from)
            .or_else(|| orders.first().and_then(|o| o.pair.clone()))
            .ok_or_else(|| {
                KrakenError::Validation(
                    "Batch needs a pair: pass --pair or name it in the first order".into(),
                )
            })?;
        let request = rest::AddOrderBatch {
            orders,
            pair,
            asset_class: asset_class.as_deref().map(rest_bridge::open_asset_class),
            deadline,
            validate,
        };
        let placed = ctx.spot_account()?.sign(&request).await?;
        CommandOutput::published(&placed)
    }
}

impl Execute for Amend {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            txid,
            cl_ord_id,
            order_qty,
            display_qty,
            limit_price,
            trigger_price,
            pair,
            post_only,
            deadline,
        } = self;
        let request = rest::AmendOrder {
            order: amend_target(txid, cl_ord_id)?,
            order_qty: rest_bridge::optional_amount("--order-qty", order_qty.as_deref())?,
            display_qty: rest_bridge::optional_amount("--display-qty", display_qty.as_deref())?,
            limit_price: limit_price.as_deref().map(price).transpose()?,
            trigger_price: trigger_price.as_deref().map(price).transpose()?,
            pair: pair.map(Pair::from),
            post_only,
            deadline,
        };
        let amended = ctx.spot_account()?.sign(&request).await?;
        CommandOutput::published(&amended)
    }
}

impl Execute for Edit {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            txid,
            volume,
            price: new_price,
            pair,
        } = self;
        let request = rest::EditOrder {
            txid: txid.into(),
            volume: rest_bridge::optional_amount("--volume", volume.as_deref())?,
            price: new_price.as_deref().map(price).transpose()?,
            pair: pair.map(Pair::from),
        };
        let edited = ctx.spot_account()?.sign(&request).await?;
        CommandOutput::published(&edited)
    }
}

impl Execute for Cancel {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            txids,
            cl_ord_id,
            reason,
        } = self;
        if txids.is_empty() && cl_ord_id.is_none() {
            return Err(KrakenError::Validation(
                "Provide txid(s) or --cl-ord-id to cancel".into(),
            ));
        }
        if !txids.is_empty() && cl_ord_id.is_some() {
            return Err(KrakenError::Validation(
                "Provide either txid(s) or --cl-ord-id, not both".into(),
            ));
        }
        match workspace_guard::active_mode(ctx)? {
            workspace_guard::ActiveMode::Paper(manifest) => {
                if cl_ord_id.is_some() {
                    return Err(kraken_workspace::policy::unsupported(
                        "order cancel --cl-ord-id",
                        &manifest.name,
                        manifest.mode,
                    )
                    .into());
                }
                let [txid] = txids.as_slice() else {
                    return Err(kraken_workspace::policy::unsupported(
                        "order cancel with multiple txids",
                        &manifest.name,
                        manifest.mode,
                    )
                    .into());
                };
                super::paper::cancel_on_workspace(ctx, &manifest, txid, reason.as_deref()).await
            }
            workspace_guard::ActiveMode::Live(manifest) => {
                live_cancel(txids, cl_ord_id, reason.as_deref(), &manifest, ctx).await
            }
            workspace_guard::ActiveMode::Master => {
                master_cancel(txids, cl_ord_id, reason.as_deref(), ctx).await
            }
        }
    }
}

async fn live_cancel(
    txids: Vec<String>,
    cl_ord_id: Option<String>,
    reason: Option<&str>,
    manifest: &kraken_workspace::WorkspaceManifest,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    workspace_guard::warn_live_workspace(manifest);
    if cl_ord_id.is_some() {
        return Err(KrakenError::Validation(format!(
            "order cancel --cl-ord-id is not supported in live workspace '{}'; cancel by venue \
             txid so the workspace can check that it owns the order",
            manifest.name
        )));
    }
    let name: kraken_workspace::WorkspaceName = manifest.name.parse()?;
    let scope = kraken_workspace::workspace_dir(&crate::config::config_dir()?, &name);
    let journal = scope.join(kraken_workspace::JOURNAL_FILE);
    // Reconcile before the ownership check, so an order the venue has already
    // closed is reported as closed rather than accepted for cancellation.
    super::journal_mirror::sync_live_workspace(ctx, manifest).await?;
    let mut out = cancel_at(
        journal,
        &scope,
        txids,
        None,
        reason,
        ctx,
        Some(&manifest.name),
    )
    .await?;
    // Stamped from a POST-cancel sync: the pre-cancel one would assert
    // `reconciled` about a ledger the cancel just changed — and if the
    // cancel's own fold failed, this retries it before anything is claimed.
    let sync = super::journal_mirror::sync_live_workspace(ctx, manifest).await?;
    sync.stamp(&mut out, &manifest.name);
    Ok(out)
}

async fn master_cancel(
    txids: Vec<String>,
    cl_ord_id: Option<String>,
    reason: Option<&str>,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    eprintln!("{LIVE_BANNER}");
    let scope = crate::config::config_dir()?;
    cancel_at(
        super::journal_mirror::master_journal_path()?,
        &scope,
        txids,
        cl_ord_id,
        reason,
        ctx,
        None,
    )
    .await
}

async fn cancel_at(
    journal: std::path::PathBuf,
    decision_scope: &std::path::Path,
    txids: Vec<String>,
    cl_ord_id: Option<String>,
    reason: Option<&str>,
    ctx: &AppContext,
    workspace: Option<&str>,
) -> Result<CommandOutput> {
    // Open the mirror first so local lock failure cannot follow execution.
    // A cl_ord_id cancel has no txid to mirror; reconciliation closes that gap.
    let mut mirror = super::journal_mirror::open_attached_at(journal, ctx).await?;
    if let Some(workspace) = workspace {
        let open_ids = mirror
            .state()?
            .open_orders
            .iter()
            .map(|order| order.id.as_str())
            .collect::<Vec<_>>();
        if let Some(unknown) = txids.iter().find(|txid| !open_ids.contains(&txid.as_str())) {
            return Err(KrakenError::Validation(format!(
                "Order '{unknown}' is not an open order recorded by workspace '{workspace}'. A \
                 workspace can only cancel the orders it placed."
            )));
        }
    }
    let account = ctx.spot_account()?;

    // A client order ID names no txid, so there is nothing to own-check or
    // mirror; reconciliation closes that gap.
    if let Some(id) = cl_ord_id {
        let request = rest::CancelOrder {
            order: OrderRef::ClientOrderId(id),
        };
        let cancelled = account.sign(&request).await?;
        return CommandOutput::published(&cancelled);
    }

    // One request per order. Kraken's CancelOrder cancels a single txid, and
    // its per-order reply is the only way to learn which orders actually
    // closed — a batch reply carries a bare count. Mirroring a cancel the
    // venue never confirmed would close the order locally while it stays live,
    // and the ownership check above would then refuse every retry.
    let mut cancelled = Vec::new();
    let mut failed = Vec::new();
    let mut already_gone = Vec::new();
    for txid in &txids {
        let request = rest::CancelOrder {
            order: OrderRef::Txid(txid.as_str().into()),
        };
        if let Err(err) = account.sign(&request).await.map_err(KrakenError::from) {
            // "No such order" means there is nothing left to cancel there, so a
            // flatten has achieved what it asked for and must not keep failing.
            // Only where the ownership check above proved the txid is this
            // workspace's: without a workspace a bad txid is a typo, and
            // reporting success for it would hide the mistake.
            //
            // The ledger is deliberately untouched. Kraken answers this for any
            // txid it does not hold OPEN — including one that filled between
            // the reconcile above and this call — so retiring the row here
            // would drop a real execution. Left open, `QueryOrders` keeps being
            // asked about it and still folds the fill if the venue answers.
            if workspace.is_some() && matches!(err, KrakenError::UnknownOrder(_)) {
                tracing::warn!(txid, "the venue holds no such order; nothing to cancel");
                already_gone.push(txid.clone());
                continue;
            }
            failed.push(crate::errors::CancelFailure {
                txid: txid.clone(),
                verify_first: err.is_verify_first(),
                error: err.to_string(),
            });
            continue;
        }
        // A master cancel books the plain local cancel: the account's session
        // healer folds its executions from venue trade ids, and a per-txid
        // fold here would book the same execution twice under two id schemes.
        if workspace.is_none() {
            super::journal_mirror::mirror_cancelled(&mut mirror, txid);
        }
        if let Some(reason) = reason
            && let Err(err) =
                super::paper::log_routed_cancel_decision_at(decision_scope, txid, reason)
        {
            tracing::warn!(txid, %err, "order cancelled but decision-log append failed");
        }
        cancelled.push(txid.clone());
    }

    // A workspace ledger adopts the venue's verdict on the cancelled rows
    // rather than assuming one: a cancel that partly filled comes back
    // `canceled` with `vol_exec` set, and booking a plain local cancel would
    // drop that real execution for good — reconciliation only asks about rows
    // still open. Best-effort like the mirror itself: the cancels are already
    // real, so a failed fold is a warning and the rows stay open for the next
    // sync.
    if workspace.is_some() && !cancelled.is_empty() {
        match super::journal_mirror::reconcile_open_orders(&mut mirror, ctx).await {
            Ok(fold) if !fold.complete => {
                tracing::warn!(
                    ?cancelled,
                    workspace,
                    "venue verdict on cancelled orders incomplete; next sync retries"
                );
            }
            Err(err) => {
                tracing::warn!(
                    ?cancelled,
                    workspace,
                    %err,
                    "cancelled at the venue but the ledger fold failed"
                );
            }
            Ok(_) => {}
        }
    }

    if !failed.is_empty() {
        return Err(KrakenError::PartialCancel {
            cancelled,
            already_gone,
            failed,
        });
    }
    // Same row shape the simulator's cancel prints, so a cancel reads the same
    // before and after promotion — led by the mode, because this one closed
    // real orders at the venue.
    let mut out = CommandOutput::key_value(
        vec![
            (
                "Mode".to_string(),
                crate::output::mode_label(kraken_workspace::receipt::TradeMode::Live).to_string(),
            ),
            ("Action".to_string(), "Orders cancelled".to_string()),
            ("Cancelled".to_string(), cancelled.len().to_string()),
            ("Order IDs".to_string(), cancelled.join(", ")),
        ],
        serde_json::json!({
            "count": cancelled.len(),
            "cancelled": cancelled,
            "already_gone_at_venue": already_gone,
        }),
    );
    if !already_gone.is_empty() {
        out.stamp_context_row("Already gone at the venue", &already_gone.len().to_string());
        out.footer.push(format!(
            "The venue holds no order with {}: nothing left to cancel there. Still recorded here \
             until it answers for {} — see `open-orders`.",
            already_gone.join(", "),
            if already_gone.len() == 1 {
                "it"
            } else {
                "them"
            },
        ));
    }
    Ok(out)
}

impl Execute for CancelBatch {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { txids, cl_ord_ids } = self;
        validate_cancel_batch(&txids, &cl_ord_ids)?;
        let request = rest::CancelOrderBatch {
            orders: txids.into_iter().map(Into::into).collect(),
            cl_ord_ids,
        };
        let cancelled = ctx.spot_account()?.sign(&request).await?;
        CommandOutput::published(&cancelled)
    }
}

impl Execute for CancelAll {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        match workspace_guard::active_mode(ctx)? {
            workspace_guard::ActiveMode::Paper(manifest) => {
                super::paper::cancel_all_on_workspace(ctx, &manifest).await
            }
            workspace_guard::ActiveMode::Live(manifest) => live_cancel_all(&manifest, ctx).await,
            workspace_guard::ActiveMode::Master => master_cancel_all(ctx).await,
        }
    }
}

async fn live_cancel_all(
    manifest: &kraken_workspace::WorkspaceManifest,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    workspace_guard::warn_live_workspace(manifest);
    let name: kraken_workspace::WorkspaceName = manifest.name.parse()?;
    let journal =
        kraken_workspace::Workspaces::new(crate::config::config_dir()?).journal_path(&name);
    // Reconcile before collecting the sweep: without it the mirror would still
    // list orders the venue already filled, and every one of those becomes a
    // doomed cancel that fails the whole command.
    let sync = super::journal_mirror::sync_live_workspace(ctx, manifest).await?;
    let open_ids = workspace_open_order_ids(&journal, ctx)?;
    if open_ids.is_empty() {
        let mut out = CommandOutput::key_value(
            vec![("Count".to_string(), "0".to_string())],
            serde_json::json!({ "count": 0 }),
        );
        sync.stamp(&mut out, &manifest.name);
        return Ok(out);
    }
    ctx.confirm_destructive(&format!(
        "Cancel all {} open order(s) recorded by live workspace '{}'?",
        open_ids.len(),
        manifest.name
    ))?;
    let scope = kraken_workspace::workspace_dir(&crate::config::config_dir()?, &name);
    let mut out = cancel_at(
        journal,
        &scope,
        open_ids,
        None,
        None,
        ctx,
        Some(&manifest.name),
    )
    .await?;
    // Stamped from a POST-cancel sync, for the same reason as `live_cancel`:
    // the pre-cancel flag would vouch for a ledger the sweep just changed.
    let sync = super::journal_mirror::sync_live_workspace(ctx, manifest).await?;
    sync.stamp(&mut out, &manifest.name);
    Ok(out)
}

fn workspace_open_order_ids(journal: &std::path::Path, ctx: &AppContext) -> Result<Vec<String>> {
    let account = kraken_paper::account::PaperAccount::open_at(
        journal.to_path_buf(),
        kraken_paper::account::Origin::from_mcp_mode(ctx.mcp_mode),
    )?;
    Ok(account
        .state()?
        .open_orders
        .iter()
        .map(|order| order.id.clone())
        .collect())
}

async fn master_cancel_all(ctx: &AppContext) -> Result<CommandOutput> {
    eprintln!("{LIVE_BANNER}");
    // Confirm and lock the mirror before execution can succeed.
    // The venue reports only a count, so the mirror closes
    // every order it knows — anything it never saw is reconciliation's.
    ctx.confirm_destructive("Cancel ALL open orders?")?;
    cancel_all_at(super::journal_mirror::master_journal_path()?, ctx).await
}

async fn cancel_all_at(journal: std::path::PathBuf, ctx: &AppContext) -> Result<CommandOutput> {
    let mut mirror = super::journal_mirror::open_attached_at(journal, ctx).await?;
    let cancelled = ctx
        .spot_account()?
        .sign(&rest::CancelAll::default())
        .await?;
    let known: Vec<String> = mirror
        .state()
        .ok()
        .map(|state| state.open_orders.iter().map(|o| o.id.clone()).collect())
        .unwrap_or_default();
    for txid in &known {
        super::journal_mirror::mirror_cancelled(&mut mirror, txid);
    }
    CommandOutput::published(&cancelled)
}

impl Execute for CancelAfter {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { timeout } = self;
        let request = rest::CancelAllOrdersAfter { timeout };
        let switch = ctx.spot_account()?.sign(&request).await?;
        CommandOutput::published(&switch)
    }
}

/// One line of live-mode visibility on every real-account trading verb
/// — stderr, so `-o json` pipelines stay clean. Pinned by test;
/// skills quote it.
pub(crate) const LIVE_BANNER: &str = "live: this goes to the real Kraken account";

/// The mode switch: one order vocabulary, the backend picked
/// by the workspace the guard resolved. This boundary keeps named live
/// workspaces from falling through to the real account.
async fn route_order(
    side: OrderSide,
    order: OrderRequest,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    match workspace_guard::active_mode(ctx)? {
        workspace_guard::ActiveMode::Paper(manifest) => {
            paper_order(side, order, &manifest, ctx).await
        }
        workspace_guard::ActiveMode::Master => master_order(side, order, ctx).await,
        workspace_guard::ActiveMode::Live(manifest) => {
            live_order(side, order, &manifest, ctx).await
        }
    }
}

async fn live_order(
    side: OrderSide,
    order: OrderRequest,
    manifest: &kraken_workspace::WorkspaceManifest,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    live_order_at(&crate::config::config_dir()?, side, order, manifest, ctx).await
}

/// [`live_order`] against a caller-chosen config base — the unit-test seam.
async fn live_order_at(
    base: &std::path::Path,
    side: OrderSide,
    order: OrderRequest,
    manifest: &kraken_workspace::WorkspaceManifest,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    ensure_paper_mappable(&order, manifest)?;
    let canonical_pair = kraken_workspace::policy::canonical_pair(&order.pair)?;
    kraken_workspace::policy::ensure_pair_allowed(manifest, &canonical_pair)?;
    if !order.validate {
        workspace_guard::warn_live_workspace(manifest);
    }
    let name: kraken_workspace::WorkspaceName = manifest.name.parse()?;
    let journal = kraken_workspace::Workspaces::new(base).journal_path(&name);
    // Level the ledger and check the allocation BEFORE the venue call: an
    // over-allocated workspace would otherwise record real orders against
    // money the account no longer holds.
    let sync = super::journal_mirror::sync_live_workspace_at(base, ctx, manifest).await?;
    ensure_backed(manifest, &sync, side, &canonical_pair)?;
    // Price the spend before the journal opens: the ask fetch is a venue
    // round-trip and the lock below should span only the check and the order.
    let spend = if acquires(side, &canonical_pair, &sync.backing().asset)? {
        // The exit is never refused, so it carries no gate figure.
        None
    } else {
        Some(gate_spend(ctx, side, &canonical_pair, &order).await?)
    };
    // A pending market order reserves its spend until the fill folds: with a
    // zero reservation, every order until reconciliation would pass the funds
    // check against money already committed. Exits too — skipping their
    // refusal must not skip their reservation — but priced best-effort, so
    // an unpriceable exit is still never blocked.
    let market_valuation = if !order.r#type.eq_ignore_ascii_case("market") {
        None
    } else if let Some((_, price)) = spend {
        Some(price)
    } else if order.validate {
        // A dry run mirrors nothing; pricing it would fund nothing.
        None
    } else {
        exit_market_valuation(ctx, side, &canonical_pair).await
    };
    // Open the mirror BEFORE the funds check: its writer lock serializes
    // concurrent trades, so the ledger checked here is the ledger this order
    // lands on — two parallel orders cannot both pass against one snapshot.
    // A validate-only request places nothing, so no mirror and no race.
    let mirror = if order.validate {
        None
    } else {
        Some(super::journal_mirror::open_attached_at(journal.clone(), ctx).await?)
    };
    if let Some((volume, price)) = spend {
        let state = match &mirror {
            Some(mirror) => mirror.state()?,
            None => sync.state(),
        };
        ensure_allocation_funds(manifest, &sync, state, side, &canonical_pair, volume, price)?;
    }
    let mut out = master_order_on(mirror, &journal, side, order, market_valuation, ctx).await?;
    // The receipt's spendable figure must carry the order it reports: the
    // sync's snapshot predates the reservation just booked. Best-effort like
    // the mirror itself — the order is already real, so a failed re-read
    // falls back to the sync's figure rather than failing the receipt.
    match kraken_paper::account::PaperAccount::open_at(
        journal,
        kraken_paper::account::Origin::from_mcp_mode(ctx.mcp_mode),
    )
    .and_then(|account| Ok(account.state()?.available_balance(&sync.backing().asset)))
    {
        Ok(spendable) => sync.stamp_with_spendable(&mut out, &manifest.name, spendable),
        Err(err) => {
            tracing::warn!(%err, "receipt stamped with the pre-order spendable figure");
            sync.stamp(&mut out, &manifest.name);
        }
    }
    Ok(out)
}

/// Refuse an order that would spend more of an allocation the account can no
/// longer stand behind — or cannot currently be shown to stand behind — but
/// never the order that gets the user out.
///
/// The CLI enforces the allocation; Kraken does not. Another tool on the same
/// key can spend allocated funds at any time, which is exactly the case this
/// catches — and why the refusal names the numbers instead of asserting a
/// boundary that does not exist at the venue. An unreadable balance fails the
/// same direction: `workspace create` refuses an allocation it cannot verify,
/// and letting orders spend against one would make the outage the loophole.
///
/// The exemption is deliberately not "sells": in a BTC-denominated workspace,
/// selling BTC is what spends the allocation and buying it back is the exit, so
/// a side-based rule would trap that user and wave through the harmful
/// direction. What matters is which leg the allocated asset is on. Refusing the
/// acquiring order too would leave real exposure with no way out through the
/// workspace, and an unbacked allocation is a reason to stop adding risk, never
/// a reason to hold someone in a position.
fn ensure_backed(
    manifest: &kraken_workspace::WorkspaceManifest,
    sync: &super::journal_mirror::LiveSync,
    side: OrderSide,
    canonical_pair: &str,
) -> Result<()> {
    use kraken_workspace::backing::BackingStatus;
    let backing = sync.backing();
    let asset = &backing.asset;
    if acquires(side, canonical_pair, asset)? {
        return Ok(());
    }
    match backing.status {
        BackingStatus::Backed => Ok(()),
        BackingStatus::OverAllocated => Err(KrakenError::Refused {
            message: format!(
                "workspace '{}' is over-allocated, so it will not spend more {asset}: {}. An \
                 order that pays {asset} back into the workspace — closing a position — is still \
                 allowed; otherwise move funds into the account until it covers the allocation.",
                manifest.name,
                backing.summary()
            ),
            refusal: Box::new(crate::errors::Refusal::OverAllocated {
                workspace: manifest.name.clone(),
                backing: backing.clone(),
            }),
        }),
        BackingStatus::Unverified => Err(KrakenError::Refused {
            message: format!(
                "workspace '{}' cannot verify the account behind its allocation, so it will not \
                 spend more {asset}: {}. An order that pays {asset} back into the workspace — \
                 closing a position — is still allowed; fix what the summary names — credentials, \
                 venue connectivity, or an unreadable workspace — then retry.",
                manifest.name,
                backing.summary()
            ),
            refusal: Box::new(crate::errors::Refusal::Unverified {
                workspace: manifest.name.clone(),
                backing: backing.clone(),
            }),
        }),
    }
}

/// Whether filling this order pays `asset` INTO the workspace.
///
/// A buy spends the quote to acquire the base; a sell the reverse. A pair
/// carrying `asset` on neither leg cannot reduce the shortfall, so it counts as
/// spending — the conservative side, and it still leaves the allocated asset's
/// own exit open.
fn acquires(side: OrderSide, canonical_pair: &str, asset: &str) -> Result<bool> {
    let (_, base, quote) = kraken_paper::parse_pair(canonical_pair)?;
    let acquired = match side {
        OrderSide::Buy => base,
        OrderSide::Sell => quote,
    };
    Ok(acquired.eq_ignore_ascii_case(asset))
}

/// The `(volume, price)` the funds gate values a spending order at.
///
/// A market buy is priced at the venue's current ask even when a `--price`
/// came along — the venue ignores that price, so gating at it would let a
/// token `--price 0.01` walk an arbitrarily large market order through the
/// budget. Fail closed when no usable ask is readable: an unpriceable order
/// must not be the one order that bypasses the budget. A sell spends the base
/// leg, so its requirement is the volume itself and no price enters it.
async fn gate_spend(
    ctx: &AppContext,
    side: OrderSide,
    canonical_pair: &str,
    order: &OrderRequest,
) -> Result<(Decimal, Decimal)> {
    let volume: Decimal = order.volume.parse().map_err(|_| {
        KrakenError::Validation(format!("Volume '{}' is not a number", order.volume))
    })?;
    kraken_paper::validate_money(volume, "Volume")?;
    let price = match side {
        OrderSide::Sell => Decimal::ZERO,
        OrderSide::Buy if order.r#type.eq_ignore_ascii_case("market") => {
            let (api_pair, _, _) = kraken_paper::parse_pair(canonical_pair)?;
            let (ask, _) = super::paper::fetch_ticker_price(ctx.spot(), &api_pair).await?;
            kraken_paper::validate_money(ask, "Ask")?;
            ask
        }
        OrderSide::Buy => match order.price.as_deref() {
            Some(price) => {
                let price: Decimal = price.parse().map_err(|_| {
                    KrakenError::Validation(format!("Price '{price}' is not a number"))
                })?;
                kraken_paper::validate_money(price, "Price")?;
                price
            }
            None => {
                return Err(KrakenError::Validation(
                    "order --type limit requires --price".into(),
                ));
            }
        },
    };
    Ok((volume, price))
}

/// The valuation funding an exit market order's reservation.
///
/// Best-effort by the exit doctrine: pricing must never become refusing, so
/// an unreadable ask books no reservation instead of failing the order. A
/// sell needs no valuation at all — its commitment is the base volume it
/// names, and the mirror books that without a price.
async fn exit_market_valuation(
    ctx: &AppContext,
    side: OrderSide,
    canonical_pair: &str,
) -> Option<Decimal> {
    if matches!(side, OrderSide::Sell) {
        return None;
    }
    let ask: Result<Decimal> = async {
        let (api_pair, _, _) = kraken_paper::parse_pair(canonical_pair)?;
        let (ask, _) = super::paper::fetch_ticker_price(ctx.spot(), &api_pair).await?;
        kraken_paper::validate_money(ask, "Ask")?;
        Ok(ask)
    }
    .await;
    match ask {
        Ok(ask) => Some(ask),
        Err(err) => {
            tracing::warn!(
                %err,
                pair = canonical_pair,
                "exit market buy books no reservation: ask unreadable"
            );
            None
        }
    }
}

/// Refuse an order the workspace's own ledger cannot fund, before the venue
/// sees it. [`ensure_backed`] guards the account behind the allocation; this
/// guards the allocation itself, and shares the exemption doctrine: the
/// caller skips it entirely for the order that pays the allocated asset back
/// in — an exit is never refused.
fn ensure_allocation_funds(
    manifest: &kraken_workspace::WorkspaceManifest,
    sync: &super::journal_mirror::LiveSync,
    state: &kraken_paper::PaperState,
    side: OrderSide,
    canonical_pair: &str,
    volume: Decimal,
    price: Decimal,
) -> Result<()> {
    let Some(shortfall) = allocation_shortfall(state, side, canonical_pair, volume, price)? else {
        return Ok(());
    };
    // Normalized so "60000.0" and "3.48440" read (and serialize) as the
    // figures they are, in prose, rows, and envelope fields alike.
    let available = shortfall.available.normalize();
    let required = shortfall.required.normalize();
    Err(KrakenError::Refused {
        message: format!(
            "Insufficient {funding} in live workspace '{workspace}': available {available}, \
             required {required}. The workspace spends only its own allocation ledger; reduce the \
             order or cancel resting orders to free reserves.{stale}",
            funding = shortfall.funding,
            workspace = manifest.name,
            stale = if sync.reconciled() {
                ""
            } else {
                " The ledger could not be fully confirmed against the venue, so the available \
                 figure may be stale."
            }
        ),
        refusal: Box::new(crate::errors::Refusal::AllocationBudget {
            workspace: manifest.name.clone(),
            funding: shortfall.funding,
            available,
            required,
            reconciled: sync.reconciled(),
        }),
    })
}

/// A spend the ledger cannot cover, in the order's funding asset.
#[derive(Debug)]
struct Shortfall {
    funding: String,
    required: Decimal,
    available: Decimal,
}

/// The order's spend set against what the ledger holds free of reservations:
/// `Some` when the ledger cannot fund it, `None` when it can.
///
/// Fee-free on purpose, matching the reservations `commitment` books in the
/// journal mirror: the venue charges its own fee, and padding by the contract's
/// simulated rate would refuse orders the venue itself accepts — so this
/// figure and `workspace balance` always agree.
fn allocation_shortfall(
    state: &kraken_paper::PaperState,
    side: OrderSide,
    canonical_pair: &str,
    volume: Decimal,
    price: Decimal,
) -> Result<Option<Shortfall>> {
    let (funding, required) = spend_requirement(side, canonical_pair, volume, price)?;
    let available = state.available_balance(&funding);
    Ok((required > available).then_some(Shortfall {
        funding,
        required,
        available,
    }))
}

/// What the order spends, in which asset: a buy spends the quote at
/// `volume × price`, a sell spends the base by volume.
fn spend_requirement(
    side: OrderSide,
    canonical_pair: &str,
    volume: Decimal,
    price: Decimal,
) -> Result<(String, Decimal)> {
    let (_, base, quote) = kraken_paper::parse_pair(canonical_pair)?;
    match side {
        OrderSide::Buy => {
            let required = volume.checked_mul(price).ok_or_else(|| {
                KrakenError::Validation("order volume times price overflows".into())
            })?;
            Ok((quote, required))
        }
        OrderSide::Sell => Ok((base, volume)),
    }
}

/// The explicit paper mapping: market/limit, price, --validate, and
/// --reason cross 1:1; any other set flag is refused naming the parameter —
/// honest validation, never silent degradation.
fn ensure_paper_mappable(
    order: &OrderRequest,
    manifest: &kraken_workspace::WorkspaceManifest,
) -> Result<()> {
    let unsupported: &[(&str, bool)] = &[
        ("--price2", order.price2.is_some()),
        ("--displayvol", order.displayvol.is_some()),
        ("--trigger", order.trigger.is_some()),
        ("--leverage", order.leverage.is_some()),
        ("--reduce-only", order.reduce_only),
        ("--timeinforce", order.timeinforce.is_some()),
        ("--start-time", order.start_time.is_some()),
        ("--expire-time", order.expire_time.is_some()),
        ("--userref", order.userref.is_some()),
        ("--cl-ord-id", order.cl_ord_id.is_some()),
        ("--oflags", order.oflags.is_some()),
        ("--stptype", order.stptype.is_some()),
        ("--close-ordertype", order.close_ordertype.is_some()),
        ("--close-price", order.close_price.is_some()),
        ("--close-price2", order.close_price2.is_some()),
        ("--deadline", order.deadline.is_some()),
        ("--asset-class", order.asset_class.is_some()),
    ];
    if let Some((flag, _)) = unsupported.iter().find(|(_, set)| *set) {
        return Err(kraken_workspace::policy::unsupported(
            &format!("order {flag}"),
            &manifest.name,
            manifest.mode,
        )
        .into());
    }
    if !order.r#type.eq_ignore_ascii_case("market") && !order.r#type.eq_ignore_ascii_case("limit") {
        return Err(kraken_workspace::policy::unsupported(
            &format!("order --type {}", order.r#type),
            &manifest.name,
            manifest.mode,
        )
        .into());
    }
    Ok(())
}

async fn paper_order(
    side: OrderSide,
    order: OrderRequest,
    manifest: &kraken_workspace::WorkspaceManifest,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    use kraken_workspace::receipt::{
        OrderReceipt, PaperFill, ReceiptCore, ReceiptStatus, TradeMode,
    };
    ensure_paper_mappable(&order, manifest)?;
    let routed = super::paper::RoutedOrder {
        side,
        pair: &order.pair,
        volume: &order.volume,
        order_type: &order.r#type,
        price: order.price.as_deref(),
        reason: order.reason.as_deref(),
    };
    if order.validate {
        super::paper::validate_routed_order(ctx, manifest, routed).await?;
        return receipt_output(OrderReceipt {
            core: ReceiptCore {
                mode: TradeMode::Paper,
                status: ReceiptStatus::Validated,
                order_id: None,
                // Canonical, matching the placed receipts: `--validate` must
                // report the pair the real order would carry.
                pair: Some(
                    kraken_paper::parse_pair(&order.pair)
                        .map_or_else(|_| order.pair.to_uppercase(), |(api, _, _)| api),
                ),
                side: Some(side),
                order_type: Some(order.r#type.to_lowercase()),
                volume: order.volume.parse().ok(),
                price: order.price.as_deref().and_then(|p| p.parse().ok()),
            },
            venue: None,
            paper: None,
        });
    }
    let placement = super::paper::place_routed_order(ctx, manifest, routed).await?;
    let receipt = match placement {
        super::paper::PaperPlacement::Limit { order: placed } => OrderReceipt {
            core: ReceiptCore {
                mode: TradeMode::Paper,
                status: ReceiptStatus::Submitted,
                order_id: Some(placed.id),
                pair: Some(placed.pair),
                side: Some(side),
                order_type: Some("limit".into()),
                volume: Some(placed.volume),
                price: Some(placed.price),
            },
            venue: None,
            paper: None,
        },
        super::paper::PaperPlacement::Market { trade } => OrderReceipt {
            core: ReceiptCore {
                mode: TradeMode::Paper,
                status: ReceiptStatus::Filled,
                order_id: Some(trade.order_id.clone()),
                pair: Some(trade.pair.clone()),
                side: Some(side),
                order_type: Some("market".into()),
                volume: Some(trade.volume),
                price: Some(trade.price),
            },
            venue: None,
            paper: Some(PaperFill {
                trade_id: Some(trade.id),
                fee: Some(trade.fee),
                cost: Some(trade.cost),
            }),
        },
    };
    // Show where the fill landed — the active session (or `none`) and the
    // workspace, the same Run/Workspace rows `explain pnl` stamps.
    let mut out = receipt_output(receipt)?;
    let run = match kraken_workspace::session::active(&ctx.scoped_base()?) {
        Ok(Some((session_id, _))) => session_id.to_string(),
        _ => "none".to_string(),
    };
    out.stamp_session(&run);
    out.stamp_workspace(&manifest.name);
    Ok(out)
}

async fn master_order(
    side: OrderSide,
    order: OrderRequest,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    // A validate-only request executes nothing, so no banner.
    if !order.validate {
        eprintln!("{LIVE_BANNER}");
    }
    let mut out = master_order_at(
        super::journal_mirror::master_journal_path()?,
        side,
        order,
        ctx,
    )
    .await?;
    // Name the scope rather than letting its absence imply it: a live workspace
    // adds `Scope: … allocation …` here, so an unlabelled table would leave the
    // widest-reaching order in the CLI as the one that says least about itself.
    out.stamp_context_row("Scope", "the WHOLE real account, no workspace limit");
    Ok(out)
}

/// [`master_order`] with a caller-chosen mirror journal — the unit-test seam.
async fn master_order_at(
    journal: std::path::PathBuf,
    side: OrderSide,
    order: OrderRequest,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    // Open the mirror before the venue call: a busy journal
    // refuses before money moves, and its writer lock serializes concurrent
    // trades. A validate-only request places nothing, so no mirror.
    let mirror = if order.validate {
        None
    } else {
        Some(super::journal_mirror::open_attached_at(journal.clone(), ctx).await?)
    };
    // No allocation gate on the whole-account path, so a market order stays
    // unvalued and books no reservation.
    master_order_on(mirror, &journal, side, order, None, ctx).await
}

/// [`master_order_at`] on an already-opened mirror — the live path opens it
/// before its funds gate so the check and the submission share one lock
/// window, leaving no gap for a concurrent order to pass against the same
/// ledger snapshot. `market_valuation` is the price that gate admitted a
/// market order at, mirrored as its reservation.
async fn master_order_on(
    mut mirror: Option<kraken_paper::account::PaperAccount>,
    journal: &std::path::Path,
    side: OrderSide,
    order: OrderRequest,
    market_valuation: Option<Decimal>,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    use kraken_workspace::receipt::{OrderReceipt, ReceiptCore, ReceiptStatus, TradeMode};
    let decision_scope = journal
        .parent()
        .ok_or_else(|| KrakenError::Config("journal path has no parent".into()))?
        .to_path_buf();
    let facts = (
        order.pair.clone(),
        order.volume.clone(),
        order.r#type.clone(),
        order.price.clone(),
        order.validate,
        order.reason.clone(),
    );
    let request = add_order_request(side, order)?;
    let added = ctx.spot_account()?.sign(&request).await?;

    let (pair, volume, order_type, price, validated, reason) = facts;
    let txid = added
        .txid
        .as_ref()
        .and_then(|ids| ids.first())
        .map(ToString::to_string);
    if let (Some(mirror), Some(txid)) = (mirror.as_mut(), txid.as_deref()) {
        let market = order_type.eq_ignore_ascii_case("market");
        let paper_type = if market {
            kraken_paper::PaperOrderType::Market
        } else {
            kraken_paper::PaperOrderType::Limit
        };
        super::journal_mirror::mirror_submitted(
            mirror,
            txid,
            side,
            &pair,
            paper_type,
            volume.parse().unwrap_or_default(),
            // A market order is valued at the gate's own estimate — never at
            // a --price the venue ignores, which would let a token figure
            // shrink the reservation.
            if market {
                market_valuation
            } else {
                price.as_deref().and_then(|p| p.parse().ok())
            },
        );
        if let Some(reason) = reason.as_deref() {
            // Best-effort like the paper path: the order is already real.
            if let Err(err) =
                super::paper::log_routed_decision_at(&decision_scope, side, &pair, txid, reason)
            {
                tracing::warn!(txid, %err, "live order placed but decision-log append failed");
            }
        }
    }
    receipt_output(OrderReceipt {
        core: ReceiptCore {
            mode: TradeMode::Live,
            status: if validated {
                ReceiptStatus::Validated
            } else {
                ReceiptStatus::Submitted
            },
            order_id: txid,
            // The same canonical form the paper receipt reports, so `pair` does
            // not change spelling when a strategy is promoted. Falls back to the
            // raw request for a pair the canonicaliser does not know, rather
            // than failing an order the venue already accepted.
            pair: Some(
                kraken_paper::parse_pair(&pair)
                    .map_or_else(|_| pair.to_uppercase(), |(api_pair, _, _)| api_pair),
            ),
            side: Some(side),
            order_type: Some(order_type.to_lowercase()),
            volume: volume.parse().ok(),
            price: price.as_deref().and_then(|p| p.parse().ok()),
        },
        venue: Some(published_json(&added)?),
        paper: None,
    })
}

/// Render an order receipt: one labelled table for paper and live alike.
///
/// `order buy` is the verb that survives promotion unchanged, so its receipt is
/// the one place a user most needs to see which kind of money just moved. The
/// untyped fallback this replaced dumped the JSON keys alphabetically and put
/// `mode: live` lowercase between `order_id` and `pair` — the single most
/// consequential fact in the CLI, rendered as its least prominent row.
fn receipt_output(receipt: kraken_workspace::receipt::OrderReceipt) -> Result<CommandOutput> {
    let data = serde_json::to_value(&receipt)
        .map_err(|e| KrakenError::Parse(format!("receipt serialization failed: {e}")))?;
    Ok(CommandOutput::key_value(receipt_rows(&receipt), data))
}

fn receipt_rows(receipt: &kraken_workspace::receipt::OrderReceipt) -> Vec<(String, String)> {
    use kraken_workspace::receipt::ReceiptStatus;
    let core = &receipt.core;
    let mut rows = vec![
        (
            "Mode".to_string(),
            crate::output::mode_label(core.mode).to_string(),
        ),
        ("Action".to_string(), receipt_action(core)),
    ];
    let mut push = |label: &str, value: Option<String>| {
        if let Some(value) = value {
            rows.push((label.to_string(), value));
        }
    };
    push("Order ID", core.order_id.clone());
    push("Pair", core.pair.clone());
    push("OrderSide", core.side.map(|s| s.to_string()));
    push("Type", core.order_type.clone());
    push(
        "Volume",
        core.volume.map(|v| format!("{:.8}", rounded(v, 8))),
    );
    push("Price", core.price.map(|p| format!("{:.2}", rounded(p, 2))));
    if let Some(fill) = &receipt.paper {
        push("Trade ID", fill.trade_id.clone());
        push("Fee", fill.fee.map(|f| format!("{:.4}", rounded(f, 4))));
        push("Cost", fill.cost.map(|c| format!("{:.2}", rounded(c, 2))));
    }
    if core.status == ReceiptStatus::Validated {
        rows.push((
            "Placed".to_string(),
            "no — --validate only checked the order".to_string(),
        ));
    }
    rows
}

/// The one-line "what just happened", worded as the paper verbs word it so the
/// two surfaces read alike.
fn receipt_action(core: &kraken_workspace::receipt::ReceiptCore) -> String {
    use kraken_workspace::receipt::ReceiptStatus;
    let side = core
        .side
        .map_or_else(|| "order".to_string(), |s| s.to_string());
    // Capitalized to read as the sentence the paper verbs already print
    // ("Market buy executed"), so the two surfaces sound like one CLI.
    let kind = match core.order_type.as_deref() {
        Some("market") => "Market",
        Some("limit") => "Limit",
        Some(other) => return format!("{other} {side}: {:?}", core.status),
        None => "Order",
    };
    match core.status {
        ReceiptStatus::Validated => format!("{kind} {side} validated, NOT placed"),
        // A market order the simulator handles comes back `Filled`, so this arm
        // is the venue's: it accepted the order but has not reported the fill
        // yet. Saying only "placed" next to paper's "executed" would read as a
        // market order that failed to fill.
        ReceiptStatus::Submitted if kind == "Market" => {
            format!("Market {side} placed at the venue; the fill lands on the next read")
        }
        ReceiptStatus::Submitted => format!("{kind} {side} order placed"),
        ReceiptStatus::Filled => format!("{kind} {side} executed"),
        ReceiptStatus::Cancelled => format!("{kind} {side} cancelled"),
    }
}

/// The command line's order, checked once into a request Kraken can only
/// accept or reject on its merits: every type-dependent price rule is enforced
/// by building the [`OrderKind`].
fn add_order_request(side: OrderSide, order: OrderRequest) -> Result<rest::AddOrder> {
    let OrderRequest {
        pair,
        volume,
        r#type,
        price: limit,
        // Decision-log rationale, consumed by the routing layer — never a
        // venue parameter.
        reason: _,
        price2,
        displayvol,
        trigger,
        leverage,
        reduce_only,
        timeinforce,
        start_time,
        expire_time,
        userref,
        cl_ord_id,
        oflags,
        stptype,
        close_ordertype,
        close_price,
        close_price2,
        deadline,
        validate,
        asset_class,
    } = order;
    let kind = order_kind(&r#type, limit, price2, displayvol, trigger)?;
    let tag = match (userref, cl_ord_id) {
        (Some(_), Some(_)) => {
            return Err(KrakenError::Validation(
                "userref and cl_ord_id are mutually exclusive".into(),
            ));
        }
        (Some(userref), None) => Some(OrderTag::UserRef(userref)),
        (None, Some(id)) => Some(OrderTag::ClientOrderId(id)),
        (None, None) => None,
    };
    let close = match (close_ordertype, close_price, close_price2) {
        (None, None, None) => None,
        (None, _, _) => {
            return Err(KrakenError::Validation(
                "close_ordertype is required when close_price or close_price2 is set".into(),
            ));
        }
        (Some(kind), close_price, close_price2) => Some(rest::CloseOrder {
            kind: rest_bridge::token("--close-ordertype", &kind, CLOSE_TYPES)?,
            price: close_price.as_deref().map(price).transpose()?,
            price2: close_price2.as_deref().map(price).transpose()?,
        }),
    };
    Ok(rest::AddOrder {
        pair: Pair::from(pair),
        side,
        volume: rest_bridge::amount("volume", &volume)?,
        kind,
        leverage,
        reduce_only,
        time_in_force: timeinforce
            .map(|tif| {
                rest_bridge::token("--timeinforce", &tif.to_uppercase(), "GTC, IOC, GTD, FOK")
            })
            .transpose()?,
        starttm: start_time,
        expiretm: expire_time,
        tag,
        oflags: oflags
            .map(|flags| {
                rest_bridge::token::<CommaList<_>>(
                    "--oflags",
                    &flags,
                    "post, fcib, fciq, nompp, viqc",
                )
            })
            .transpose()?
            .unwrap_or_default(),
        stptype: stptype
            .map(|stp| {
                rest_bridge::token(
                    "--stptype",
                    &stp,
                    "cancel-newest, cancel-oldest, cancel-both",
                )
            })
            .transpose()?,
        close,
        deadline,
        validate,
        asset_class: asset_class.as_deref().map(rest_bridge::open_asset_class),
    })
}

const CLOSE_TYPES: &str = "limit, iceberg, stop-loss, take-profit, stop-loss-limit, \
                           take-profit-limit, trailing-stop, trailing-stop-limit";
const TWO_PRICE_TYPES: [Type; 3] = [
    Type::StopLossLimit,
    Type::TakeProfitLimit,
    Type::TrailingStopLimit,
];
const TRIGGERED_TYPES: [Type; 6] = [
    Type::StopLoss,
    Type::StopLossLimit,
    Type::TakeProfit,
    Type::TakeProfitLimit,
    Type::TrailingStop,
    Type::TrailingStopLimit,
];

/// One order type and the prices it takes. The rules Kraken applies per type
/// (a limit needs a price, only a stop has a trigger) decide which variant can
/// be built, so a request that breaks one never reaches the exchange.
fn order_kind(
    order_type: &str,
    limit: Option<String>,
    price2: Option<String>,
    displayvol: Option<String>,
    trigger: Option<String>,
) -> Result<OrderKind> {
    let required = |value: Option<String>, what: &str| {
        value.ok_or_else(|| {
            KrakenError::Validation(format!("{what} is required for {order_type} orders"))
        })
    };
    let kind: Type = rest_bridge::token("--type", order_type, &Type::iter().join(", "))?;
    if price2.is_some() && !TWO_PRICE_TYPES.contains(&kind) {
        return Err(KrakenError::Validation(format!(
            "--price2 is only valid for {} orders",
            TWO_PRICE_TYPES.iter().join(", ")
        )));
    }
    let trigger = match trigger {
        Some(_) if !TRIGGERED_TYPES.contains(&kind) => {
            return Err(KrakenError::Validation(format!(
                "--trigger is only valid for {} orders",
                TRIGGERED_TYPES.iter().join(", ")
            )));
        }
        Some(t) => Some(rest_bridge::token::<Trigger>(
            "--trigger",
            &t,
            "last, index",
        )?),
        None => None,
    };
    let first = move || required(limit, "--price").and_then(|p| price(&p));
    let second = move || required(price2, "--price2").and_then(|p| price(&p));
    Ok(match kind {
        Type::Market => OrderKind::Market,
        Type::Limit => OrderKind::Limit { price: first()? },
        Type::Iceberg => OrderKind::Iceberg {
            price: first()?,
            display_volume: rest_bridge::amount(
                "--displayvol",
                &required(displayvol, "--displayvol")?,
            )?,
        },
        Type::StopLoss => OrderKind::StopLoss {
            trigger_price: first()?,
            trigger,
        },
        Type::TakeProfit => OrderKind::TakeProfit {
            trigger_price: first()?,
            trigger,
        },
        Type::StopLossLimit => OrderKind::StopLossLimit {
            trigger_price: first()?,
            limit_price: second()?,
            trigger,
        },
        Type::TakeProfitLimit => OrderKind::TakeProfitLimit {
            trigger_price: first()?,
            limit_price: second()?,
            trigger,
        },
        Type::TrailingStop => OrderKind::TrailingStop {
            offset: first()?,
            trigger,
        },
        Type::TrailingStopLimit => OrderKind::TrailingStopLimit {
            offset: first()?,
            limit_offset: second()?,
            trigger,
        },
        Type::SettlePosition => OrderKind::SettlePosition { price: first()? },
    })
}

fn price(raw: &str) -> Result<OrderPrice> {
    raw.parse()
        .map_err(|e: rest::InvalidPrice| KrakenError::Validation(e.to_string()))
}

fn amend_target(txid: Option<String>, cl_ord_id: Option<String>) -> Result<OrderRef> {
    match (txid, cl_ord_id) {
        (Some(txid), None) => Ok(OrderRef::Txid(txid.into())),
        (None, Some(id)) => Ok(OrderRef::ClientOrderId(id)),
        (None, None) => Err(KrakenError::Validation(
            "Either --txid or --cl-ord-id is required for amend".into(),
        )),
        (Some(_), Some(_)) => Err(KrakenError::Validation(
            "Only one of --txid or --cl-ord-id should be provided".into(),
        )),
    }
}

/// A batch file: Kraken's order objects, 2 to 15 of them.
fn batch_orders(contents: &str) -> Result<Vec<rest::BatchOrder>> {
    let orders: Vec<rest::BatchOrder> = serde_json::from_str(contents).map_err(|e| {
        KrakenError::Validation(format!(
            "Batch file must contain a JSON array of orders: {e}"
        ))
    })?;
    if !(2..=15).contains(&orders.len()) {
        return Err(KrakenError::Validation(
            "Batch must contain 2-15 orders".into(),
        ));
    }
    Ok(orders)
}

fn validate_cancel_batch(txids: &[String], cl_ord_ids: &[String]) -> Result<()> {
    if txids.is_empty() && cl_ord_ids.is_empty() {
        return Err(KrakenError::Validation(
            "Provide txid(s) or --cl-ord-ids to cancel".into(),
        ));
    }
    if txids.len() + cl_ord_ids.len() > 50 {
        return Err(KrakenError::Validation(
            "Cancel batch supports at most 50 orders total".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// The banner is agent-facing copy quoted by skills — byte-stable.
    #[test]
    fn live_banner_copy_is_pinned() {
        assert_eq!(LIVE_BANNER, "live: this goes to the real Kraken account");
    }

    /// An over-allocated workspace must not hold a user in a position. The exit
    /// is whichever side pays the allocated asset back in, which is the buy for
    /// a BTC-denominated workspace and the sell for a USD one — so a rule keyed
    /// on the side alone would trap one of them and wave the other through.
    #[test]
    fn the_order_that_recovers_the_allocated_asset_is_the_one_exempted() {
        // USD workspace holding BTC: selling BTC/USD is the way back to USD.
        assert!(acquires(OrderSide::Sell, "BTC/USD", "USD").unwrap());
        assert!(!acquires(OrderSide::Buy, "BTC/USD", "USD").unwrap());

        // BTC workspace holding USD: the exit is the mirror image.
        assert!(acquires(OrderSide::Buy, "BTC/USD", "BTC").unwrap());
        assert!(!acquires(OrderSide::Sell, "BTC/USD", "BTC").unwrap());
    }

    /// A pair carrying the allocated asset on neither leg cannot reduce the
    /// shortfall, so it is treated as spending rather than as an exit.
    #[test]
    fn a_pair_that_cannot_reduce_the_shortfall_is_not_an_exit() {
        for side in [OrderSide::Buy, OrderSide::Sell] {
            assert!(
                !acquires(side, "ETH/BTC", "USD").unwrap(),
                "{side} ETH/BTC does not pay USD back in"
            );
        }
    }

    /// The venue spells pairs several ways; the exemption cannot depend on
    /// which.
    #[test]
    fn the_exemption_survives_the_venues_pair_spellings() {
        for pair in ["BTC/USD", "XBTUSD", "XXBTZUSD", "btcusd"] {
            assert!(
                acquires(OrderSide::Sell, pair, "USD").unwrap(),
                "selling {pair} recovers USD"
            );
        }
    }

    fn funded_state(balance: rust_decimal::Decimal, currency: &str) -> kraken_paper::PaperState {
        kraken_paper::PaperState::with_config(kraken_paper::PaperConfig {
            balance,
            currency: currency.into(),
            fee_rate: rust_decimal_macros::dec!(0.0026),
            slippage_rate: rust_decimal_macros::dec!(0),
        })
    }

    /// An absurd order must fail closed as a refusal, never a panic.
    #[test]
    fn an_overflowing_notional_is_a_refusal_not_a_panic() {
        use rust_decimal_macros::dec;
        let state = funded_state(dec!(100), "USD");
        let err = allocation_shortfall(
            &state,
            OrderSide::Buy,
            "BTC/USD",
            rust_decimal::Decimal::MAX,
            dec!(2),
        )
        .expect_err("volume times price overflows Decimal");
        assert!(err.to_string().contains("overflows"), "{err}");
    }

    /// `order buy` is the verb that survives promotion unchanged, so its
    /// receipt is where a user finds out which kind of money moved. Paper
    /// and live must render the same table shape, and the mode row must
    /// lead it in both — the difference between them cannot be a lowercase
    /// word further down.
    #[test]
    fn an_order_receipt_leads_with_the_mode_in_both_modes() {
        use kraken_workspace::receipt::{OrderReceipt, ReceiptCore, ReceiptStatus, TradeMode};
        let receipt = |mode| OrderReceipt {
            core: ReceiptCore {
                mode,
                status: ReceiptStatus::Submitted,
                order_id: Some("TX-1".into()),
                pair: Some("BTC/USD".into()),
                side: Some(OrderSide::Buy),
                order_type: Some("limit".into()),
                volume: "0.01".parse().ok(),
                price: "60000".parse().ok(),
            },
            venue: None,
            paper: None,
        };

        let paper = receipt_rows(&receipt(TradeMode::Paper));
        let live = receipt_rows(&receipt(TradeMode::Live));
        assert_eq!(paper[0].0, "Mode");
        assert_eq!(live[0].0, "Mode");
        assert!(paper[0].1.starts_with("[PAPER]"), "{}", paper[0].1);
        assert!(live[0].1.starts_with("[LIVE]"), "{}", live[0].1);
        assert_eq!(
            paper.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            live.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            "one table shape across the promotion boundary"
        );
        assert_eq!(paper[1].1, "Limit buy order placed");
    }

    /// `--validate` places nothing; the table has to say so rather than leave
    /// the reader to infer it from an absent order id.
    #[test]
    fn a_validated_receipt_says_nothing_was_placed() {
        use kraken_workspace::receipt::{OrderReceipt, ReceiptCore, ReceiptStatus, TradeMode};
        let rows = receipt_rows(&OrderReceipt {
            core: ReceiptCore {
                mode: TradeMode::Live,
                status: ReceiptStatus::Validated,
                order_id: None,
                pair: Some("BTC/USD".into()),
                side: Some(OrderSide::Buy),
                order_type: Some("market".into()),
                volume: "0.01".parse().ok(),
                price: None,
            },
            venue: None,
            paper: None,
        });
        assert_eq!(rows[1].1, "Market buy validated, NOT placed");
        assert!(
            rows.iter()
                .any(|(k, v)| k == "Placed" && v.starts_with("no"))
        );
    }

    fn order() -> OrderRequest {
        OrderRequest {
            pair: "XBTUSD".into(),
            volume: "0.01".into(),
            r#type: "limit".into(),
            price: Some("50000".into()),
            reason: None,
            price2: None,
            displayvol: None,
            trigger: None,
            leverage: None,
            reduce_only: false,
            timeinforce: None,
            start_time: None,
            expire_time: None,
            userref: None,
            cl_ord_id: None,
            oflags: None,
            stptype: None,
            close_ordertype: None,
            close_price: None,
            close_price2: None,
            deadline: None,
            validate: false,
            asset_class: None,
        }
    }

    fn body(a: OrderRequest) -> Value {
        serde_json::to_value(add_order_request(OrderSide::Buy, a).unwrap()).unwrap()
    }

    fn expect_err(a: OrderRequest, substr: &str) {
        let err = add_order_request(OrderSide::Buy, a)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(substr),
            "expected error containing {substr:?}, got: {err}"
        );
    }

    #[test]
    fn valid_limit_order() {
        let a = order();
        let params = body(a);
        assert_eq!(params["ordertype"], "limit");
        assert_eq!(params["price"], "50000");
    }

    #[test]
    fn valid_market_order_no_price() {
        let mut a = order();
        a.r#type = "market".into();
        a.price = None;
        body(a);
    }

    #[test]
    fn invalid_order_type() {
        let mut a = order();
        a.r#type = "bogus".into();
        expect_err(a, "--type \"bogus\" is not one of");
    }

    #[test]
    fn an_order_type_is_matched_exactly() {
        let mut a = order();
        a.r#type = "LIMIT".into();
        expect_err(a, "--type \"LIMIT\" is not one of");
    }

    #[test]
    fn price_required_for_limit() {
        let mut a = order();
        a.price = None;
        expect_err(a, "--price is required");
    }

    #[test]
    fn iceberg_requires_displayvol() {
        let mut a = order();
        a.r#type = "iceberg".into();
        expect_err(a, "--displayvol is required for iceberg");
    }

    #[test]
    fn iceberg_with_displayvol_ok() {
        let mut a = order();
        a.r#type = "iceberg".into();
        a.displayvol = Some("0.001".into());
        let params = body(a);
        assert_eq!(params["displayvol"], "0.001");
    }

    #[test]
    fn price2_rejected_on_limit_order() {
        let mut a = order();
        a.price2 = Some("60000".into());
        expect_err(a, "--price2 is only valid for");
    }

    #[test]
    fn price2_accepted_on_stop_loss_limit() {
        let mut a = order();
        a.r#type = "stop-loss-limit".into();
        a.price2 = Some("49000".into());
        let params = body(a);
        assert_eq!(params["price2"], "49000");
    }

    #[test]
    fn price2_accepted_on_take_profit_limit() {
        let mut a = order();
        a.r#type = "take-profit-limit".into();
        a.price2 = Some("55000".into());
        body(a);
    }

    #[test]
    fn price2_accepted_on_trailing_stop_limit() {
        let mut a = order();
        a.r#type = "trailing-stop-limit".into();
        a.price2 = Some("100".into());
        body(a);
    }

    #[test]
    fn trigger_rejected_on_limit_order() {
        let mut a = order();
        a.trigger = Some("last".into());
        expect_err(a, "--trigger is only valid for");
    }

    #[test]
    fn trigger_accepted_on_stop_loss() {
        let mut a = order();
        a.r#type = "stop-loss".into();
        a.trigger = Some("last".into());
        let params = body(a);
        assert_eq!(params["trigger"], "last");
    }

    #[test]
    fn trigger_accepted_on_trailing_stop() {
        let mut a = order();
        a.r#type = "trailing-stop".into();
        a.trigger = Some("index".into());
        body(a);
    }

    #[test]
    fn trigger_invalid_value() {
        let mut a = order();
        a.r#type = "stop-loss".into();
        a.trigger = Some("las".into());
        expect_err(a, "--trigger \"las\" is not one of");
    }

    #[test]
    fn trigger_value_index_ok() {
        let mut a = order();
        a.r#type = "take-profit".into();
        a.trigger = Some("index".into());
        body(a);
    }

    #[test]
    fn userref_and_cl_ord_id_mutually_exclusive() {
        let mut a = order();
        a.userref = Some(12345);
        a.cl_ord_id = Some("my-order".into());
        expect_err(a, "userref and cl_ord_id are mutually exclusive");
    }

    #[test]
    fn userref_alone_ok() {
        let mut a = order();
        a.userref = Some(12345);
        let params = body(a);
        assert_eq!(params["userref"], 12345);
        assert!(params.get("cl_ord_id").is_none());
    }

    #[test]
    fn cl_ord_id_alone_ok() {
        let mut a = order();
        a.cl_ord_id = Some("my-order".into());
        let params = body(a);
        assert_eq!(params["cl_ord_id"], "my-order");
        assert!(params.get("userref").is_none());
    }

    #[test]
    fn close_price_without_close_ordertype() {
        let mut a = order();
        a.close_price = Some("55000".into());
        expect_err(a, "close_ordertype is required");
    }

    #[test]
    fn close_price2_without_close_ordertype() {
        let mut a = order();
        a.close_price2 = Some("60000".into());
        expect_err(a, "close_ordertype is required");
    }

    #[test]
    fn close_order_with_ordertype_ok() {
        let mut a = order();
        a.close_ordertype = Some("limit".into());
        a.close_price = Some("55000".into());
        let params = body(a);
        assert_eq!(params["close"]["ordertype"], "limit");
        assert_eq!(params["close"]["price"], "55000");
    }

    #[test]
    fn all_optional_params_mapped() {
        let mut a = order();
        a.leverage = Some("2".into());
        a.reduce_only = true;
        a.timeinforce = Some("GTC".into());
        a.start_time = Some("0".into());
        a.expire_time = Some("+3600".into());
        a.oflags = Some("post,fcib".into());
        a.stptype = Some("cancel-newest".into());
        a.deadline = Some("2026-03-01T18:00:00Z".into());
        a.validate = true;
        a.cl_ord_id = Some("test-123".into());
        a.asset_class = Some("tokenized_asset".into());
        let params = body(a);
        assert_eq!(params["leverage"], "2");
        assert_eq!(params["reduce_only"], true);
        assert_eq!(params["timeinforce"], "GTC");
        assert_eq!(params["starttm"], "0");
        assert_eq!(params["expiretm"], "+3600");
        assert_eq!(params["oflags"], "post,fcib");
        assert_eq!(params["stptype"], "cancel-newest");
        assert_eq!(params["deadline"], "2026-03-01T18:00:00Z");
        assert_eq!(params["validate"], true);
        assert_eq!(params["cl_ord_id"], "test-123");
        assert_eq!(params["asset_class"], "tokenized_asset");
    }

    #[test]
    fn amend_requires_at_least_one_id() {
        let err = amend_target(None, None).unwrap_err().to_string();
        assert!(err.contains("Either --txid or --cl-ord-id is required"));
    }

    #[test]
    fn amend_rejects_both_ids() {
        let err = amend_target(Some("OXXXXX".into()), Some("my-id".into()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("Only one of --txid or --cl-ord-id"));
    }

    #[test]
    fn amend_txid_only_ok() {
        amend_target(Some("OXXXXX".into()), None).unwrap();
    }

    #[test]
    fn amend_cl_ord_id_only_ok() {
        amend_target(None, Some("my-id".into())).unwrap();
    }

    fn batch_of(count: usize) -> String {
        let order =
            r#"{"pair":"BTCUSD","ordertype":"limit","type":"buy","volume":"0.01","price":"50000"}"#;
        format!("[{}]", vec![order; count].join(","))
    }

    #[test]
    fn batch_rejects_non_array() {
        let err = batch_orders(r#"{"not": "an array"}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("JSON array"));
    }

    #[test]
    fn batch_rejects_single_order() {
        let err = batch_orders(&batch_of(1)).unwrap_err().to_string();
        assert!(err.contains("2-15 orders"));
    }

    #[test]
    fn batch_rejects_sixteen_orders() {
        let err = batch_orders(&batch_of(16)).unwrap_err().to_string();
        assert!(err.contains("2-15 orders"));
    }

    #[test]
    fn batch_accepts_two_orders() {
        assert_eq!(batch_orders(&batch_of(2)).unwrap().len(), 2);
    }

    #[test]
    fn batch_accepts_fifteen_orders() {
        assert_eq!(batch_orders(&batch_of(15)).unwrap().len(), 15);
    }

    #[test]
    fn a_misspelt_batch_field_names_itself() {
        let file = r#"[{"ordertype":"limit","type":"buy","volume":"1","pirce":"5"},{"ordertype":"market","type":"buy","volume":"1"}]"#;
        let err = batch_orders(file).unwrap_err().to_string();
        assert!(err.contains("pirce"), "{err}");
    }

    #[test]
    fn a_two_price_stop_needs_its_limit() {
        let mut a = order();
        a.r#type = "stop-loss-limit".into();
        expect_err(a, "--price2 is required for stop-loss-limit orders");
    }

    #[test]
    fn a_relative_price_is_sent_verbatim() {
        let mut a = order();
        a.r#type = "trailing-stop".into();
        a.price = Some("+50".into());
        assert_eq!(body(a)["price"], "+50");
    }

    #[test]
    fn a_malformed_price_is_refused_before_the_venue() {
        let mut a = order();
        a.price = Some("50,000".into());
        expect_err(a, "is not a price");
    }

    #[test]
    fn cancel_batch_rejects_empty() {
        let err = validate_cancel_batch(&[], &[]).unwrap_err().to_string();
        assert!(err.contains("Provide txid(s) or --cl-ord-ids"));
    }

    #[test]
    fn cancel_batch_rejects_over_fifty() {
        let txids: Vec<String> = (0..30).map(|i| format!("TX{i}")).collect();
        let cl_ids: Vec<String> = (0..21).map(|i| format!("CL{i}")).collect();
        let err = validate_cancel_batch(&txids, &cl_ids)
            .unwrap_err()
            .to_string();
        assert!(err.contains("at most 50"));
    }

    #[test]
    fn cancel_batch_txids_only_ok() {
        let txids = vec!["TX1".into(), "TX2".into()];
        validate_cancel_batch(&txids, &[]).unwrap();
    }

    #[test]
    fn cancel_batch_cl_ord_ids_only_ok() {
        let cl_ids = vec!["CL1".into()];
        validate_cancel_batch(&[], &cl_ids).unwrap();
    }

    #[test]
    fn cancel_batch_mixed_within_limit() {
        let txids: Vec<String> = (0..25).map(|i| format!("TX{i}")).collect();
        let cl_ids: Vec<String> = (0..25).map(|i| format!("CL{i}")).collect();
        validate_cancel_batch(&txids, &cl_ids).unwrap();
    }

    /// Pins venue truth flowing through [`master_order_at`] into the
    /// receipt envelope and the mirror journal against a mock venue.
    mod mirror_wiring {
        use super::{
            super::{cancel_at, master_order_at},
            order,
        };
        pub(super) use crate::commands::test_support::venue_ctx;
        use kraken_core::OrderSide;
        use kraken_paper::{
            AccountEvent,
            account::{Origin, PaperAccount},
        };
        use rust_decimal_macros::dec;
        use serde_json::json;
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{header, method, path},
        };

        #[tokio::test]
        async fn master_order_mirrors_the_venue_txid_and_returns_a_live_receipt() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/0/private/Balance"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": { "ZUSD": "1000.00" }
                })))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/0/private/AddOrder"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": {
                        "descr": { "order": "buy 0.01000000 XBTUSD @ limit 50000.0" },
                        "txid": ["OTEST1-MIRROR-00001"]
                    }
                })))
                .expect(1)
                .mount(&server)
                .await;

            let dir = tempfile::tempdir().unwrap();
            let journal = dir.path().join("journal.jsonl");
            let ctx = venue_ctx(server.uri(), "test-api-key");
            let out = master_order_at(journal.clone(), OrderSide::Buy, order(), &ctx)
                .await
                .unwrap();

            // The unified envelope: identity in the core, venue verbatim.
            assert_eq!(out.data["mode"], "live");
            assert_eq!(out.data["status"], "submitted");
            assert_eq!(out.data["order_id"], "OTEST1-MIRROR-00001");
            assert_eq!(out.data["venue"]["txid"][0], "OTEST1-MIRROR-00001");

            let events: Vec<serde_json::Value> = std::fs::read_to_string(&journal)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
            assert_eq!(events[0]["event"], "attached", "anchor precedes the order");
            let submitted = events
                .iter()
                .find(|e| e["event"] == "order_submitted")
                .expect("venue order mirrored");
            assert_eq!(submitted["order"]["id"], "OTEST1-MIRROR-00001");
            // The workspace's own commitment, so `balance` reports what is left
            // to spend: 0.01 × 50000 of the funding asset. Kraken's separate
            // hold against the account is not this figure and is not knowable
            // here — recording zero read as "nothing is committed".
            assert_eq!(submitted["order"]["reserved_amount"], "500.00");
            assert_eq!(submitted["order"]["reserved_asset"], "USD");
        }

        #[tokio::test]
        async fn caller_chosen_journal_isolates_attribution_on_the_same_account() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/0/private/Balance"))
                .and(header("API-Key", "shared-account-key"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": { "ZUSD": "1000.00" }
                })))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/0/private/AddOrder"))
                .and(header("API-Key", "shared-account-key"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": { "descr": { "order": "buy 0.001 BTCUSD @ limit 50000" }, "txid": ["OWORKA-00001"] }
                })))
                .expect(1)
                .mount(&server)
                .await;

            let dir = tempfile::tempdir().unwrap();
            let journal_a = dir.path().join("workspace-a.jsonl");
            let journal_b = dir.path().join("workspace-b.jsonl");
            let ctx = venue_ctx(server.uri(), "shared-account-key");

            master_order_at(journal_a.clone(), OrderSide::Buy, order(), &ctx)
                .await
                .expect("workspace A order");

            server.reset().await;
            Mock::given(method("POST"))
                .and(path("/0/private/Balance"))
                .and(header("API-Key", "shared-account-key"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": { "ZUSD": "1000.00" }
                })))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/0/private/AddOrder"))
                .and(header("API-Key", "shared-account-key"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": { "descr": { "order": "buy 0.001 BTCUSD @ limit 50000" }, "txid": ["OWORKB-00001"] }
                })))
                .expect(1)
                .mount(&server)
                .await;
            master_order_at(journal_b.clone(), OrderSide::Buy, order(), &ctx)
                .await
                .expect("workspace B order");

            let a = std::fs::read_to_string(journal_a).unwrap();
            let b = std::fs::read_to_string(journal_b).unwrap();
            assert!(a.contains("OWORKA-00001"));
            assert!(!a.contains("OWORKB-00001"));
            assert!(b.contains("OWORKB-00001"));
            assert!(!b.contains("OWORKA-00001"));
        }

        /// The venue answering "no such order" means there is nothing left to
        /// cancel there, so a flatten must stop failing — but it is NOT proof
        /// the order never filled (Kraken answers the same for one that
        /// closed a moment ago). The row therefore stays in the ledger,
        /// where `QueryOrders` keeps being asked about it. Retiring it
        /// here would drop a real execution, which is the whole reason
        /// this is not a cancel.
        #[tokio::test]
        async fn a_cancel_the_venue_has_no_record_of_stops_failing_but_keeps_the_row() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/0/private/CancelOrder"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": ["EOrder:Unknown order"], "result": null
                })))
                .mount(&server)
                .await;

            let dir = tempfile::tempdir().unwrap();
            let journal = dir.path().join("journal.jsonl");
            let mut account = PaperAccount::open_at(journal.clone(), Origin::Cli).unwrap();
            let records =
                account.stamp(vec![AccountEvent::Initialized(kraken_paper::PaperConfig {
                    balance: dec!(10000),
                    currency: "USD".into(),
                    fee_rate: dec!(0.0026),
                    slippage_rate: dec!(0),
                })]);
            account.append(&records).unwrap();
            crate::commands::journal_mirror::mirror_submitted(
                &mut account,
                "OGHOST-00001-AAAAAA",
                OrderSide::Buy,
                "BTC/USD",
                kraken_paper::PaperOrderType::Limit,
                dec!(0.001),
                Some(dec!(30000)),
            );
            drop(account);

            let ctx = venue_ctx(server.uri(), "shared-account-key");
            let out = cancel_at(
                journal.clone(),
                dir.path(),
                vec!["OGHOST-00001-AAAAAA".into()],
                None,
                None,
                &ctx,
                Some("prod"),
            )
            .await
            .expect("a sweep the venue cannot act on is not a failure");

            assert_eq!(out.data["count"], 0, "nothing was cancelled");
            assert_eq!(out.data["already_gone_at_venue"][0], "OGHOST-00001-AAAAAA");

            let reopened = PaperAccount::open_at(journal, Origin::Cli).unwrap();
            let open = &reopened.state().unwrap().open_orders;
            assert_eq!(
                open.len(),
                1,
                "the order must stay queryable so a fill it may have taken is not lost"
            );
        }

        /// A venue-accepted cancel is not proof nothing executed: Kraken
        /// reports a partly-filled order as `canceled` with `vol_exec` set,
        /// and that execution must reach the ledger. Booking a plain local
        /// cancel here used to release the whole reservation and lose the
        /// fill for good — reconciliation only asks about rows still open.
        #[tokio::test]
        async fn a_cancel_that_partly_filled_folds_the_execution_not_a_plain_cancel() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/0/private/CancelOrder"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [], "result": { "count": 1 }
                })))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/0/private/QueryOrders"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": {
                        "OPART-00001-AAAAAA": crate::commands::test_support::order_row(json!({
                            "status": "canceled", "vol": "0.15", "vol_exec": "0.10",
                            "cost": "5000.0", "fee": "13.00", "price": "50000"
                        }))
                    }
                })))
                .expect(1)
                .mount(&server)
                .await;

            let dir = tempfile::tempdir().unwrap();
            let journal = dir.path().join("journal.jsonl");
            let mut account = PaperAccount::open_at(journal.clone(), Origin::Cli).unwrap();
            let records =
                account.stamp(vec![AccountEvent::Initialized(kraken_paper::PaperConfig {
                    balance: dec!(10000),
                    currency: "USD".into(),
                    fee_rate: dec!(0.0026),
                    slippage_rate: dec!(0),
                })]);
            account.append(&records).unwrap();
            crate::commands::journal_mirror::mirror_submitted(
                &mut account,
                "OPART-00001-AAAAAA",
                OrderSide::Buy,
                "BTC/USD",
                kraken_paper::PaperOrderType::Limit,
                dec!(0.15),
                Some(dec!(50000)),
            );
            drop(account);

            let ctx = venue_ctx(server.uri(), "test-api-key");
            let out = cancel_at(
                journal.clone(),
                dir.path(),
                vec!["OPART-00001-AAAAAA".into()],
                None,
                None,
                &ctx,
                Some("prod"),
            )
            .await
            .expect("the venue accepted the cancel");
            assert_eq!(out.data["count"], 1);

            let reopened = PaperAccount::open_at(journal.clone(), Origin::Cli).unwrap();
            let state = reopened.state().unwrap();
            assert!(state.open_orders.is_empty(), "the row is terminal");
            drop(reopened);
            let events: Vec<serde_json::Value> = std::fs::read_to_string(&journal)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
            let fill = events
                .iter()
                .find(|e| e["event"] == "order_filled")
                .expect("the executed 0.10 is a real fill, not a cancel");
            assert_eq!(fill["trade"]["volume"], "0.10");
            assert_eq!(fill["trade"]["fee"], "13.00");
            assert!(
                !events.iter().any(|e| e["event"] == "order_cancelled"),
                "no plain cancel may shadow the execution"
            );
        }

        /// Kraken cancels asynchronously: a venue-accepted cancel whose
        /// QueryOrders answer is still non-terminal leaves the row open —
        /// inventing a terminal state for it would be a guess about real
        /// money, and the next sync folds the truth.
        #[tokio::test]
        async fn a_cancel_the_venue_still_reports_open_leaves_the_row_open() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/0/private/CancelOrder"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [], "result": { "count": 1 }
                })))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/0/private/QueryOrders"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": {
                        "OSLOW0-00001-AAAAAA": crate::commands::test_support::order_row(json!({
                            "status": "open", "vol": "0.01", "vol_exec": "0",
                            "cost": "0", "fee": "0", "price": "30000"
                        }))
                    }
                })))
                .mount(&server)
                .await;

            let dir = tempfile::tempdir().unwrap();
            let journal = dir.path().join("journal.jsonl");
            let mut account = PaperAccount::open_at(journal.clone(), Origin::Cli).unwrap();
            let records =
                account.stamp(vec![AccountEvent::Initialized(kraken_paper::PaperConfig {
                    balance: dec!(10000),
                    currency: "USD".into(),
                    fee_rate: dec!(0.0026),
                    slippage_rate: dec!(0),
                })]);
            account.append(&records).unwrap();
            crate::commands::journal_mirror::mirror_submitted(
                &mut account,
                "OSLOW0-00001-AAAAAA",
                OrderSide::Buy,
                "BTC/USD",
                kraken_paper::PaperOrderType::Limit,
                dec!(0.01),
                Some(dec!(30000)),
            );
            drop(account);

            let ctx = venue_ctx(server.uri(), "test-api-key");
            let out = cancel_at(
                journal.clone(),
                dir.path(),
                vec!["OSLOW0-00001-AAAAAA".into()],
                None,
                None,
                &ctx,
                Some("prod"),
            )
            .await
            .expect("the venue accepted the cancel");
            assert_eq!(out.data["count"], 1);

            let reopened = PaperAccount::open_at(journal, Origin::Cli).unwrap();
            let state = reopened.state().unwrap();
            assert_eq!(
                state.open_orders.len(),
                1,
                "still working at the venue, so still open (and reserved) here"
            );
        }

        /// Without a workspace there is no ownership check, so an unrecognised
        /// transaction ID is a typo and must keep failing rather than report a
        /// successful flatten.
        #[tokio::test]
        async fn an_unknown_order_still_fails_the_sweep_without_a_workspace() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/0/private/CancelOrder"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": ["EOrder:Unknown order"], "result": null
                })))
                .mount(&server)
                .await;

            let dir = tempfile::tempdir().unwrap();
            let ctx = venue_ctx(server.uri(), "shared-account-key");
            let err = cancel_at(
                dir.path().join("journal.jsonl"),
                dir.path(),
                vec!["OTYPO-00000-XXXXXX".into()],
                None,
                None,
                &ctx,
                None,
            )
            .await
            .expect_err("a typo must not read as a completed flatten");

            assert!(err.to_string().contains("1 failed"), "{err}");
        }

        #[tokio::test]
        async fn a_cancel_rejects_an_order_owned_by_another_workspace_before_io() {
            let dir = tempfile::tempdir().unwrap();
            let journal = dir.path().join("workspace-b.jsonl");
            let mut account = PaperAccount::open_at(journal.clone(), Origin::Cli).unwrap();
            let records =
                account.stamp(vec![AccountEvent::Initialized(kraken_paper::PaperConfig {
                    balance: dec!(1000),
                    currency: "USD".into(),
                    fee_rate: dec!(0.0026),
                    slippage_rate: dec!(0),
                })]);
            account.append(&records).unwrap();
            drop(account);

            let ctx = venue_ctx("http://127.0.0.1:9".into(), "same-account-key");
            let err = cancel_at(
                journal,
                dir.path(),
                vec!["ORDER-FROM-WORKSPACE-A".into()],
                None,
                None,
                &ctx,
                Some("workspace-b"),
            )
            .await
            .expect_err("cross-workspace cancellation must refuse locally");

            assert!(err.to_string().contains("not an open order recorded"));
            assert!(err.to_string().contains("workspace-b"));
        }
    }

    /// The live allocation gate end to end over [`live_order_at`]: an order
    /// the workspace's own ledger cannot fund never reaches the venue —
    /// `expect(0)` on `AddOrder` is the proof — and the exit order is never
    /// refused. Drives the same wiring the CLI and MCP paths route through.
    mod allocation_gate {
        use super::{super::live_order_at, mirror_wiring::venue_ctx, order};
        use kraken_core::OrderSide;
        use kraken_paper::account::Origin;
        use rust_decimal::Decimal;
        use rust_decimal_macros::dec;
        use serde_json::json;
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };

        fn live_workspace(
            base: &std::path::Path,
            capital: Decimal,
        ) -> kraken_workspace::WorkspaceManifest {
            live_workspace_in(base, capital, "USD")
        }

        fn live_workspace_in(
            base: &std::path::Path,
            capital: Decimal,
            currency: &str,
        ) -> kraken_workspace::WorkspaceManifest {
            kraken_workspace::Workspaces::new(base)
                .create(
                    kraken_workspace::CreateSpec {
                        name: "prod".parse().unwrap(),
                        capital,
                        currency: currency.into(),
                        mode: kraken_workspace::WorkspaceMode::Live,
                        fee_rate: dec!(0.0026),
                        slippage_rate: dec!(0),
                        allowed_pairs: None,
                    },
                    Origin::Cli,
                )
                .unwrap()
        }

        async fn mount_balance(server: &MockServer, usd: &str) {
            Mock::given(method("POST"))
                .and(path("/0/private/Balance"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": { "ZUSD": usd }
                })))
                .mount(server)
                .await;
        }

        async fn mount_add_order(server: &MockServer, txid: &str, expected_calls: u64) {
            Mock::given(method("POST"))
                .and(path("/0/private/AddOrder"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [], "result": { "descr": { "order": "buy BTCUSD" }, "txid": [txid] }
                })))
                .expect(expected_calls)
                .mount(server)
                .await;
        }

        fn sized(volume: &str, price: Option<&str>) -> kraken_contract::order::OrderRequest {
            let mut req = order();
            req.pair = "BTC/USD".into();
            req.volume = volume.into();
            match price {
                Some(price) => req.price = Some(price.into()),
                None => {
                    req.r#type = "market".into();
                    req.price = None;
                }
            }
            req
        }

        /// The demonstrated hole, closed: a 100 USD allocation on a 50,000 USD
        /// account must not fund a 300,000 USD order.
        #[tokio::test]
        async fn an_order_beyond_the_allocation_never_reaches_the_venue() {
            let server = MockServer::start().await;
            mount_balance(&server, "50000.00").await;
            mount_add_order(&server, "ONEVER-00000-000000", 0).await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace(base.path(), dec!(100));
            let ctx = venue_ctx(server.uri(), "test-api-key");

            let err = live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("5", Some("60000")),
                &manifest,
                &ctx,
            )
            .await
            .expect_err("300000 USD does not fit a 100 USD allocation");

            let msg = err.to_string();
            assert!(
                msg.contains("Insufficient USD in live workspace 'prod'"),
                "{msg}"
            );
            assert!(msg.contains("available 100"), "{msg}");
            assert!(msg.contains("required 300000"), "{msg}");
            assert!(
                !msg.contains("may be stale"),
                "a fully reconciled refusal carries no staleness caveat: {msg}"
            );
            // The same facts as typed envelope fields — the agent-facing half
            // of the output contract, normalized (300000, never 300000.0).
            let envelope = err.to_json_envelope();
            assert_eq!(envelope["refusal"], "allocation_budget");
            assert_eq!(envelope["funding"], "USD");
            assert_eq!(envelope["available"], "100");
            assert_eq!(envelope["required"], "300000");
            assert_eq!(envelope["reconciled"], true);
        }

        /// The gate bounds the workspace; it does not disable it.
        #[tokio::test]
        async fn an_order_the_allocation_funds_is_submitted() {
            let server = MockServer::start().await;
            mount_balance(&server, "50000.00").await;
            mount_add_order(&server, "OFITS0-00001-AAAAAA", 1).await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace(base.path(), dec!(100));
            let ctx = venue_ctx(server.uri(), "test-api-key");

            let out = live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("0.002", Some("50000")),
                &manifest,
                &ctx,
            )
            .await
            .expect("100 USD funds a 100 USD order — the strict > comparison, as paper");
            assert_eq!(out.data["order_id"], "OFITS0-00001-AAAAAA");
            assert_eq!(out.data["backing"]["status"], "backed");
            // The receipt's spendable carries the order it reports: the whole
            // allocation just went on reserve, so the next order budgets
            // against 0 — not against the pre-order snapshot.
            assert_eq!(out.data["backing"]["spendable"], "0");
        }

        /// The exit doctrine carries over from `ensure_backed`: the order that
        /// pays the allocated asset back in is never refused, even when the
        /// ledger shows none of the asset being sold — a partial fill the
        /// ledger has not folded yet must stay exitable. The venue is the
        /// authority on whether the account can actually deliver it.
        #[tokio::test]
        async fn the_recovering_sell_is_exempt_even_with_an_empty_ledger() {
            let server = MockServer::start().await;
            mount_balance(&server, "50000.00").await;
            mount_add_order(&server, "OEXIT0-00001-AAAAAA", 1).await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace(base.path(), dec!(100));
            let ctx = venue_ctx(server.uri(), "test-api-key");

            let out = live_order_at(
                base.path(),
                OrderSide::Sell,
                sized("0.5", Some("60000")),
                &manifest,
                &ctx,
            )
            .await
            .expect("selling BTC pays the USD allocation back in; never refused");
            assert_eq!(out.data["order_id"], "OEXIT0-00001-AAAAAA");
        }

        /// An unpriceable market buy fails closed: it must not become the one
        /// order shape that bypasses the budget.
        #[tokio::test]
        async fn an_unpriceable_market_buy_fails_closed() {
            let server = MockServer::start().await;
            mount_balance(&server, "50000.00").await;
            mount_add_order(&server, "ONEVER-00000-000000", 0).await;
            // No Ticker mock: the quote fetch fails.
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace(base.path(), dec!(100));
            let ctx = venue_ctx(server.uri(), "test-api-key");

            live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("0.001", None),
                &manifest,
                &ctx,
            )
            .await
            .expect_err("no quote, no order — fail closed");
        }

        /// A resting order's reservation is spending already committed: the
        /// next order is checked against what is left, not the whole
        /// allocation.
        #[tokio::test]
        async fn a_resting_reservation_counts_against_the_next_order() {
            let server = MockServer::start().await;
            mount_balance(&server, "50000.00").await;
            mount_add_order(&server, "OREST0-00001-AAAAAA", 1).await;
            // The second order's sync asks the venue about the resting order;
            // still working, so its reservation stands.
            Mock::given(method("POST"))
                .and(path("/0/private/QueryOrders"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": {
                        "OREST0-00001-AAAAAA": crate::commands::test_support::order_row(json!({
                            "status": "open", "vol": "0.0012", "vol_exec": "0",
                            "cost": "0", "fee": "0", "price": "0"
                        }))
                    }
                })))
                .mount(&server)
                .await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace(base.path(), dec!(100));
            let ctx = venue_ctx(server.uri(), "test-api-key");

            // 0.0012 × 50000 = 60 of the 100 rests.
            live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("0.0012", Some("50000")),
                &manifest,
                &ctx,
            )
            .await
            .expect("60 USD fits the fresh allocation");

            // 0.001 × 50000 = 50 does not fit the 40 left.
            let err = live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("0.001", Some("50000")),
                &manifest,
                &ctx,
            )
            .await
            .expect_err("50 USD does not fit the 40 left free of the reservation");
            let msg = err.to_string();
            assert!(msg.contains("available 40"), "{msg}");
            assert!(msg.contains("required 50"), "{msg}");
        }

        /// A refusal off a ledger the venue could not confirm says so: the
        /// available figure may be stale, and hiding that would misdirect the
        /// user's next move.
        #[tokio::test]
        async fn an_unreconciled_refusal_discloses_staleness() {
            let server = MockServer::start().await;
            mount_balance(&server, "50000.00").await;
            mount_add_order(&server, "OSTALE-00001-AAAAAA", 1).await;
            // QueryOrders is unreachable for the second command's sync.
            Mock::given(method("POST"))
                .and(path("/0/private/QueryOrders"))
                .respond_with(ResponseTemplate::new(500))
                .mount(&server)
                .await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace(base.path(), dec!(100));
            let ctx = venue_ctx(server.uri(), "test-api-key");

            live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("0.0012", Some("50000")),
                &manifest,
                &ctx,
            )
            .await
            .expect("the first order fits");

            let err = live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("0.001", Some("50000")),
                &manifest,
                &ctx,
            )
            .await
            .expect_err("the reservation still counts on the stale ledger");
            assert!(err.to_string().contains("may be stale"), "{err}");
            assert_eq!(
                err.to_json_envelope()["reconciled"],
                false,
                "staleness is a typed field, not just prose"
            );
        }

        /// `--validate` is a dry run of the real submission, so it reports the
        /// same refusal the real order would hit.
        #[tokio::test]
        async fn a_validate_order_is_gated_the_same_way() {
            let server = MockServer::start().await;
            mount_balance(&server, "50000.00").await;
            mount_add_order(&server, "ONEVER-00000-000000", 0).await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace(base.path(), dec!(100));
            let ctx = venue_ctx(server.uri(), "test-api-key");

            let mut req = sized("5", Some("60000"));
            req.validate = true;
            let err = live_order_at(base.path(), OrderSide::Buy, req, &manifest, &ctx)
                .await
                .expect_err("a dry run reports what the real submission would hit");
            assert!(
                err.to_string()
                    .contains("Insufficient USD in live workspace 'prod'"),
                "{err}"
            );
        }

        /// A market buy is priced at the ask even when a `--price` came
        /// along: the venue ignores that price, so gating at it would let
        /// `--price 0.01` walk an arbitrarily large market order through.
        #[tokio::test]
        async fn a_market_buy_with_a_price_is_still_priced_at_the_ask() {
            let server = MockServer::start().await;
            mount_balance(&server, "50000.00").await;
            mount_add_order(&server, "ONEVER-00000-000000", 0).await;
            Mock::given(method("GET"))
                .and(path("/0/public/Ticker"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": {
                        "XXBTZUSD": crate::commands::test_support::ticker_row("60000.0", "59999.0")
                    }
                })))
                .expect(1)
                .mount(&server)
                .await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace(base.path(), dec!(100));
            let ctx = venue_ctx(server.uri(), "test-api-key");

            let mut req = sized("5", Some("0.01"));
            req.r#type = "market".into();
            let err = live_order_at(base.path(), OrderSide::Buy, req, &manifest, &ctx)
                .await
                .expect_err("5 BTC at the 60000 ask is 300000 USD, whatever --price says");
            assert!(err.to_string().contains("required 300000"), "{err}");
        }

        /// A limit buy without a price is refused before anything is opened
        /// or sent — the paper path refuses the same shape.
        #[tokio::test]
        async fn a_limit_buy_without_a_price_is_refused() {
            let server = MockServer::start().await;
            mount_balance(&server, "50000.00").await;
            mount_add_order(&server, "ONEVER-00000-000000", 0).await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace(base.path(), dec!(100));
            let ctx = venue_ctx(server.uri(), "test-api-key");

            let mut req = sized("1", None);
            req.r#type = "limit".into();
            let err = live_order_at(base.path(), OrderSide::Buy, req, &manifest, &ctx)
                .await
                .expect_err("a limit order names its price");
            assert!(err.to_string().contains("requires --price"), "{err}");
        }

        /// In a base-denominated workspace the SELL is the spend: the
        /// exemption is keyed on which leg the allocated asset sits on, and
        /// only a test in a non-USD workspace can tell that apart from a
        /// side-based rule.
        #[tokio::test]
        async fn a_base_denominated_workspace_gates_the_sell_and_exempts_the_buy() {
            let server = MockServer::start().await;
            // The account genuinely holds the BTC claim: the refusal below
            // must come from the allocation gate, not from ensure_backed.
            Mock::given(method("POST"))
                .and(path("/0/private/Balance"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": { "ZUSD": "50000.00", "XXBT": "10.0" }
                })))
                .mount(&server)
                .await;
            mount_add_order(&server, "OBASE0-00001-AAAAAA", 1).await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace_in(base.path(), dec!(2), "BTC");
            let ctx = venue_ctx(server.uri(), "test-api-key");

            let err = live_order_at(
                base.path(),
                OrderSide::Sell,
                sized("5", Some("60000")),
                &manifest,
                &ctx,
            )
            .await
            .expect_err("selling 5 BTC does not fit a 2 BTC allocation");
            let msg = err.to_string();
            assert!(msg.contains("Insufficient BTC"), "{msg}");
            assert!(msg.contains("available 2"), "{msg}");
            assert!(msg.contains("required 5"), "{msg}");

            // The exit is the buy here, exempt even with an empty USD ledger.
            live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("5", Some("60000")),
                &manifest,
                &ctx,
            )
            .await
            .expect("buying BTC pays the allocation back in; never refused");
        }

        /// A market order's reservation is booked at the ask estimate the
        /// gate admitted it at, so the receipt's re-read spendable already
        /// carries the spend — an agent is never told the money it just
        /// committed is still free.
        #[tokio::test]
        async fn a_market_receipt_deducts_its_own_priced_spend() {
            let server = MockServer::start().await;
            mount_balance(&server, "50000.00").await;
            mount_add_order(&server, "OMKT00-00001-AAAAAA", 1).await;
            Mock::given(method("GET"))
                .and(path("/0/public/Ticker"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": {
                        "XXBTZUSD": crate::commands::test_support::ticker_row("60000.0", "59999.0")
                    }
                })))
                .mount(&server)
                .await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace(base.path(), dec!(100));
            let ctx = venue_ctx(server.uri(), "test-api-key");

            // 0.0005 × 60000 ask = 30 USD of the 100.
            let out = live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("0.0005", None),
                &manifest,
                &ctx,
            )
            .await
            .expect("30 USD at the ask fits the allocation");
            assert_eq!(out.data["backing"]["spendable"], "70");
        }

        /// The demonstrated bypass, closed: while the venue still reports a
        /// market buy working, its ask-priced reservation stands, so a second
        /// order cannot pass the funds gate against the pre-spend ledger.
        #[tokio::test]
        async fn a_pending_market_buy_counts_against_the_next_order() {
            let server = MockServer::start().await;
            mount_balance(&server, "50000.00").await;
            mount_add_order(&server, "OMKTP0-00001-AAAAAA", 1).await;
            Mock::given(method("GET"))
                .and(path("/0/public/Ticker"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": {
                        "XXBTZUSD": crate::commands::test_support::ticker_row("60000.0", "59999.0")
                    }
                })))
                .mount(&server)
                .await;
            // The second order's sync asks about the pending market order;
            // nothing terminal has folded, so the reservation stands.
            Mock::given(method("POST"))
                .and(path("/0/private/QueryOrders"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": {
                        "OMKTP0-00001-AAAAAA": crate::commands::test_support::order_row(json!({
                            "status": "pending", "vol": "0.001", "vol_exec": "0",
                            "cost": "0", "fee": "0", "price": "0"
                        }))
                    }
                })))
                .mount(&server)
                .await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace(base.path(), dec!(100));
            let ctx = venue_ctx(server.uri(), "test-api-key");

            // 0.001 × 60000 ask = 60 of the 100.
            live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("0.001", None),
                &manifest,
                &ctx,
            )
            .await
            .expect("60 USD at the ask fits the fresh allocation");

            // Another 60 does not fit the 40 left free of the reservation.
            let err = live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("0.001", None),
                &manifest,
                &ctx,
            )
            .await
            .expect_err("a pending market spend is not free money");
            let msg = err.to_string();
            assert!(msg.contains("available 40"), "{msg}");
            assert!(msg.contains("required 60"), "{msg}");
        }

        /// A pending market sell reserves its base volume — known exactly,
        /// no estimate involved — so a base-denominated allocation cannot be
        /// spent twice while the venue still reports the first sell working.
        #[tokio::test]
        async fn a_pending_market_sell_counts_against_the_next_order() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/0/private/Balance"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": { "ZUSD": "50000.00", "XXBT": "10.0" }
                })))
                .mount(&server)
                .await;
            mount_add_order(&server, "OMKTS0-00001-AAAAAA", 1).await;
            Mock::given(method("POST"))
                .and(path("/0/private/QueryOrders"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": {
                        "OMKTS0-00001-AAAAAA": crate::commands::test_support::order_row(json!({
                            "status": "open", "vol": "1.5", "vol_exec": "0",
                            "cost": "0", "fee": "0", "price": "0"
                        }))
                    }
                })))
                .mount(&server)
                .await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace_in(base.path(), dec!(2), "BTC");
            let ctx = venue_ctx(server.uri(), "test-api-key");

            live_order_at(
                base.path(),
                OrderSide::Sell,
                sized("1.5", None),
                &manifest,
                &ctx,
            )
            .await
            .expect("1.5 BTC fits the fresh 2 BTC allocation");

            let err = live_order_at(
                base.path(),
                OrderSide::Sell,
                sized("1", None),
                &manifest,
                &ctx,
            )
            .await
            .expect_err("1 BTC does not fit the 0.5 left free of the pending sell");
            let msg = err.to_string();
            assert!(msg.contains("available 0.5"), "{msg}");
            assert!(msg.contains("required 1"), "{msg}");
        }

        /// The mirrored reservation of workspace "prod"'s one submitted
        /// order, as `(asset, amount)`.
        fn mirrored_reservation(base: &std::path::Path) -> (String, Decimal) {
            let journal =
                kraken_workspace::Workspaces::new(base).journal_path(&"prod".parse().unwrap());
            let submitted = std::fs::read_to_string(journal)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
                .find(|e| e["event"] == "order_submitted")
                .expect("venue order mirrored");
            (
                submitted["order"]["reserved_asset"]
                    .as_str()
                    .unwrap()
                    .into(),
                submitted["order"]["reserved_amount"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap(),
            )
        }

        /// The exit doctrine skips the refusal, not the ledger: a pending
        /// exit market sell still holds real funds at the venue, so it books
        /// its base volume — known exactly, no pricing round-trip — and the
        /// next order that spends that asset budgets against what is left.
        #[tokio::test]
        async fn an_exit_market_sell_reserves_its_base_volume() {
            let server = MockServer::start().await;
            mount_balance(&server, "50000.00").await;
            mount_add_order(&server, "OEXITM-00001-AAAAAA", 1).await;
            // No Ticker mock: the exit sell must not need one.
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace(base.path(), dec!(100));
            let ctx = venue_ctx(server.uri(), "test-api-key");

            live_order_at(
                base.path(),
                OrderSide::Sell,
                sized("0.5", None),
                &manifest,
                &ctx,
            )
            .await
            .expect("selling BTC pays the USD allocation back in; never refused");
            assert_eq!(
                mirrored_reservation(base.path()),
                ("BTC".to_string(), dec!(0.5))
            );
        }

        /// An exit market buy spends the quote leg, so its reservation needs
        /// a price: the venue's ask, fetched best-effort.
        #[tokio::test]
        async fn an_exit_market_buy_reserves_at_the_ask() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/0/private/Balance"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": { "ZUSD": "50000.00", "XXBT": "10.0" }
                })))
                .mount(&server)
                .await;
            mount_add_order(&server, "OEXITB-00001-AAAAAA", 1).await;
            Mock::given(method("GET"))
                .and(path("/0/public/Ticker"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": {
                        "XXBTZUSD": crate::commands::test_support::ticker_row("60000.0", "59999.0")
                    }
                })))
                .expect(1)
                .mount(&server)
                .await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace_in(base.path(), dec!(2), "BTC");
            let ctx = venue_ctx(server.uri(), "test-api-key");

            live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("0.001", None),
                &manifest,
                &ctx,
            )
            .await
            .expect("buying BTC pays the allocation back in; never refused");
            assert_eq!(
                mirrored_reservation(base.path()),
                ("USD".to_string(), dec!(60))
            );
        }

        /// An unpriceable exit market buy still reaches the venue: the exit
        /// doctrine outranks the reservation, which books nothing rather
        /// than blocking the way out. The gated shape of the same failure
        /// fails closed — see `an_unpriceable_market_buy_fails_closed`.
        #[tokio::test]
        async fn an_unpriceable_exit_market_buy_is_still_submitted() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/0/private/Balance"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": { "ZUSD": "50000.00", "XXBT": "10.0" }
                })))
                .mount(&server)
                .await;
            mount_add_order(&server, "OEXITU-00001-AAAAAA", 1).await;
            // No Ticker mock: the ask fetch fails.
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace_in(base.path(), dec!(2), "BTC");
            let ctx = venue_ctx(server.uri(), "test-api-key");

            live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("0.001", None),
                &manifest,
                &ctx,
            )
            .await
            .expect("an unreadable ask must not block the exit");
            let (_, amount) = mirrored_reservation(base.path());
            assert_eq!(amount, Decimal::ZERO, "unpriceable, so nothing is booked");
        }

        /// A workspace that converted part of its allocation into position
        /// stays backed: its claim on the account is what the ledger still
        /// holds of the allocation asset, not the original capital — the
        /// spent part left the account with the fill it funded.
        #[tokio::test]
        async fn an_invested_allocation_is_still_backed_by_what_remains() {
            let server = MockServer::start().await;
            // A tightly allocated account: it holds exactly the allocation,
            // then exactly what the ledger says is left after the fill.
            Mock::given(method("POST"))
                .and(path("/0/private/Balance"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": { "ZUSD": "100.00" }
                })))
                .up_to_n_times(1)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/0/private/Balance"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": { "ZUSD": "40.00" }
                })))
                .mount(&server)
                .await;
            mount_add_order(&server, "OINV00-00001-AAAAAA", 2).await;
            // The second order's sync folds the first order's fill: 60 USD
            // became BTC and left the account.
            Mock::given(method("POST"))
                .and(path("/0/private/QueryOrders"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "error": [],
                    "result": {
                        "OINV00-00001-AAAAAA": crate::commands::test_support::order_row(json!({
                            "status": "closed", "vol": "0.0012", "vol_exec": "0.0012",
                            "cost": "60.0", "fee": "0", "price": "50000"
                        }))
                    }
                })))
                .mount(&server)
                .await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace(base.path(), dec!(100));
            let ctx = venue_ctx(server.uri(), "test-api-key");

            live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("0.0012", Some("50000")),
                &manifest,
                &ctx,
            )
            .await
            .expect("60 USD fits the fresh allocation");

            // 30 of the 40 still in USD; charging the nominal 100 against
            // the account would refuse this as over-allocated.
            let out = live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("0.0006", Some("50000")),
                &manifest,
                &ctx,
            )
            .await
            .expect("the invested allocation is not an over-allocation");
            assert_eq!(out.data["backing"]["status"], "backed");
        }

        /// The over-allocated refusal end to end: the third refusal kind's
        /// envelope tag and backing payload are a routing contract like the
        /// other two.
        #[tokio::test]
        async fn an_over_allocated_refusal_carries_its_tag_and_backing() {
            let server = MockServer::start().await;
            // The account holds 50 against a 100 USD claim.
            mount_balance(&server, "50.00").await;
            mount_add_order(&server, "ONEVER-00000-000000", 0).await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace(base.path(), dec!(100));
            let ctx = venue_ctx(server.uri(), "test-api-key");

            let err = live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("0.0001", Some("50000")),
                &manifest,
                &ctx,
            )
            .await
            .expect_err("an over-allocated workspace stops spending");
            let envelope = err.to_json_envelope();
            assert_eq!(envelope["error"], "validation");
            assert_eq!(envelope["refusal"], "over_allocated");
            assert_eq!(envelope["workspace"], "prod");
            assert_eq!(envelope["backing"]["status"], "over_allocated");
            assert_eq!(envelope["backing"]["unallocated"], "-50.00");
        }

        /// An unreadable venue balance stops spending — the same direction
        /// `workspace create` fails on the same condition — but never the
        /// exit: an outage must not trap a position.
        #[tokio::test]
        async fn an_unverifiable_backing_stops_spending_but_never_the_exit() {
            let server = MockServer::start().await;
            // No Balance mock: the read fails, so backing is unverified.
            mount_add_order(&server, "OEXIT1-00001-AAAAAA", 1).await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace(base.path(), dec!(100));
            let ctx = venue_ctx(server.uri(), "test-api-key");

            let err = live_order_at(
                base.path(),
                OrderSide::Buy,
                sized("0.001", Some("50000")),
                &manifest,
                &ctx,
            )
            .await
            .expect_err("an order the allocation would fund still needs a readable account");
            assert!(
                err.to_string()
                    .contains("cannot verify the account behind its allocation"),
                "{err}"
            );
            let envelope = err.to_json_envelope();
            assert_eq!(envelope["refusal"], "unverified");
            assert!(
                envelope["backing"]["account"].is_null(),
                "the backing object rides the refusal: {envelope}"
            );

            live_order_at(
                base.path(),
                OrderSide::Sell,
                sized("0.001", Some("50000")),
                &manifest,
                &ctx,
            )
            .await
            .expect("the exit stays open through an outage");
        }

        /// A pair carrying the allocated asset on NEITHER leg is spending,
        /// and it is checked in the order's own funding asset — which the
        /// ledger holds none of. Paper refuses the same way.
        #[tokio::test]
        async fn a_pair_off_the_allocation_is_gated_in_its_own_funding_asset() {
            let server = MockServer::start().await;
            mount_balance(&server, "50000.00").await;
            mount_add_order(&server, "ONEVER-00000-000000", 0).await;
            let base = tempfile::tempdir().unwrap();
            let manifest = live_workspace(base.path(), dec!(100));
            let ctx = venue_ctx(server.uri(), "test-api-key");

            let mut req = sized("1", Some("0.05"));
            req.pair = "ETH/BTC".into();
            let err = live_order_at(base.path(), OrderSide::Buy, req, &manifest, &ctx)
                .await
                .expect_err("a USD ledger holds no BTC to fund an ETH/BTC buy");
            let msg = err.to_string();
            assert!(msg.contains("Insufficient BTC"), "{msg}");
            assert!(msg.contains("available 0"), "{msg}");
        }
    }
}
