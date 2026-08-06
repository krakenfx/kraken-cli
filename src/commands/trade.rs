//! Spot trading commands.

use std::collections::HashMap;
use std::path::PathBuf;

use clap::Subcommand;
use enum_dispatch::enum_dispatch;
use kraken_core::OrderSide;
use serde_json::Value;

use super::{Execute, workspace_guard};
use crate::cli::AppContext;
use crate::errors::{KrakenError, Result};
use crate::output::CommandOutput;

#[enum_dispatch(Execute)]
#[derive(Debug, Subcommand)]
pub(crate) enum OrderCommand {
    /// Place a buy order.
    Buy(Buy),
    /// Place a sell order.
    Sell(Sell),
    /// Submit batch orders from a JSON file (2-15 orders).
    Batch(Batch),
    /// Amend a live order in-place (preserves queue priority and identifiers).
    Amend(Amend),
    /// Edit (cancel+replace) an order.
    Edit(Edit),
    /// Cancel an open order by txid, userref, or client order ID.
    Cancel(Cancel),
    /// Cancel a batch of orders (max 50 total across txids and cl-ord-ids).
    CancelBatch(CancelBatch),
    /// Cancel all open orders.
    CancelAll(CancelAll),
    /// Dead man's switch — cancel all orders after timeout.
    CancelAfter(CancelAfter),
}

