/// Private account data commands (balance, orders, history, exports).
use std::collections::HashMap;
use std::path::PathBuf;

use clap::Subcommand;
use serde_json::Value;

use super::helpers::jstr;
use crate::client::SpotClient;
use crate::config::SpotCredentials;
use crate::errors::Result;
use crate::output::CommandOutput;

#[derive(Debug, Subcommand)]
pub(crate) enum AccountCommand {
    /// Get all cash balances.
    Balance {
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get extended balances including credits and held amounts.
    ExtendedBalance,
    /// Get credit line details (VIP only).
    CreditLines {
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get margin/equity trade balance summary.
    TradeBalance {
        /// Base asset for balance calculation (default: ZUSD).
        #[arg(long)]
        asset: Option<String>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get currently open orders.
    OpenOrders {
        /// Include trade details.
        #[arg(long)]
        trades: bool,
        /// Filter by user reference ID.
        #[arg(long)]
        userref: Option<String>,
        /// Filter by client order ID.
        #[arg(long)]
        cl_ord_id: Option<String>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get closed orders.
    ClosedOrders {
        /// Include trade details.
        #[arg(long)]
        trades: bool,
        /// Start unix timestamp or order tx ID (exclusive).
        #[arg(long)]
        start: Option<String>,
        /// End unix timestamp or order tx ID (inclusive).
        #[arg(long)]
        end: Option<String>,
        /// Result offset for pagination.
        #[arg(long)]
        offset: Option<u64>,
        /// User reference filter.
        #[arg(long)]
        userref: Option<String>,
        /// Filter by client order ID.
        #[arg(long)]
        cl_ord_id: Option<String>,
        /// Which time to use (open, close, both).
        #[arg(long)]
        closetime: Option<String>,
        /// Consolidate trades by individual taker trades.
        #[arg(long)]
        consolidate_taker: bool,
        /// Omit page count from result (faster for large histories).
        #[arg(long)]
        without_count: bool,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Query specific orders by TXID.
    QueryOrders {
        /// Transaction IDs (comma-delimited, up to 50).
        txids: Vec<String>,
        /// Include trade details.
        #[arg(long)]
        trades: bool,
        /// User reference filter.
        #[arg(long)]
        userref: Option<String>,
        /// Consolidate taker trades.
        #[arg(long)]
        consolidate_taker: bool,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get trade history.
    TradesHistory {
        /// Trade type filter (all, any position, closed position, closing position, no position).
        #[arg(long = "type")]
        trade_type: Option<String>,
        /// Starting unix timestamp or trade tx ID (exclusive).
        #[arg(long)]
        start: Option<String>,
        /// Ending unix timestamp or trade tx ID (inclusive).
        #[arg(long)]
        end: Option<String>,
        /// Include trades related to position in output.
        #[arg(long)]
        trades: bool,
        /// Result offset for pagination.
        #[arg(long)]
        offset: Option<u64>,
        /// Consolidate trades by individual taker trades.
        #[arg(long)]
        consolidate_taker: bool,
        /// Omit count from result (faster for large histories).
        #[arg(long)]
        without_count: bool,
        /// Include related ledger IDs for each trade (slower).
        #[arg(long)]
        ledgers: bool,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Query specific trades by TXID.
    QueryTrades {
        /// Transaction IDs (comma-delimited, up to 20).
        txids: Vec<String>,
        /// Include trades related to position in output.
        #[arg(long)]
        trades: bool,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get open margin positions.
    Positions {
        /// Filter by TXIDs.
        #[arg(long)]
        txid: Vec<String>,
        /// Include P&L calculations.
        #[arg(long)]
        show_pnl: bool,
        /// Consolidate positions by market/pair.
        #[arg(long)]
        consolidation: Option<String>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get ledger entries.
    Ledgers {
        /// Filter by asset or comma-delimited list of assets.
        #[arg(long)]
        asset: Option<String>,
        /// Type of ledger to retrieve.
        #[arg(long = "type")]
        ledger_type: Option<String>,
        /// Starting unix timestamp or ledger ID (exclusive).
        #[arg(long)]
        start: Option<String>,
        /// Ending unix timestamp or ledger ID (inclusive).
        #[arg(long)]
        end: Option<String>,
        /// Filter by asset class.
        #[arg(long)]
        aclass: Option<String>,
        /// Result offset for pagination.
        #[arg(long)]
        offset: Option<u64>,
        /// Omit count from result (faster for large ledgers).
        #[arg(long)]
        without_count: bool,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Query specific ledger entries.
    QueryLedgers {
        /// Ledger IDs (comma-delimited, up to 20).
        ids: Vec<String>,
        /// Include trades related to position in output.
        #[arg(long)]
        trades: bool,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get trade volume and fee info.
    Volume {
        /// Comma-delimited list of asset pairs for fee info.
        #[arg(long)]
        pair: Vec<String>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Request an export report.
    ExportReport {
        /// Type of data to export (trades or ledgers).
        #[arg(long)]
        report: String,
        /// Description for the export.
        #[arg(long)]
        description: String,
        /// File format (CSV or TSV).
        #[arg(long, default_value = "CSV")]
        format: String,
        /// Comma-delimited list of fields to include (default: all).
        #[arg(long)]
        fields: Option<String>,
        /// UNIX timestamp for report start time (default: 1st of current month).
        #[arg(long)]
        starttm: Option<String>,
        /// UNIX timestamp for report end time (default: now).
        #[arg(long)]
        endtm: Option<String>,
    },
    /// Check export report status.
    ExportStatus {
        /// Report type (trades or ledgers).
        #[arg(long)]
        report: String,
    },
    /// Download an export report.
    ExportRetrieve {
        /// Report ID to retrieve.
        report_id: String,
        /// Output file path for the downloaded report.
        #[arg(long)]
        output_file: Option<PathBuf>,
    },
    /// Delete an export report.
    ExportDelete {
        /// Report ID to delete.
        report_id: String,
    },
}

pub(crate) async fn execute(
    cmd: &AccountCommand,
    client: &SpotClient,
    creds: &SpotCredentials,
    otp: Option<&str>,
    verbose: bool,
) -> Result<CommandOutput> {
    match cmd {
        AccountCommand::Balance { rebase_multiplier } => {
            let mut params = HashMap::new();
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("Balance", params, creds, otp, true, verbose)
                .await?;
            let (headers, rows) = parse_balance_table(&data);
            Ok(CommandOutput::new(data, headers, rows))
        }
        AccountCommand::ExtendedBalance => {
            let data = client
                .private_post("BalanceEx", HashMap::new(), creds, otp, true, verbose)
                .await?;
            let pairs = parse_kv_pairs(&data);
            Ok(CommandOutput::key_value(pairs, data))
        }
        AccountCommand::CreditLines { rebase_multiplier } => {
            let mut params = HashMap::new();
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("CreditLines", params, creds, otp, true, verbose)
                .await?;
            let pairs = parse_kv_pairs(&data);
            Ok(CommandOutput::key_value(pairs, data))
        }
        AccountCommand::TradeBalance {
            asset,
            rebase_multiplier,
        } => {
            let mut params = HashMap::new();
            if let Some(a) = asset {
                params.insert("asset".into(), a.clone());
            }
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("TradeBalance", params, creds, otp, true, verbose)
                .await?;
            let pairs = parse_kv_pairs(&data);
            Ok(CommandOutput::key_value(pairs, data))
        }
        AccountCommand::OpenOrders {
            trades,
            userref,
            cl_ord_id,
            rebase_multiplier,
        } => {
            let mut params = HashMap::new();
            if *trades {
                params.insert("trades".into(), "true".into());
            }
            if let Some(r) = userref {
                params.insert("userref".into(), r.clone());
            }
            if let Some(id) = cl_ord_id {
                params.insert("cl_ord_id".into(), id.clone());
            }
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("OpenOrders", params, creds, otp, true, verbose)
                .await?;
            let (headers, rows) = parse_orders_table(&data);
            Ok(CommandOutput::new(data, headers, rows))
        }
        AccountCommand::ClosedOrders {
            trades,
            start,
            end,
            offset,
            userref,
            cl_ord_id,
            closetime,
            consolidate_taker,
            without_count,
            rebase_multiplier,
        } => {
            let mut params = HashMap::new();
            if *trades {
                params.insert("trades".into(), "true".into());
            }
            if let Some(s) = start {
                params.insert("start".into(), s.clone());
            }
            if let Some(e) = end {
                params.insert("end".into(), e.clone());
            }
            if let Some(o) = offset {
                params.insert("ofs".into(), o.to_string());
            }
            if let Some(r) = userref {
                params.insert("userref".into(), r.clone());
            }
            if let Some(id) = cl_ord_id {
                params.insert("cl_ord_id".into(), id.clone());
            }
            if let Some(ct) = closetime {
                params.insert("closetime".into(), ct.clone());
            }
            if *consolidate_taker {
                params.insert("consolidate_taker".into(), "true".into());
            }
            if *without_count {
                params.insert("without_count".into(), "true".into());
            }
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("ClosedOrders", params, creds, otp, true, verbose)
                .await?;
            let (headers, rows) = parse_orders_table(&data);
            Ok(CommandOutput::new(data, headers, rows))
        }
        AccountCommand::QueryOrders {
            txids,
            trades,
            userref,
            consolidate_taker,
            rebase_multiplier,
        } => {
            let mut params = HashMap::new();
            params.insert("txid".into(), txids.join(","));
            if *trades {
                params.insert("trades".into(), "true".into());
            }
            if let Some(r) = userref {
                params.insert("userref".into(), r.clone());
            }
            if *consolidate_taker {
                params.insert("consolidate_taker".into(), "true".into());
            }
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("QueryOrders", params, creds, otp, true, verbose)
                .await?;
            let (headers, rows) = parse_orders_table(&data);
            Ok(CommandOutput::new(data, headers, rows))
        }
        AccountCommand::TradesHistory {
            trade_type,
            start,
            end,
            trades,
            offset,
            consolidate_taker,
            without_count,
            ledgers,
            rebase_multiplier,
        } => {
            let mut params = HashMap::new();
            if let Some(t) = trade_type {
                params.insert("type".into(), t.clone());
            }
            if let Some(s) = start {
                params.insert("start".into(), s.clone());
            }
            if let Some(e) = end {
                params.insert("end".into(), e.clone());
            }
            if *trades {
                params.insert("trades".into(), "true".into());
            }
            if let Some(o) = offset {
                params.insert("ofs".into(), o.to_string());
            }
            if *consolidate_taker {
                params.insert("consolidate_taker".into(), "true".into());
            }
            if *without_count {
                params.insert("without_count".into(), "true".into());
            }
            if *ledgers {
                params.insert("ledgers".into(), "true".into());
            }
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("TradesHistory", params, creds, otp, true, verbose)
                .await?;
            let (headers, rows) = parse_trades_history(&data);
            Ok(CommandOutput::new(data, headers, rows))
        }
        AccountCommand::QueryTrades {
            txids,
            trades,
            rebase_multiplier,
        } => {
            let mut params = HashMap::new();
            params.insert("txid".into(), txids.join(","));
            if *trades {
                params.insert("trades".into(), "true".into());
            }
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("QueryTrades", params, creds, otp, true, verbose)
                .await?;
            let (headers, rows) = parse_trades_history(&data);
            Ok(CommandOutput::new(data, headers, rows))
        }
        AccountCommand::Positions {
            txid,
            show_pnl,
            consolidation,
            rebase_multiplier,
        } => {
            let mut params = HashMap::new();
            if !txid.is_empty() {
                params.insert("txid".into(), txid.join(","));
            }
            if *show_pnl {
                params.insert("docalcs".into(), "true".into());
            }
            if let Some(c) = consolidation {
                params.insert("consolidation".into(), c.clone());
            }
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("OpenPositions", params, creds, otp, true, verbose)
                .await?;
            let pairs = parse_kv_pairs(&data);
            Ok(CommandOutput::key_value(pairs, data))
        }
        AccountCommand::Ledgers {
            asset,
            ledger_type,
            start,
            end,
            aclass,
            offset,
            without_count,
            rebase_multiplier,
        } => {
            let mut params = HashMap::new();
            if let Some(a) = asset {
                params.insert("asset".into(), a.clone());
            }
            if let Some(t) = ledger_type {
                params.insert("type".into(), t.clone());
            }
            if let Some(s) = start {
                params.insert("start".into(), s.clone());
            }
            if let Some(e) = end {
                params.insert("end".into(), e.clone());
            }
            if let Some(ac) = aclass {
                params.insert("aclass".into(), ac.clone());
            }
            if let Some(o) = offset {
                params.insert("ofs".into(), o.to_string());
            }
            if *without_count {
                params.insert("without_count".into(), "true".into());
            }
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("Ledgers", params, creds, otp, true, verbose)
                .await?;
            let pairs = parse_kv_pairs(&data);
            Ok(CommandOutput::key_value(pairs, data))
        }
        AccountCommand::QueryLedgers {
            ids,
            trades,
            rebase_multiplier,
        } => {
            let mut params = HashMap::new();
            params.insert("id".into(), ids.join(","));
            if *trades {
                params.insert("trades".into(), "true".into());
            }
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("QueryLedgers", params, creds, otp, true, verbose)
                .await?;
            let pairs = parse_kv_pairs(&data);
            Ok(CommandOutput::key_value(pairs, data))
        }
        AccountCommand::Volume {
            pair,
            rebase_multiplier,
        } => {
            let mut params = HashMap::new();
            if !pair.is_empty() {
                params.insert("pair".into(), pair.join(","));
            }
            if let Some(rm) = rebase_multiplier {
                params.insert("rebase_multiplier".into(), rm.clone());
            }
            let data = client
                .private_post("TradeVolume", params, creds, otp, true, verbose)
                .await?;
            let pairs = parse_kv_pairs(&data);
            Ok(CommandOutput::key_value(pairs, data))
        }
        AccountCommand::ExportReport {
            report,
            description,
            format,
            fields,
            starttm,
            endtm,
        } => {
            let mut params = HashMap::new();
            params.insert("report".into(), report.clone());
            params.insert("description".into(), description.clone());
            params.insert("format".into(), format.clone());
            if let Some(f) = fields {
                params.insert("fields".into(), f.clone());
            }
            if let Some(s) = starttm {
                params.insert("starttm".into(), s.clone());
            }
            if let Some(e) = endtm {
                params.insert("endtm".into(), e.clone());
            }
            let data = client
                .private_post("AddExport", params, creds, otp, false, verbose)
                .await?;
            let pairs = parse_kv_pairs(&data);
            Ok(CommandOutput::key_value(pairs, data))
        }
        AccountCommand::ExportStatus { report } => {
            let mut params = HashMap::new();
            params.insert("report".into(), report.clone());
            let data = client
                .private_post("ExportStatus", params, creds, otp, true, verbose)
                .await?;
            let pairs = parse_kv_pairs(&data);
            Ok(CommandOutput::key_value(pairs, data))
        }
        AccountCommand::ExportRetrieve {
            report_id,
            output_file,
        } => {
            let mut params = HashMap::new();
            params.insert("id".into(), report_id.clone());

            let bytes = client
                .private_post_raw("RetrieveExport", params, creds, otp, true, verbose)
                .await?;

            let dest = output_file
                .clone()
                .unwrap_or_else(|| PathBuf::from(format!("kraken_export_{report_id}.zip")));

            std::fs::write(&dest, &bytes)?;

            let meta = serde_json::json!({
                "report_id": report_id,
                "file": dest.display().to_string(),
                "size_bytes": bytes.len(),
            });
            Ok(CommandOutput::key_value(
                vec![
                    ("Report ID".into(), report_id.clone()),
                    ("File".into(), dest.display().to_string()),
                    ("Size".into(), format!("{} bytes", bytes.len())),
                ],
                meta,
            ))
        }
        AccountCommand::ExportDelete { report_id } => {
            let mut params = HashMap::new();
            params.insert("id".into(), report_id.clone());
            let data = client
                .private_post("RemoveExport", params, creds, otp, false, verbose)
                .await?;
            let pairs = parse_kv_pairs(&data);
            Ok(CommandOutput::key_value(pairs, data))
        }
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
        "Type".into(),
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
                jstr(order, "status"),
                jstr(descr, "type"),
                jstr(descr, "pair"),
                jstr(descr, "price"),
                jstr(order, "vol"),
            ]);
        }
    }
    (headers, rows)
}

fn parse_trades_history(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec![
        "TXID".into(),
        "Pair".into(),
        "Type".into(),
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
                jstr(trade, "pair"),
                jstr(trade, "type"),
                jstr(trade, "price"),
                jstr(trade, "vol"),
                jstr(trade, "cost"),
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
