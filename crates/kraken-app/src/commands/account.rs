//! Private account data commands.

use super::{Execute, rest_bridge};
use crate::{
    cli::AppContext,
    errors::Result,
    output::{CommandOutput, json_field, published_json},
};
use kraken_contract::account::{
    AccountCommand, Balance, ClosedOrders, CreditLines, ExportDelete, ExportReport, ExportRetrieve,
    ExportStatus, ExtendedBalance, Ledgers, OpenOrders, OrderbookL3, Positions, QueryLedgers,
    QueryOrders, QueryTrades, TradeBalance, TradesHistory, Volume,
};
use kraken_paper::FillSource;
use kraken_rest::{Pair, spot::account as rest};
use serde_json::Value;
use std::path::PathBuf;

/// Free function: `OpenOrders` is `kraken_contract`'s type, so args cannot be
/// inherent. Venue-only filters have no paper meaning; refuse rather than
/// ignore.
fn ensure_open_orders_plain(
    args: &OpenOrders,
    manifest: &kraken_workspace::WorkspaceManifest,
) -> Result<()> {
    let set: &[(&str, bool)] = &[
        ("open-orders --trades", args.trades),
        ("open-orders --userref", args.userref.is_some()),
        ("open-orders --cl-ord-id", args.cl_ord_id.is_some()),
        (
            "open-orders --rebase-multiplier",
            args.rebase.rebase_multiplier.is_some(),
        ),
    ];
    ensure_no_venue_only_flags(set, manifest)
}

/// Free function: `TradesHistory` is `kraken_contract`'s type, so args cannot
/// be inherent. Venue-only filters have no paper meaning; refuse rather than
/// ignore.
fn ensure_trades_history_plain(
    args: &TradesHistory,
    manifest: &kraken_workspace::WorkspaceManifest,
) -> Result<()> {
    let set: &[(&str, bool)] = &[
        ("trades-history --type", args.trade_type.is_some()),
        ("trades-history --trades", args.trades),
        ("trades-history --consolidate-taker", args.consolidate_taker),
        ("trades-history --ledgers", args.ledgers),
        ("trades-history --start", args.page.is_bounded()),
        (
            "trades-history --rebase-multiplier",
            args.rebase.rebase_multiplier.is_some(),
        ),
    ];
    ensure_no_venue_only_flags(set, manifest)
}

fn ensure_no_venue_only_flags(
    set: &[(&str, bool)],
    manifest: &kraken_workspace::WorkspaceManifest,
) -> Result<()> {
    if let Some((flag, _)) = set.iter().find(|(_, on)| *on) {
        return Err(
            kraken_workspace::policy::unsupported(flag, &manifest.name, manifest.mode).into(),
        );
    }
    Ok(())
}

impl Execute for Balance {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        match super::workspace_guard::active_mode(ctx)? {
            super::workspace_guard::ActiveMode::Paper(manifest)
            | super::workspace_guard::ActiveMode::Live(manifest) => {
                kraken_contract::paper::Balance {
                    name: Some(manifest.name),
                }
                .execute(ctx)
                .await
            }
            super::workspace_guard::ActiveMode::Master => venue_balance(self, ctx).await,
        }
    }
}

/// Free function: `Balance` is `kraken_contract`'s type, so args cannot be
/// inherent. The unrouted venue request — also the `workspace balance` default
/// target, which is the master account by definition.
pub(crate) async fn venue_balance(args: Balance, ctx: &AppContext) -> Result<CommandOutput> {
    let Balance { rebase } = args;
    let query = rest::BalanceQuery {
        rebase_multiplier: rest_bridge::rebase(&rebase),
    };
    let data = published_json(&ctx.spot_account()?.sign(&query).await?)?;
    let (headers, rows) = parse_balance_table(&data);
    Ok(CommandOutput::new(data, headers, rows))
}