// Buy and sell take the same arguments; only the order side differs.
// `enum_dispatch` needs a distinct type per variant, so each side is a thin
// wrapper flattening the shared set.
#[derive(Debug, clap::Args)]
pub(crate) struct Buy {
    #[command(flatten)]
    order: OrderRequest,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Sell {
    #[command(flatten)]
    order: OrderRequest,
}

#[derive(Debug, clap::Args)]
pub(crate) struct OrderRequest {
    /// Trading pair (e.g. XBTUSD).
    pair: String,
    /// Order volume.
    volume: String,
    /// Order type (market, limit, iceberg, stop-loss, take-profit, stop-loss-limit,
    /// take-profit-limit, trailing-stop, trailing-stop-limit, settle-position).
    #[arg(long, default_value = "limit")]
    r#type: String,
    /// Price (required for non-market orders).
    #[arg(long)]
    price: Option<String>,
    /// Limit price for stop-loss-limit, take-profit-limit, and trailing-stop-limit orders.
    #[arg(long)]
    price2: Option<String>,
    /// Display volume for iceberg orders (visible portion in the order book).
    #[arg(long)]
    displayvol: Option<String>,
    /// Price signal for triggered orders (last or index). Applies to stop-loss,
    /// stop-loss-limit, take-profit, take-profit-limit, trailing-stop, trailing-stop-limit.
    #[arg(long)]
    trigger: Option<String>,
    /// Leverage.
    #[arg(long)]
    leverage: Option<String>,
    /// Reduce-only flag.
    #[arg(long)]
    reduce_only: bool,
    /// Time-in-force (GTC, IOC, GTD).
    #[arg(long)]
    timeinforce: Option<String>,
    /// Start time.
    #[arg(long)]
    start_time: Option<String>,
    /// Expire time.
    #[arg(long)]
    expire_time: Option<String>,
    /// User reference ID (signed 32-bit, for tagging groups of orders).
    #[arg(long)]
    userref: Option<i32>,
    /// Client order ID (mutually exclusive with userref).
    #[arg(long)]
    cl_ord_id: Option<String>,
    /// Order flags (comma-delimited: post, fcib, fciq, nompp, viqc).
    #[arg(long)]
    oflags: Option<String>,
    /// Self-trade prevention type (cancel-newest, cancel-oldest, cancel-both).
    #[arg(long)]
    stptype: Option<String>,
    /// Conditional close order type (limit, stop-loss, take-profit, stop-loss-limit,
    /// take-profit-limit).
    #[arg(long)]
    close_ordertype: Option<String>,
    /// Conditional close order price.
    #[arg(long)]
    close_price: Option<String>,
    /// Conditional close order secondary price.
    #[arg(long)]
    close_price2: Option<String>,
    /// RFC3339 deadline for the order to reach the matching engine.
    #[arg(long)]
    deadline: Option<String>,
    /// Validate only (do not submit).
    #[arg(long)]
    validate: bool,
    /// Asset class (required for non-crypto pairs, e.g. tokenized_asset for xstocks).
    #[arg(long)]
    asset_class: Option<String>,
    /// Rationale to record in the decision log (workspace-scoped inside a
    /// workspace; requires --session against the real account).
    #[arg(long)]
    reason: Option<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Batch {
    /// Path to JSON file containing order array.
    json_file: PathBuf,
    /// Asset pair (overrides pair from JSON; required if not in JSON).
    #[arg(long)]
    pair: Option<String>,
    /// Asset class (required for non-crypto pairs, e.g. tokenized_asset for xstocks).
    #[arg(long)]
    asset_class: Option<String>,
    /// RFC3339 deadline for the batch to reach the matching engine.
    #[arg(long)]
    deadline: Option<String>,
    /// Validate only (do not submit).
    #[arg(long)]
    validate: bool,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Amend {
    /// Kraken transaction ID of the order to amend (either txid or --cl-ord-id required).
    #[arg(long)]
    txid: Option<String>,
    /// Client order ID of the order to amend (either --txid or --cl-ord-id required).
    #[arg(long)]
    cl_ord_id: Option<String>,
    /// New order quantity in base asset.
    #[arg(long)]
    order_qty: Option<String>,
    /// New display quantity for iceberg orders (min 1/15 of remaining quantity).
    #[arg(long)]
    display_qty: Option<String>,
    /// New limit price (supports +/- relative and % suffix).
    #[arg(long)]
    limit_price: Option<String>,
    /// New trigger price for triggered order types (supports +/- relative and % suffix).
    #[arg(long)]
    trigger_price: Option<String>,
    /// Trading pair (required for non-crypto pairs, e.g. xstocks).
    #[arg(long)]
    pair: Option<String>,
    /// Reject amend if limit price change would cause immediate match.
    #[arg(long)]
    post_only: bool,
    /// RFC3339 deadline (min now()+2s, max now()+60s).
    #[arg(long)]
    deadline: Option<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Edit {
    /// Transaction ID of the order to edit.
    txid: String,
    /// New volume.
    #[arg(long)]
    volume: Option<String>,
    /// New price.
    #[arg(long)]
    price: Option<String>,
    /// Trading pair.
    #[arg(long)]
    pair: Option<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Cancel {
    /// Transaction ID(s) or userref (comma-separated for multiple).
    #[arg(num_args = 1..)]
    txids: Vec<String>,
    /// Cancel by client order ID instead of txid.
    #[arg(long)]
    cl_ord_id: Option<String>,
    /// Rationale to record in the decision log (workspace-scoped inside a
    /// workspace; beside the journal against the real account).
    #[arg(long)]
    reason: Option<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct CancelBatch {
    /// Transaction IDs or user references (up to 50).
    #[arg(num_args = 1..)]
    txids: Vec<String>,
    /// Client order IDs to cancel (up to 50 total combined with txids).
    #[arg(long, num_args = 1..)]
    cl_ord_ids: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct CancelAll {}

#[derive(Debug, clap::Args)]
pub(crate) struct CancelAfter {
    /// Timeout in seconds (0 to disable).
    timeout: u64,
}

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
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            json_file,
            pair,
            asset_class,
            deadline,
            validate,
        } = self;
        let contents = std::fs::read_to_string(json_file)?;
        let orders: Value = serde_json::from_str(&contents)?;
        let arr = validate_batch_orders(&orders)?;

        let resolved_pair = if let Some(p) = pair {
            Some(p)
        } else {
            arr.first()
                .and_then(|o| o.get("pair"))
                .and_then(|p| p.as_str())
                .map(|s| s.to_string())
        };

        let mut body = serde_json::Map::new();
        body.insert("orders".into(), orders);
        if let Some(p) = resolved_pair {
            body.insert("pair".into(), Value::String(p));
        }
        if let Some(ac) = asset_class {
            body.insert("asset_class".into(), Value::String(ac));
        }
        if let Some(dl) = deadline {
            body.insert("deadline".into(), Value::String(dl));
        }
        if validate {
            body.insert("validate".into(), Value::Bool(true));
        }
        let data = client
            .private_post_json("AddOrderBatch", body, creds, otp, false)
            .await?;
        Ok(parse_order_result(&data))
    }
}

impl Execute for Amend {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
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
        validate_amend_ids(&txid, &cl_ord_id)?;
        let mut params = HashMap::new();
        if let Some(id) = txid {
            params.insert("txid".into(), id);
        }
        if let Some(id) = cl_ord_id {
            params.insert("cl_ord_id".into(), id);
        }
        if let Some(qty) = order_qty {
            params.insert("order_qty".into(), qty);
        }
        if let Some(dq) = display_qty {
            params.insert("display_qty".into(), dq);
        }
        if let Some(lp) = limit_price {
            params.insert("limit_price".into(), lp);
        }
        if let Some(tp) = trigger_price {
            params.insert("trigger_price".into(), tp);
        }
        if let Some(p) = pair {
            params.insert("pair".into(), p);
        }
        if post_only {
            params.insert("post_only".into(), "true".into());
        }
        if let Some(dl) = deadline {
            params.insert("deadline".into(), dl);
        }
        let data = client
            .private_post("AmendOrder", params, creds, otp, false)
            .await?;
        Ok(parse_order_result(&data))
    }
}

impl Execute for Edit {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            txid,
            volume,
            price,
            pair,
        } = self;
        let mut params = HashMap::new();
        params.insert("txid".into(), txid);
        if let Some(v) = volume {
            params.insert("volume".into(), v);
        }
        if let Some(p) = price {
            params.insert("price".into(), p);
        }
        if let Some(pair) = pair {
            params.insert("pair".into(), pair);
        }
        let data = client
            .private_post("EditOrder", params, creds, otp, false)
            .await?;
        Ok(parse_order_result(&data))
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
                Err(kraken_workspace::WorkspaceError::LiveUnavailable {
                    name: manifest.name,
                }
                .into())
            }
            workspace_guard::ActiveMode::Master => {
                master_cancel(txids, cl_ord_id, reason.as_deref(), ctx).await
            }
        }
    }
}

