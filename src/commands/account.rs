//! Private account data commands.

use std::collections::HashMap;
use std::path::PathBuf;

use clap::Subcommand;
use enum_dispatch::enum_dispatch;
use serde_json::Value;

use super::Execute;
use super::schema;
use super::value_types::{
    AssetClass, CliWire, Closetime, L3Depth, Pagination, RebaseOpts, ReportFormat, ReportType,
    TradeType,
};
use crate::cli::AppContext;
use crate::errors::Result;
use crate::output::CommandOutput;
use crate::output::json_field;

#[enum_dispatch(Execute)]
#[derive(Debug, Subcommand)]
pub(crate) enum AccountCommand {
    /// Get all cash balances.
    Balance(Balance),
    /// Get extended balances including credits and held amounts.
    ExtendedBalance(ExtendedBalance),
    /// Get L3 (per-order) order book via the authenticated Level3 endpoint.
    ///
    /// Authenticated market data: it needs credentials, so it lives with the
    /// account handlers rather than the unauthenticated `market` path.
    OrderbookL3(OrderbookL3),
    /// Get credit line details (VIP only).
    CreditLines(CreditLines),
    /// Get margin/equity trade balance summary.
    TradeBalance(TradeBalance),
    /// Get currently open orders.
    OpenOrders(OpenOrders),
    /// Get closed orders.
    ClosedOrders(ClosedOrders),
    /// Query specific orders by TXID.
    QueryOrders(QueryOrders),
    /// Get trade history.
    TradesHistory(TradesHistory),
    /// Query specific trades by TXID.
    QueryTrades(QueryTrades),
    /// Get open margin positions.
    Positions(Positions),
    /// Get ledger entries.
    Ledgers(Ledgers),
    /// Query specific ledger entries.
    QueryLedgers(QueryLedgers),
    /// Get trade volume and fee info.
    Volume(Volume),
    /// Request an export report.
    ExportReport(ExportReport),
    /// Check export report status.
    ExportStatus(ExportStatus),
    /// Download an export report.
    ExportRetrieve(ExportRetrieve),
    /// Delete an export report.
    ExportDelete(ExportDelete),
}

#[derive(Debug, clap::Args)]
pub(crate) struct Balance {
    #[command(flatten)]
    rebase: RebaseOpts,
}

impl Balance {
    /// The plain venue view, for `workspace balance` addressing the real
    /// account: no rebase, exactly the bare `kraken balance` request.
    pub(crate) fn plain() -> Self {
        Self {
            rebase: RebaseOpts {
                rebase_multiplier: None,
            },
        }
    }
}

impl OpenOrders {
    /// Venue-only filters have no paper meaning; refuse rather than ignore.
    fn ensure_paper_plain(&self, manifest: &kraken_workspace::WorkspaceManifest) -> Result<()> {
        let set: &[(&str, bool)] = &[
            ("open-orders --trades", self.trades),
            ("open-orders --userref", self.userref.is_some()),
            ("open-orders --cl-ord-id", self.cl_ord_id.is_some()),
            (
                "open-orders --rebase-multiplier",
                self.rebase.rebase_multiplier.is_some(),
            ),
        ];
        ensure_no_venue_only_flags(set, manifest)
    }
}

impl TradesHistory {
    /// Venue-only filters have no paper meaning; refuse rather than ignore.
    fn ensure_paper_plain(&self, manifest: &kraken_workspace::WorkspaceManifest) -> Result<()> {
        let set: &[(&str, bool)] = &[
            ("trades-history --type", self.trade_type.is_some()),
            ("trades-history --trades", self.trades),
            ("trades-history --consolidate-taker", self.consolidate_taker),
            ("trades-history --ledgers", self.ledgers),
            ("trades-history --start", self.page.is_bounded()),
            (
                "trades-history --rebase-multiplier",
                self.rebase.rebase_multiplier.is_some(),
            ),
        ];
        ensure_no_venue_only_flags(set, manifest)
    }
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
            super::workspace_guard::ActiveMode::Paper(manifest) => {
                super::workspace::Balance {
                    name: Some(manifest.name),
                }
                .execute(ctx)
                .await
            }
            super::workspace_guard::ActiveMode::Live(manifest) => {
                Err(kraken_workspace::WorkspaceError::LiveUnavailable {
                    name: manifest.name,
                }
                .into())
            }
            super::workspace_guard::ActiveMode::Master => self.venue(ctx).await,
        }
    }
}