impl Execute for ExtendedBalance {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let query = rest::ExtendedBalanceQuery::default();
        let data = published_json(&ctx.spot_account()?.sign(&query).await?)?;
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

impl Execute for OrderbookL3 {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { pair, depth } = self;
        let query = rest::Level3Query {
            pair: Pair::from(pair),
            depth: Some(rest_bridge::l3_depth(depth)),
        };
        let data = published_json(&ctx.spot_account()?.sign(&query).await?)?;
        Ok(CommandOutput::new(
            data,
            vec!["Data".into()],
            vec![vec!["L3 orderbook data returned (see JSON output)".into()]],
        ))
    }
}

impl Execute for CreditLines {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { rebase } = self;
        let query = rest::CreditLinesQuery {
            rebase_multiplier: rest_bridge::rebase(&rebase),
        };
        let data = published_json(&ctx.spot_account()?.sign(&query).await?)?;
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

impl Execute for TradeBalance {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { asset, rebase } = self;
        let query = rest::TradeBalanceQuery {
            asset,
            rebase_multiplier: rest_bridge::rebase(&rebase),
        };
        let data = published_json(&ctx.spot_account()?.sign(&query).await?)?;
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

impl Execute for OpenOrders {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        match super::workspace_guard::active_mode(ctx)? {
            super::workspace_guard::ActiveMode::Paper(manifest) => {
                ensure_open_orders_plain(&self, &manifest)?;
                let mut out =
                    super::paper::orders_on_workspace(ctx, &manifest, FillSource::Simulated)
                        .await?;
                out.stamp_workspace(&manifest.name);
                Ok(out)
            }
            super::workspace_guard::ActiveMode::Live(manifest) => {
                ensure_open_orders_plain(&self, &manifest)?;
                super::workspace_guard::warn_live_workspace(&manifest);
                // Reconcile first: the read must see the venue's verdict on
                // args workspace's orders, not the CLI's last local guess.
                let sync = super::journal_mirror::sync_live_workspace(ctx, &manifest).await?;
                let mut out =
                    super::paper::orders_on_workspace(ctx, &manifest, FillSource::Venue).await?;
                sync.stamp(&mut out, &manifest.name);
                Ok(out)
            }
            super::workspace_guard::ActiveMode::Master => venue_open_orders(self, ctx).await,
        }
    }
}

/// Free function: `OpenOrders` is `kraken_contract`'s type, so args cannot be
/// inherent.
async fn venue_open_orders(args: OpenOrders, ctx: &AppContext) -> Result<CommandOutput> {
    let OpenOrders {
        trades,
        userref,
        cl_ord_id,
        rebase,
    } = args;
    let query = rest::OpenOrdersQuery {
        trades,
        userref,
        cl_ord_id,
        rebase_multiplier: rest_bridge::rebase(&rebase),
    };
    let data = published_json(&ctx.spot_account()?.sign(&query).await?)?;
    let (headers, rows) = parse_orders_table(&data);
    Ok(CommandOutput::new(data, headers, rows))
}

impl Execute for ClosedOrders {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            trades,
            page,
            userref,
            cl_ord_id,
            closetime,
            consolidate_taker,
            rebase,
        } = self;
        let query = rest::ClosedOrdersQuery {
            trades,
            userref,
            cl_ord_id,
            window: rest_bridge::window(page),
            closetime: closetime.map(rest_bridge::close_time),
            consolidate_taker,
            rebase_multiplier: rest_bridge::rebase(&rebase),
        };
        let data = published_json(&ctx.spot_account()?.sign(&query).await?)?;
        let (headers, rows) = parse_orders_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

impl Execute for QueryOrders {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            txids,
            trades,
            userref,
            consolidate_taker,
            rebase,
        } = self;
        let query = rest::QueryOrdersQuery {
            txid: txids.into_iter().map(Into::into).collect(),
            trades,
            userref,
            consolidate_taker,
            rebase_multiplier: rest_bridge::rebase(&rebase),
        };
        let data = published_json(&ctx.spot_account()?.sign(&query).await?)?;
        let (headers, rows) = parse_orders_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

impl Execute for TradesHistory {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        match super::workspace_guard::active_mode(ctx)? {
            super::workspace_guard::ActiveMode::Paper(manifest) => {
                ensure_trades_history_plain(&self, &manifest)?;
                let mut out =
                    super::paper::history_on_workspace(ctx, &manifest, FillSource::Simulated)
                        .await?;
                out.stamp_workspace(&manifest.name);
                Ok(out)
            }
            super::workspace_guard::ActiveMode::Live(manifest) => {
                ensure_trades_history_plain(&self, &manifest)?;
                super::workspace_guard::warn_live_workspace(&manifest);
                // Reconcile first: these are the venue's own fills, folded in
                // by txid, so the history is real rather than simulated.
                let sync = super::journal_mirror::sync_live_workspace(ctx, &manifest).await?;
                let mut out =
                    super::paper::history_on_workspace(ctx, &manifest, FillSource::Venue).await?;
                sync.stamp(&mut out, &manifest.name);
                Ok(out)
            }
            super::workspace_guard::ActiveMode::Master => venue_trades_history(self, ctx).await,
        }
    }
}