async fn master_cancel(
    txids: Vec<String>,
    cl_ord_id: Option<String>,
    reason: Option<&str>,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    eprintln!("{LIVE_BANNER}");
    // Open the mirror first so local lock failure cannot follow execution.
    // A cl_ord_id cancel has no txid to mirror; reconciliation closes that gap.
    let mut mirror = super::journal_mirror::open_attached(ctx).await?;
    let (client, creds, otp) = ctx.spot_authed()?;
    let mut params = HashMap::new();
    if !txids.is_empty() {
        params.insert("txid".into(), txids.join(","));
    }
    if let Some(id) = cl_ord_id {
        params.insert("cl_ord_id".into(), id);
    }
    let data = client
        .private_post("CancelOrder", params, creds, otp, false)
        .await?;
    for txid in &txids {
        super::journal_mirror::mirror_cancelled(&mut mirror, txid);
        if let Some(reason) = reason {
            // Best-effort like the order path: the cancel already reached the
            // venue. A master cancel names a txid, not a pair, so symbol is None.
            if let Err(err) = super::paper::log_routed_cancel_decision(txid, reason) {
                tracing::warn!(txid, %err, "order cancelled but decision-log append failed");
            }
        }
    }
    Ok(parse_order_result(&data))
}

impl Execute for CancelBatch {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self { txids, cl_ord_ids } = self;
        validate_cancel_batch(&txids, &cl_ord_ids)?;
        let mut body = serde_json::Map::new();
        let all_ids: Vec<Value> = txids
            .into_iter()
            .chain(cl_ord_ids)
            .map(Value::String)
            .collect();
        body.insert("orders".into(), Value::Array(all_ids));
        let data = client
            .private_post_json("CancelOrderBatch", body, creds, otp, true)
            .await?;
        Ok(parse_order_result(&data))
    }
}