impl Balance {
    /// The unrouted venue request — also the `workspace balance` default
    /// target, which is the master account by definition.
    pub(crate) async fn venue(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self { rebase } = self;
        let mut params = HashMap::new();
        rebase.apply(&mut params);
        let mut data = client
            .private_post("Balance", params, creds, otp, true)
            .await?;
        schema::balance(&mut data);
        let (headers, rows) = parse_balance_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct ExtendedBalance {}

impl Execute for ExtendedBalance {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let mut data = client
            .private_post("BalanceEx", HashMap::new(), creds, otp, true)
            .await?;
        schema::extended_balance(&mut data);
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct OrderbookL3 {
    /// Trading pair.
    pair: String,
    /// Number of price levels per side (0 for full book).
    #[arg(long, default_value = "100")]
    depth: L3Depth,
}

impl Execute for OrderbookL3 {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self { pair, depth } = self;
        let mut body = serde_json::Map::new();
        body.insert("pair".into(), Value::String(pair));
        body.insert("depth".into(), Value::Number(depth.as_u64().into()));
        let data = client
            .private_post_json("Level3", body, creds, otp, true)
            .await?;
        Ok(CommandOutput::new(
            data,
            vec!["Data".into()],
            vec![vec!["L3 orderbook data returned (see JSON output)".into()]],
        ))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct CreditLines {
    #[command(flatten)]
    rebase: RebaseOpts,
}

impl Execute for CreditLines {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self { rebase } = self;
        let mut params = HashMap::new();
        rebase.apply(&mut params);
        let data = client
            .private_post("CreditLines", params, creds, otp, true)
            .await?;
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct TradeBalance {
    /// Base asset for balance calculation (default: ZUSD).
    #[arg(long)]
    asset: Option<String>,
    #[command(flatten)]
    rebase: RebaseOpts,
}

impl TradeBalance {
    /// The plain venue view, for `workspace status` addressing the real
    /// account: default asset, no rebase.
    pub(crate) fn plain() -> Self {
        Self {
            asset: None,
            rebase: RebaseOpts {
                rebase_multiplier: None,
            },
        }
    }
}

impl Execute for TradeBalance {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self { asset, rebase } = self;
        let mut params = HashMap::new();
        if let Some(a) = asset {
            params.insert("asset".into(), a);
        }
        rebase.apply(&mut params);
        let mut data = client
            .private_post("TradeBalance", params, creds, otp, true)
            .await?;
        schema::trade_balance(&mut data);
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct OpenOrders {
    /// Include trade details.
    #[arg(long)]
    trades: bool,
    /// Filter by user reference ID.
    #[arg(long)]
    userref: Option<i32>,
    /// Filter by client order ID.
    #[arg(long)]
    cl_ord_id: Option<String>,
    #[command(flatten)]
    rebase: RebaseOpts,
}

impl Execute for OpenOrders {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        match super::workspace_guard::active_mode(ctx)? {
            super::workspace_guard::ActiveMode::Paper(manifest) => {
                self.ensure_paper_plain(&manifest)?;
                let mut out = super::paper::orders_on_workspace(ctx, &manifest).await?;
                out.stamp_workspace(&manifest.name);
                Ok(out)
            }
            super::workspace_guard::ActiveMode::Live(manifest) => {
                Err(kraken_workspace::WorkspaceError::LiveUnavailable {
                    name: manifest.name,
                }
                .into())
            }
            super::workspace_guard::ActiveMode::Master => self.venue(ctx).await,
        }
    }
}

impl OpenOrders {
    async fn venue(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            trades,
            userref,
            cl_ord_id,
            rebase,
        } = self;
        let mut params = HashMap::new();
        if trades {
            params.insert("trades".into(), "true".into());
        }
        if let Some(r) = userref {
            params.insert("userref".into(), r.to_string());
        }
        if let Some(id) = cl_ord_id {
            params.insert("cl_ord_id".into(), id);
        }
        rebase.apply(&mut params);
        let mut data = client
            .private_post("OpenOrders", params, creds, otp, true)
            .await?;
        schema::order_rows(&mut data);
        let (headers, rows) = parse_orders_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct ClosedOrders {
    /// Include trade details.
    #[arg(long)]
    trades: bool,
    #[command(flatten)]
    page: Pagination,
    /// User reference filter.
    #[arg(long)]
    userref: Option<i32>,
    /// Filter by client order ID.
    #[arg(long)]
    cl_ord_id: Option<String>,
    /// Which time to use (open, close, both).
    #[arg(long)]
    closetime: Option<Closetime>,
    /// Consolidate trades by individual taker trades.
    #[arg(long)]
    consolidate_taker: bool,
    #[command(flatten)]
    rebase: RebaseOpts,
}

impl Execute for ClosedOrders {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            trades,
            page,
            userref,
            cl_ord_id,
            closetime,
            consolidate_taker,
            rebase,
        } = self;
        let mut params = HashMap::new();
        if trades {
            params.insert("trades".into(), "true".into());
        }
        page.apply(&mut params);
        if let Some(r) = userref {
            params.insert("userref".into(), r.to_string());
        }
        if let Some(id) = cl_ord_id {
            params.insert("cl_ord_id".into(), id);
        }
        if let Some(ct) = closetime {
            params.insert("closetime".into(), ct.as_wire());
        }
        if consolidate_taker {
            params.insert("consolidate_taker".into(), "true".into());
        }
        rebase.apply(&mut params);
        let mut data = client
            .private_post("ClosedOrders", params, creds, otp, true)
            .await?;
        schema::order_rows(&mut data);
        let (headers, rows) = parse_orders_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct QueryOrders {
    /// Transaction IDs (comma-delimited, up to 50).
    #[arg(num_args = 1..)]
    txids: Vec<String>,
    /// Include trade details.
    #[arg(long)]
    trades: bool,
    /// User reference filter.
    #[arg(long)]
    userref: Option<i32>,
    /// Consolidate taker trades.
    #[arg(long)]
    consolidate_taker: bool,
    #[command(flatten)]
    rebase: RebaseOpts,
}

impl Execute for QueryOrders {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            txids,
            trades,
            userref,
            consolidate_taker,
            rebase,
        } = self;
        let mut params = HashMap::new();
        params.insert("txid".into(), txids.join(","));
        if trades {
            params.insert("trades".into(), "true".into());
        }
        if let Some(r) = userref {
            params.insert("userref".into(), r.to_string());
        }
        if consolidate_taker {
            params.insert("consolidate_taker".into(), "true".into());
        }
        rebase.apply(&mut params);
        let data = client
            .private_post("QueryOrders", params, creds, otp, true)
            .await?;
        let (headers, rows) = parse_orders_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct TradesHistory {
    /// Trade type filter.
    #[arg(long = "type")]
    trade_type: Option<TradeType>,
    /// Include trades related to position in output.
    #[arg(long)]
    trades: bool,
    /// Consolidate trades by individual taker trades.
    #[arg(long)]
    consolidate_taker: bool,
    /// Include related ledger IDs for each trade (slower).
    #[arg(long)]
    ledgers: bool,
    #[command(flatten)]
    page: Pagination,
    #[command(flatten)]
    rebase: RebaseOpts,
}

impl Execute for TradesHistory {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        match super::workspace_guard::active_mode(ctx)? {
            super::workspace_guard::ActiveMode::Paper(manifest) => {
                self.ensure_paper_plain(&manifest)?;
                let mut out = super::paper::history_on_workspace(ctx, &manifest).await?;
                out.stamp_workspace(&manifest.name);
                Ok(out)
            }
            super::workspace_guard::ActiveMode::Live(manifest) => {
                Err(kraken_workspace::WorkspaceError::LiveUnavailable {
                    name: manifest.name,
                }
                .into())
            }
            super::workspace_guard::ActiveMode::Master => self.venue(ctx).await,
        }
    }
}

impl TradesHistory {
    async fn venue(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            trade_type,
            trades,
            consolidate_taker,
            ledgers,
            page,
            rebase,
        } = self;
        let mut params = HashMap::new();
        if let Some(t) = trade_type {
            params.insert("type".into(), t.as_wire());
        }
        if trades {
            params.insert("trades".into(), "true".into());
        }
        if consolidate_taker {
            params.insert("consolidate_taker".into(), "true".into());
        }
        if ledgers {
            params.insert("ledgers".into(), "true".into());
        }
        page.apply(&mut params);
        rebase.apply(&mut params);
        let mut data = client
            .private_post("TradesHistory", params, creds, otp, true)
            .await?;
        schema::trade_rows(&mut data);
        let (headers, rows) = parse_trades_history(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct QueryTrades {
    /// Transaction IDs (comma-delimited, up to 20).
    #[arg(num_args = 1..)]
    txids: Vec<String>,
    /// Include trades related to position in output.
    #[arg(long)]
    trades: bool,
    #[command(flatten)]
    rebase: RebaseOpts,
}

impl Execute for QueryTrades {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            txids,
            trades,
            rebase,
        } = self;
        let mut params = HashMap::new();
        params.insert("txid".into(), txids.join(","));
        if trades {
            params.insert("trades".into(), "true".into());
        }
        rebase.apply(&mut params);
        let data = client
            .private_post("QueryTrades", params, creds, otp, true)
            .await?;
        let (headers, rows) = parse_trades_history(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Positions {
    /// Filter by TXIDs.
    #[arg(long)]
    txid: Vec<String>,
    /// Include P&L calculations.
    #[arg(long)]
    show_pnl: bool,
    /// Consolidate positions by market/pair.
    #[arg(long)]
    consolidation: Option<String>,
    #[command(flatten)]
    rebase: RebaseOpts,
}

impl Execute for Positions {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            txid,
            show_pnl,
            consolidation,
            rebase,
        } = self;
        let mut params = HashMap::new();
        if !txid.is_empty() {
            params.insert("txid".into(), txid.join(","));
        }
        if show_pnl {
            params.insert("docalcs".into(), "true".into());
        }
        if let Some(c) = consolidation {
            params.insert("consolidation".into(), c);
        }
        rebase.apply(&mut params);
        let mut data = client
            .private_post("OpenPositions", params, creds, otp, true)
            .await?;
        schema::position_rows(&mut data);
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Ledgers {
    /// Filter by asset or comma-delimited list of assets.
    #[arg(long)]
    asset: Option<String>,
    /// Type of ledger to retrieve.
    // Kraken's ledger-type set grows server-side (staking, dividends, NFT
    // rebates arrived after launch); constraining it here would break valid
    // filters, so the server validates.
    #[arg(long = "type")]
    ledger_type: Option<String>,
    /// Filter by asset class.
    #[arg(long, alias = "aclass")]
    asset_class: Option<AssetClass>,
    #[command(flatten)]
    page: Pagination,
    #[command(flatten)]
    rebase: RebaseOpts,
}

impl Execute for Ledgers {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            asset,
            ledger_type,
            asset_class,
            page,
            rebase,
        } = self;
        let mut params = HashMap::new();
        if let Some(a) = asset {
            params.insert("asset".into(), a);
        }
        if let Some(t) = ledger_type {
            params.insert("type".into(), t);
        }
        if let Some(ac) = asset_class {
            params.insert("aclass".into(), ac.as_wire());
        }
        page.apply(&mut params);
        rebase.apply(&mut params);
        let mut data = client
            .private_post("Ledgers", params, creds, otp, true)
            .await?;
        schema::ledger_rows(&mut data);
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct QueryLedgers {
    /// Ledger IDs (comma-delimited, up to 20).
    #[arg(num_args = 1..)]
    ids: Vec<String>,
    /// Include trades related to position in output.
    #[arg(long)]
    trades: bool,
    #[command(flatten)]
    rebase: RebaseOpts,
}

impl Execute for QueryLedgers {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            ids,
            trades,
            rebase,
        } = self;
        let mut params = HashMap::new();
        params.insert("id".into(), ids.join(","));
        if trades {
            params.insert("trades".into(), "true".into());
        }
        rebase.apply(&mut params);
        let mut data = client
            .private_post("QueryLedgers", params, creds, otp, true)
            .await?;
        schema::ledger_rows(&mut data);
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Volume {
    /// Comma-delimited list of asset pairs for fee info.
    #[arg(long)]
    pair: Vec<String>,
    #[command(flatten)]
    rebase: RebaseOpts,
}

impl Execute for Volume {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self { pair, rebase } = self;
        let mut params = HashMap::new();
        if !pair.is_empty() {
            params.insert("pair".into(), pair.join(","));
        }
        rebase.apply(&mut params);
        let mut data = client
            .private_post("TradeVolume", params, creds, otp, true)
            .await?;
        schema::trade_volume(&mut data);
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct ExportReport {
    /// Type of data to export (trades or ledgers).
    #[arg(long)]
    report: ReportType,
    /// Description for the export.
    #[arg(long)]
    description: String,
    /// File format.
    #[arg(long, default_value = "CSV")]
    format: ReportFormat,
    /// Comma-delimited list of fields to include (default: all).
    #[arg(long)]
    fields: Option<String>,
    /// UNIX timestamp for report start time (default: 1st of current month).
    #[arg(long)]
    starttm: Option<String>,
    /// UNIX timestamp for report end time (default: now).
    #[arg(long)]
    endtm: Option<String>,
}

impl Execute for ExportReport {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            report,
            description,
            format,
            fields,
            starttm,
            endtm,
        } = self;
        let mut params = HashMap::new();
        params.insert("report".into(), report.as_wire());
        params.insert("description".into(), description);
        params.insert("format".into(), format.as_wire());
        if let Some(f) = fields {
            params.insert("fields".into(), f);
        }
        if let Some(s) = starttm {
            params.insert("starttm".into(), s);
        }
        if let Some(e) = endtm {
            params.insert("endtm".into(), e);
        }
        let data = client
            .private_post("AddExport", params, creds, otp, false)
            .await?;
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct ExportStatus {
    /// Report type (trades or ledgers).
    #[arg(long)]
    report: ReportType,
}

impl Execute for ExportStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self { report } = self;
        let mut params = HashMap::new();
        params.insert("report".into(), report.as_wire());
        let data = client
            .private_post("ExportStatus", params, creds, otp, true)
            .await?;
        let pairs = parse_kv_pairs(&data);
        Ok(CommandOutput::key_value(pairs, data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct ExportRetrieve {
    /// Report ID to retrieve.
    report_id: String,
    /// Output file path for the downloaded report.
    #[arg(long)]
    output_file: Option<PathBuf>,
}

impl Execute for ExportRetrieve {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self {
            report_id,
            output_file,
        } = self;
        let mut params = HashMap::new();
        params.insert("id".into(), report_id.clone());

        let bytes = client
            .private_post_raw("RetrieveExport", params, creds, otp, true)
            .await?;

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

#[derive(Debug, clap::Args)]
pub(crate) struct ExportDelete {
    /// Report ID to delete.
    report_id: String,
}

impl Execute for ExportDelete {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let Self { report_id } = self;
        let mut params = HashMap::new();
        params.insert("id".into(), report_id);
        let data = client
            .private_post("RemoveExport", params, creds, otp, false)
            .await?;
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
                json_field_or(descr, "side", "type"),
                json_field(descr, "pair"),
                json_field(descr, "price"),
                json_field_or(order, "volume", "vol"),
            ]);
        }
    }
    (headers, rows)
}

/// A field under its published schema name, falling back to its raw wire
/// name — one renderer serves both the schema-shaped rows and the raw rows
/// of the query-* commands, which still emit the wire form.
fn json_field_or(val: &Value, schema_key: &str, wire: &str) -> String {
    if val.get(schema_key).is_some() {
        json_field(val, schema_key)
    } else {
        json_field(val, wire)
    }
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
                json_field_or(trade, "side", "type"),
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