/// Free function: `TradesHistory` is `kraken_contract`'s type, so args cannot
/// be inherent.
async fn venue_trades_history(args: TradesHistory, ctx: &AppContext) -> Result<CommandOutput> {
    let TradesHistory {
        trade_type,
        trades,
        consolidate_taker,
        ledgers,
        page,
        rebase,
    } = args;
    let query = rest::TradesHistoryQuery {
        filter: trade_type.map(rest_bridge::trade_filter),
        trades,
        consolidate_taker,
        ledgers,
        window: rest_bridge::window(page),
        rebase_multiplier: rest_bridge::rebase(&rebase),
    };
    let data = published_json(&ctx.spot_account()?.sign(&query).await?)?;
    let (headers, rows) = parse_trades_history(&data);
    Ok(CommandOutput::new(data, headers, rows))
}

impl Execute for QueryTrades {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            txids,
            trades,
            rebase,
        } = self;
        let query = rest::QueryTradesQuery {
            txid: txids.into_iter().map(Into::into).collect(),
            trades,
            rebase_multiplier: rest_bridge::rebase(&rebase),
        };
        let data = published_json(&ctx.spot_account()?.sign(&query).await?)?;
        let (headers, rows) = parse_trades_history(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

impl Execute for Positions {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            txid,
            show_pnl,
            consolidation,
            rebase,
        } = self;
        let query = rest::OpenPositionsQuery {
            txid: txid.into_iter().map(Into::into).collect(),
            docalcs: show_pnl,
            consolidation,
            rebase_multiplier: rest_bridge::rebase(&rebase),
        };
        let data = published_json(&ctx.spot_account()?.sign(&query).await?)?;
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

impl Execute for Ledgers {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            asset,
            ledger_type,
            asset_class,
            page,
            rebase,
        } = self;
        let query = rest::LedgerQuery {
            asset,
            aclass: asset_class.map(rest_bridge::asset_class),
            entry_type: ledger_type,
            window: rest_bridge::window(page),
            rebase_multiplier: rest_bridge::rebase(&rebase),
        };
        let data = published_json(&ctx.spot_account()?.sign(&query).await?)?;
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

impl Execute for QueryLedgers {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            ids,
            trades,
            rebase,
        } = self;
        let query = rest::QueryLedgerQuery {
            id: ids.into_iter().map(Into::into).collect(),
            trades,
            rebase_multiplier: rest_bridge::rebase(&rebase),
        };
        let data = published_json(&ctx.spot_account()?.sign(&query).await?)?;
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

impl Execute for Volume {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { pair, rebase } = self;
        let query = rest::TradeVolumeQuery {
            pair: pair.into_iter().map(Pair::from).collect(),
            rebase_multiplier: rest_bridge::rebase(&rebase),
        };
        let data = published_json(&ctx.spot_account()?.sign(&query).await?)?;
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

impl Execute for ExportReport {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            report,
            description,
            format,
            fields,
            starttm,
            endtm,
        } = self;
        let request = rest::AddExport {
            report: rest_bridge::export_kind(report),
            description,
            format: rest_bridge::export_format(format),
            fields,
            starttm: starttm
                .as_deref()
                .map(|t| rest_bridge::unix_seconds("--starttm", t))
                .transpose()?,
            endtm: endtm
                .as_deref()
                .map(|t| rest_bridge::unix_seconds("--endtm", t))
                .transpose()?,
        };
        let data = published_json(&ctx.spot_account()?.sign(&request).await?)?;
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

impl Execute for ExportStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { report } = self;
        let query = rest::ExportStatusQuery {
            report: rest_bridge::export_kind(report),
        };
        let data = published_json(&ctx.spot_account()?.sign(&query).await?)?;
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

impl Execute for ExportRetrieve {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            report_id,
            output_file,
        } = self;
        let request = rest::RetrieveExport {
            id: report_id.clone(),
        };
        let bytes = ctx.spot_account()?.download(&request).await?;

        let dest =
            output_file.unwrap_or_else(|| PathBuf::from(format!("kraken_export_{report_id}.zip")));

        std::fs::write(&dest, &bytes)?;

        let meta = serde_json::json!({
            "report_id": report_id,
            "file": dest.display().to_string(),
            "size_bytes": bytes.len(),
        });
        Ok(CommandOutput::key_value(
            vec![
                ("Report ID".into(), report_id),
                ("File".into(), dest.display().to_string()),
                ("Size".into(), format!("{} bytes", bytes.len())),
            ],
            meta,
        ))
    }
}