impl Execute for CancelAll {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        match workspace_guard::active_mode(ctx)? {
            workspace_guard::ActiveMode::Paper(manifest) => {
                super::paper::cancel_all_on_workspace(ctx, &manifest).await
            }
            workspace_guard::ActiveMode::Live(manifest) => {
                Err(kraken_workspace::WorkspaceError::LiveUnavailable {
                    name: manifest.name,
                }
                .into())
            }
            workspace_guard::ActiveMode::Master => master_cancel_all(ctx).await,
        }
    }
}

async fn master_cancel_all(ctx: &AppContext) -> Result<CommandOutput> {
    eprintln!("{LIVE_BANNER}");
    // Confirm and lock the mirror before execution can succeed.
    // The venue reports only a count, so the mirror closes
    // every order it knows — anything it never saw is reconciliation's.
    ctx.confirm_destructive("Cancel ALL open orders?")?;
    let mut mirror = super::journal_mirror::open_attached(ctx).await?;
    let (client, creds, otp) = ctx.spot_authed()?;
    let data = client
        .private_post("CancelAll", HashMap::new(), creds, otp, false)
        .await?;
    let known: Vec<String> = mirror
        .state()
        .ok()
        .map(|state| state.open_orders.iter().map(|o| o.id.clone()).collect())
        .unwrap_or_default();
    for txid in &known {
        super::journal_mirror::mirror_cancelled(&mut mirror, txid);
    }
    Ok(parse_order_result(&data))
}

impl Execute for CancelAfter {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self { timeout } = self;
        let mut params = HashMap::new();
        params.insert("timeout".into(), timeout.to_string());
        let data = client
            .private_post("CancelAllOrdersAfter", params, creds, otp, false)
            .await?;
        Ok(parse_order_result(&data))
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
            Err(kraken_workspace::WorkspaceError::LiveUnavailable {
                name: manifest.name,
            }
            .into())
        }
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
                pair: Some(order.pair.to_uppercase()),
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
    master_order_at(
        super::journal_mirror::master_journal_path()?,
        side,
        order,
        ctx,
    )
    .await
}

/// [`master_order`] with a caller-chosen mirror journal — the unit-test seam.
async fn master_order_at(
    journal: std::path::PathBuf,
    side: OrderSide,
    order: OrderRequest,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    use kraken_workspace::receipt::{OrderReceipt, ReceiptCore, ReceiptStatus, TradeMode};
    // Open the mirror before the venue call: a busy journal
    // refuses before money moves, and its writer lock serializes concurrent
    // trades. A validate-only request places nothing, so no mirror.
    let mut mirror = if order.validate {
        None
    } else {
        Some(super::journal_mirror::open_attached_at(journal, ctx).await?)
    };
    let facts = (
        order.pair.clone(),
        order.volume.clone(),
        order.r#type.clone(),
        order.price.clone(),
        order.validate,
        order.reason.clone(),
    );
    let (client, creds, otp) = ctx.spot_authed()?;
    let params = build_order_params(side, order)?;
    let data = client
        .private_post("AddOrder", params, creds, otp, false)
        .await?;

    let (pair, volume, order_type, price, validated, reason) = facts;
    let txid = data
        .get("txid")
        .and_then(|t| t.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .map(str::to_string);
    if let (Some(mirror), Some(txid)) = (mirror.as_mut(), txid.as_deref()) {
        let paper_type = if order_type.eq_ignore_ascii_case("market") {
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
            price.as_deref().and_then(|p| p.parse().ok()),
        );
        if let Some(reason) = reason.as_deref() {
            // Best-effort like the paper path: the order is already real.
            if let Err(err) = super::paper::log_routed_decision(side, &pair, txid, reason) {
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
            pair: Some(pair.to_uppercase()),
            side: Some(side),
            order_type: Some(order_type.to_lowercase()),
            volume: volume.parse().ok(),
            price: price.as_deref().and_then(|p| p.parse().ok()),
        },
        venue: Some(data),
        paper: None,
    })
}

fn receipt_output(receipt: kraken_workspace::receipt::OrderReceipt) -> Result<CommandOutput> {
    let data = serde_json::to_value(&receipt)
        .map_err(|e| KrakenError::Parse(format!("receipt serialization failed: {e}")))?;
    Ok(CommandOutput::from_untyped(&data))
}

fn build_order_params(
    direction: OrderSide,
    order: OrderRequest,
) -> Result<HashMap<String, String>> {
    let OrderRequest {
        pair,
        volume,
        r#type,
        price,
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
    let order_type = r#type.as_str();
    let valid_types = [
        "market",
        "limit",
        "iceberg",
        "stop-loss",
        "take-profit",
        "stop-loss-limit",
        "take-profit-limit",
        "trailing-stop",
        "trailing-stop-limit",
        "settle-position",
    ];
    if !valid_types.contains(&order_type) {
        return Err(KrakenError::Validation(format!(
            "Invalid order type: {order_type}. Valid: {}",
            valid_types.join(", ")
        )));
    }

    if order_type != "market" && price.is_none() {
        return Err(KrakenError::Validation(format!(
            "Price is required for {order_type} orders"
        )));
    }

    if order_type == "iceberg" && displayvol.is_none() {
        return Err(KrakenError::Validation(
            "displayvol is required for iceberg orders".into(),
        ));
    }

    let price2_types = [
        "stop-loss-limit",
        "take-profit-limit",
        "trailing-stop-limit",
    ];
    if price2.is_some() && !price2_types.contains(&order_type) {
        return Err(KrakenError::Validation(format!(
            "price2 is only valid for {} orders",
            price2_types.join(", ")
        )));
    }

    let trigger_types = [
        "stop-loss",
        "stop-loss-limit",
        "take-profit",
        "take-profit-limit",
        "trailing-stop",
        "trailing-stop-limit",
    ];
    if let Some(t) = trigger.as_deref() {
        if !trigger_types.contains(&order_type) {
            return Err(KrakenError::Validation(format!(
                "trigger is only valid for {} orders",
                trigger_types.join(", ")
            )));
        }
        if !["last", "index"].contains(&t) {
            return Err(KrakenError::Validation(format!(
                "Invalid trigger value: {t}. Valid: last, index"
            )));
        }
    }

    if userref.is_some() && cl_ord_id.is_some() {
        return Err(KrakenError::Validation(
            "userref and cl_ord_id are mutually exclusive".into(),
        ));
    }

    if (close_price.is_some() || close_price2.is_some()) && close_ordertype.is_none() {
        return Err(KrakenError::Validation(
            "close_ordertype is required when close_price or close_price2 is set".into(),
        ));
    }

    let mut params = HashMap::new();
    params.insert("pair".into(), pair);
    params.insert("type".into(), direction.to_string());
    params.insert("ordertype".into(), r#type);
    params.insert("volume".into(), volume);

    if let Some(p) = price {
        params.insert("price".into(), p);
    }
    if let Some(p2) = price2 {
        params.insert("price2".into(), p2);
    }
    if let Some(dv) = displayvol {
        params.insert("displayvol".into(), dv);
    }
    if let Some(t) = trigger {
        params.insert("trigger".into(), t);
    }
    if let Some(lev) = leverage {
        params.insert("leverage".into(), lev);
    }
    if reduce_only {
        params.insert("reduce_only".into(), "true".into());
    }
    if let Some(tif) = timeinforce {
        params.insert("timeinforce".into(), tif);
    }
    if let Some(st) = start_time {
        params.insert("starttm".into(), st);
    }
    if let Some(et) = expire_time {
        params.insert("expiretm".into(), et);
    }
    if let Some(ur) = userref {
        params.insert("userref".into(), ur.to_string());
    }
    if let Some(id) = cl_ord_id {
        params.insert("cl_ord_id".into(), id);
    }
    if let Some(of) = oflags {
        params.insert("oflags".into(), of);
    }
    if let Some(stp) = stptype {
        params.insert("stptype".into(), stp);
    }
    if let Some(cot) = close_ordertype {
        params.insert("close[ordertype]".into(), cot);
    }
    if let Some(cp) = close_price {
        params.insert("close[price]".into(), cp);
    }
    if let Some(cp2) = close_price2 {
        params.insert("close[price2]".into(), cp2);
    }
    if let Some(dl) = deadline {
        params.insert("deadline".into(), dl);
    }
    if validate {
        params.insert("validate".into(), "true".into());
    }
    if let Some(ac) = asset_class {
        params.insert("asset_class".into(), ac);
    }

    Ok(params)
}

fn validate_amend_ids(txid: &Option<String>, cl_ord_id: &Option<String>) -> Result<()> {
    if txid.is_none() && cl_ord_id.is_none() {
        return Err(KrakenError::Validation(
            "Either --txid or --cl-ord-id is required for amend".into(),
        ));
    }
    if txid.is_some() && cl_ord_id.is_some() {
        return Err(KrakenError::Validation(
            "Only one of --txid or --cl-ord-id should be provided".into(),
        ));
    }
    Ok(())
}

fn validate_batch_orders(orders: &Value) -> Result<&Vec<Value>> {
    let arr = orders
        .as_array()
        .ok_or_else(|| KrakenError::Validation("Batch file must contain a JSON array".into()))?;
    if arr.len() < 2 || arr.len() > 15 {
        return Err(KrakenError::Validation(
            "Batch must contain 2-15 orders".into(),
        ));
    }
    Ok(arr)
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

fn parse_order_result(data: &Value) -> CommandOutput {
    let pairs: Vec<(String, String)> = if let Some(obj) = data.as_object() {
        obj.iter()
            .map(|(k, v)| {
                let val = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (k.clone(), val)
            })
            .collect()
    } else {
        vec![("Result".into(), data.to_string())]
    };
    CommandOutput::key_value(pairs, data.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The banner is agent-facing copy quoted by skills — byte-stable.
    #[test]
    fn live_banner_copy_is_pinned() {
        assert_eq!(LIVE_BANNER, "live: this goes to the real Kraken account");
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

    fn expect_err(a: OrderRequest, substr: &str) {
        let err = build_order_params(OrderSide::Buy, a)
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
        let params = build_order_params(OrderSide::Buy, a).unwrap();
        assert_eq!(params["ordertype"], "limit");
        assert_eq!(params["price"], "50000");
    }

    #[test]
    fn valid_market_order_no_price() {
        let mut a = order();
        a.r#type = "market".into();
        a.price = None;
        build_order_params(OrderSide::Buy, a).unwrap();
    }

    #[test]
    fn invalid_order_type() {
        let mut a = order();
        a.r#type = "bogus".into();
        expect_err(a, "Invalid order type");
    }

    #[test]
    fn price_required_for_limit() {
        let mut a = order();
        a.price = None;
        expect_err(a, "Price is required");
    }

    #[test]
    fn iceberg_requires_displayvol() {
        let mut a = order();
        a.r#type = "iceberg".into();
        expect_err(a, "displayvol is required for iceberg");
    }

    #[test]
    fn iceberg_with_displayvol_ok() {
        let mut a = order();
        a.r#type = "iceberg".into();
        a.displayvol = Some("0.001".into());
        let params = build_order_params(OrderSide::Buy, a).unwrap();
        assert_eq!(params["displayvol"], "0.001");
    }

    #[test]
    fn price2_rejected_on_limit_order() {
        let mut a = order();
        a.price2 = Some("60000".into());
        expect_err(a, "price2 is only valid for");
    }

    #[test]
    fn price2_accepted_on_stop_loss_limit() {
        let mut a = order();
        a.r#type = "stop-loss-limit".into();
        a.price2 = Some("49000".into());
        let params = build_order_params(OrderSide::Buy, a).unwrap();
        assert_eq!(params["price2"], "49000");
    }

    #[test]
    fn price2_accepted_on_take_profit_limit() {
        let mut a = order();
        a.r#type = "take-profit-limit".into();
        a.price2 = Some("55000".into());
        build_order_params(OrderSide::Buy, a).unwrap();
    }

    #[test]
    fn price2_accepted_on_trailing_stop_limit() {
        let mut a = order();
        a.r#type = "trailing-stop-limit".into();
        a.price2 = Some("100".into());
        build_order_params(OrderSide::Buy, a).unwrap();
    }

    #[test]
    fn trigger_rejected_on_limit_order() {
        let mut a = order();
        a.trigger = Some("last".into());
        expect_err(a, "trigger is only valid for");
    }

    #[test]
    fn trigger_accepted_on_stop_loss() {
        let mut a = order();
        a.r#type = "stop-loss".into();
        a.trigger = Some("last".into());
        let params = build_order_params(OrderSide::Buy, a).unwrap();
        assert_eq!(params["trigger"], "last");
    }

    #[test]
    fn trigger_accepted_on_trailing_stop() {
        let mut a = order();
        a.r#type = "trailing-stop".into();
        a.trigger = Some("index".into());
        build_order_params(OrderSide::Buy, a).unwrap();
    }

    #[test]
    fn trigger_invalid_value() {
        let mut a = order();
        a.r#type = "stop-loss".into();
        a.trigger = Some("las".into());
        expect_err(a, "Invalid trigger value");
    }

    #[test]
    fn trigger_value_last_ok() {
        let mut a = order();
        a.r#type = "stop-loss".into();
        a.trigger = Some("last".into());
        build_order_params(OrderSide::Buy, a).unwrap();
    }

    #[test]
    fn trigger_value_index_ok() {
        let mut a = order();
        a.r#type = "take-profit".into();
        a.trigger = Some("index".into());
        build_order_params(OrderSide::Buy, a).unwrap();
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
        let params = build_order_params(OrderSide::Buy, a).unwrap();
        assert_eq!(params["userref"], "12345");
        assert!(!params.contains_key("cl_ord_id"));
    }

    #[test]
    fn cl_ord_id_alone_ok() {
        let mut a = order();
        a.cl_ord_id = Some("my-order".into());
        let params = build_order_params(OrderSide::Buy, a).unwrap();
        assert_eq!(params["cl_ord_id"], "my-order");
        assert!(!params.contains_key("userref"));
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
        let params = build_order_params(OrderSide::Buy, a).unwrap();
        assert_eq!(params["close[ordertype]"], "limit");
        assert_eq!(params["close[price]"], "55000");
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
        let params = build_order_params(OrderSide::Buy, a).unwrap();
        assert_eq!(params["leverage"], "2");
        assert_eq!(params["reduce_only"], "true");
        assert_eq!(params["timeinforce"], "GTC");
        assert_eq!(params["starttm"], "0");
        assert_eq!(params["expiretm"], "+3600");
        assert_eq!(params["oflags"], "post,fcib");
        assert_eq!(params["stptype"], "cancel-newest");
        assert_eq!(params["deadline"], "2026-03-01T18:00:00Z");
        assert_eq!(params["validate"], "true");
        assert_eq!(params["cl_ord_id"], "test-123");
        assert_eq!(params["asset_class"], "tokenized_asset");
    }

    #[test]
    fn amend_requires_at_least_one_id() {
        let err = validate_amend_ids(&None, &None).unwrap_err().to_string();
        assert!(err.contains("Either --txid or --cl-ord-id is required"));
    }

    #[test]
    fn amend_rejects_both_ids() {
        let err = validate_amend_ids(&Some("OXXXXX".into()), &Some("my-id".into()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("Only one of --txid or --cl-ord-id"));
    }

    #[test]
    fn amend_txid_only_ok() {
        validate_amend_ids(&Some("OXXXXX".into()), &None).unwrap();
    }

    #[test]
    fn amend_cl_ord_id_only_ok() {
        validate_amend_ids(&None, &Some("my-id".into())).unwrap();
    }

    #[test]
    fn batch_rejects_non_array() {
        let val: Value = serde_json::json!({"not": "an array"});
        let err = validate_batch_orders(&val).unwrap_err().to_string();
        assert!(err.contains("JSON array"));
    }

    #[test]
    fn batch_rejects_single_order() {
        let val: Value = serde_json::json!([{"pair": "BTCUSD"}]);
        let err = validate_batch_orders(&val).unwrap_err().to_string();
        assert!(err.contains("2-15 orders"));
    }

    #[test]
    fn batch_rejects_sixteen_orders() {
        let orders: Vec<Value> = (0..16).map(|i| serde_json::json!({"id": i})).collect();
        let val = Value::Array(orders);
        let err = validate_batch_orders(&val).unwrap_err().to_string();
        assert!(err.contains("2-15 orders"));
    }

    #[test]
    fn batch_accepts_two_orders() {
        let val: Value = serde_json::json!([{"pair":"BTCUSD"},{"pair":"BTCUSD"}]);
        let arr = validate_batch_orders(&val).unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn batch_accepts_fifteen_orders() {
        let orders: Vec<Value> = (0..15).map(|i| serde_json::json!({"id": i})).collect();
        let val = Value::Array(orders);
        let arr = validate_batch_orders(&val).unwrap();
        assert_eq!(arr.len(), 15);
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

    #[test]
    fn cancel_batch_merges_into_flat_string_array() {
        let txids: Vec<String> = vec!["OABC-12345".into(), "ODEF-67890".into()];
        let cl_ids: Vec<String> = vec!["my-order-1".into()];
        let all: Vec<Value> = txids
            .iter()
            .chain(cl_ids.iter())
            .map(|id| Value::String(id.clone()))
            .collect();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], serde_json::json!("OABC-12345"));
        assert_eq!(all[1], serde_json::json!("ODEF-67890"));
        assert_eq!(all[2], serde_json::json!("my-order-1"));
    }

    #[test]
    fn cancel_batch_serializes_as_flat_array() {
        let txids: Vec<String> = vec!["TX1".into()];
        let cl_ids: Vec<String> = vec!["CL1".into()];
        let all: Vec<Value> = txids
            .iter()
            .chain(cl_ids.iter())
            .map(|id| Value::String(id.clone()))
            .collect();
        let json_str = serde_json::to_string(&all).unwrap();
        assert_eq!(json_str, r#"["TX1","CL1"]"#);
    }

    /// Pins venue truth flowing through [`master_order_at`] into the
    /// receipt envelope and the mirror journal against a mock venue.
    mod mirror_wiring {
        use base64::Engine as _;
        use kraken_core::OrderSide;
        use serde_json::json;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        use super::super::master_order_at;
        use super::order;
        use crate::cli::AppContext;
        use crate::config::{CredentialSource, Credentials};

        fn venue_ctx(api_url: String) -> AppContext {
            AppContext {
                format: crate::output::OutputFormat::Json,
                api_url: Some(api_url),
                futures_url: None,
                ws_public_url: None,
                ws_auth_url: None,
                ws_l3_url: None,
                ws_futures_url: None,
                ws_reconnect_base_ms: None,
                spot_credentials: Some(Credentials {
                    api_key: secrecy::SecretString::from("test-api-key"),
                    api_secret: secrecy::SecretString::from(
                        base64::engine::general_purpose::STANDARD
                            .encode(b"test_secret_key_bytes_32_chars!!"),
                    ),
                    source: CredentialSource::Flag,
                }),
                futures_credentials: None,
                api_secret: None,
                otp: None,
                workspace: None,
                force: true,
                accept_invalid_certs: false,
                allow_any_url_host: true,
                mcp_mode: false,
                monitor: None,
                spot_client: std::sync::OnceLock::new(),
                futures_client: std::sync::OnceLock::new(),
            }
        }

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
            let ctx = venue_ctx(server.uri());
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
            // The venue owns reservations; the mirror is honest about it.
            assert_eq!(submitted["order"]["reserved_amount"], "0");
        }
    }
}