impl Execute for ExportDelete {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { report_id } = self;
        let request = rest::RemoveExport {
            id: report_id,
            removal: rest::Removal::Delete,
        };
        let data = published_json(&ctx.spot_account()?.sign(&request).await?)?;
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

fn parse_balance_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec!["Asset".into(), "Balance".into()];
    let mut rows = Vec::new();
    if let Some(obj) = data.as_object() {
        for (key, val) in obj {
            let balance = match val {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            rows.push(vec![key.clone(), balance]);
        }
    }
    rows.sort_by(|a, b| a[0].cmp(&b[0]));
    (headers, rows)
}

fn parse_orders_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec![
        "TXID".into(),
        "Status".into(),
        "Side".into(),
        "Pair".into(),
        "Price".into(),
        "Volume".into(),
    ];
    let mut rows = Vec::new();

    let orders_obj = data
        .get("open")
        .or_else(|| data.get("closed"))
        .and_then(|v| v.as_object())
        .or_else(|| data.as_object());

    if let Some(obj) = orders_obj {
        for (txid, order) in obj {
            let descr = order.get("descr").unwrap_or(&Value::Null);
            rows.push(vec![
                txid.clone(),
                json_field(order, "status"),
                json_field(descr, "side"),
                json_field(descr, "pair"),
                json_field(descr, "price"),
                json_field(order, "volume"),
            ]);
        }
    }
    (headers, rows)
}

fn parse_trades_history(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec![
        "TXID".into(),
        "Pair".into(),
        "Side".into(),
        "Price".into(),
        "Volume".into(),
        "Cost".into(),
    ];
    let mut rows = Vec::new();

    let trades_obj = data
        .get("trades")
        .and_then(|v| v.as_object())
        .or_else(|| data.as_object());

    if let Some(obj) = trades_obj {
        for (txid, trade) in obj {
            rows.push(vec![
                txid.clone(),
                json_field(trade, "pair"),
                json_field(trade, "side"),
                json_field(trade, "price"),
                json_field(trade, "vol"),
                json_field(trade, "cost"),
            ]);
        }
    }
    (headers, rows)
}

fn parse_kv_pairs(data: &Value) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    if let Some(obj) = data.as_object() {
        for (k, v) in obj {
            let val = match v {
                Value::String(s) => s.clone(),
                Value::Null => "null".into(),
                other => other.to_string(),
            };
            pairs.push((k.clone(), val));
        }
    }
    pairs
}

delegate_execute!(AccountCommand {
    Balance,
    ExtendedBalance,
    OrderbookL3,
    CreditLines,
    TradeBalance,
    OpenOrders,
    ClosedOrders,
    QueryOrders,
    TradesHistory,
    QueryTrades,
    Positions,
    Ledgers,
    QueryLedgers,
    Volume,
    ExportReport,
    ExportStatus,
    ExportRetrieve,
    ExportDelete
});
