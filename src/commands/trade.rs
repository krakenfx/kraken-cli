/// Spot trading commands: buy, sell, batch, amend, edit, cancel, cancel-batch,
/// cancel-all, cancel-after.
use std::collections::HashMap;
use std::path::PathBuf;

use clap::Subcommand;
use serde_json::Value;

use super::helpers::confirm_destructive;
use crate::client::SpotClient;
use crate::config::SpotCredentials;
use crate::errors::{KrakenError, Result};
use crate::output::CommandOutput;

#[derive(Debug, Subcommand)]
pub(crate) enum OrderCommand {
    /// Place a buy order.
    Buy {
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
        /// User reference ID (signed 32-bit integer, for tagging groups of orders).
        #[arg(long)]
        userref: Option<String>,
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
    },
    /// Place a sell order.
    Sell {
        /// Trading pair.
        pair: String,
        /// Order volume.
        volume: String,
        /// Order type (market, limit, iceberg, stop-loss, take-profit, stop-loss-limit,
        /// take-profit-limit, trailing-stop, trailing-stop-limit, settle-position).
        #[arg(long, default_value = "limit")]
        r#type: String,
        /// Price.
        #[arg(long)]
        price: Option<String>,
        /// Limit price for stop-loss-limit, take-profit-limit, and trailing-stop-limit orders.
        #[arg(long)]
        price2: Option<String>,
        /// Display volume for iceberg orders.
        #[arg(long)]
        displayvol: Option<String>,
        /// Price signal for triggered orders (last or index).
        #[arg(long)]
        trigger: Option<String>,
        /// Leverage.
        #[arg(long)]
        leverage: Option<String>,
        /// Reduce-only flag.
        #[arg(long)]
        reduce_only: bool,
        /// Time-in-force.
        #[arg(long)]
        timeinforce: Option<String>,
        /// Start time.
        #[arg(long)]
        start_time: Option<String>,
        /// Expire time.
        #[arg(long)]
        expire_time: Option<String>,
        /// User reference ID (signed 32-bit integer).
        #[arg(long)]
        userref: Option<String>,
        /// Client order ID (mutually exclusive with userref).
        #[arg(long)]
        cl_ord_id: Option<String>,
        /// Order flags (comma-delimited: post, fcib, fciq, nompp, viqc).
        #[arg(long)]
        oflags: Option<String>,
        /// Self-trade prevention type (cancel-newest, cancel-oldest, cancel-both).
        #[arg(long)]
        stptype: Option<String>,
        /// Conditional close order type.
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
        /// Validate only.
        #[arg(long)]
        validate: bool,
        /// Asset class (required for non-crypto pairs, e.g. tokenized_asset for xstocks).
        #[arg(long)]
        asset_class: Option<String>,
    },
    /// Submit batch orders from a JSON file (2-15 orders).
    Batch {
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
    },
    /// Amend a live order in-place (preserves queue priority and identifiers).
    Amend {
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
    },
    /// Edit (cancel+replace) an order.
    Edit {
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
    },
    /// Cancel an open order by txid, userref, or client order ID.
    Cancel {
        /// Transaction ID(s) or userref (comma-separated for multiple).
        #[arg(num_args = 1..)]
        txids: Vec<String>,
        /// Cancel by client order ID instead of txid.
        #[arg(long)]
        cl_ord_id: Option<String>,
    },
    /// Cancel a batch of orders (max 50 total across txids and cl-ord-ids).
    CancelBatch {
        /// Transaction IDs or user references (up to 50).
        #[arg(num_args = 1..)]
        txids: Vec<String>,
        /// Client order IDs to cancel (up to 50 total combined with txids).
        #[arg(long, num_args = 1..)]
        cl_ord_ids: Vec<String>,
    },
    /// Cancel all open orders.
    CancelAll,
    /// Dead man's switch — cancel all orders after timeout.
    CancelAfter {
        /// Timeout in seconds (0 to disable).
        timeout: u64,
    },
}

pub(crate) async fn execute(
    cmd: &OrderCommand,
    client: &SpotClient,
    creds: &SpotCredentials,
    otp: Option<&str>,
    force: bool,
    verbose: bool,
) -> Result<CommandOutput> {
    match cmd {
        OrderCommand::Buy {
            pair,
            volume,
            r#type,
            price,
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
        } => {
            let params = build_order_params(
                "buy",
                pair,
                volume,
                r#type,
                price,
                price2,
                displayvol,
                trigger,
                leverage,
                *reduce_only,
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
                *validate,
                asset_class,
            )?;
            let data = client
                .private_post("AddOrder", params, creds, otp, false, verbose)
                .await?;
            Ok(parse_order_result(&data))
        }
        OrderCommand::Sell {
            pair,
            volume,
            r#type,
            price,
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
        } => {
            let params = build_order_params(
                "sell",
                pair,
                volume,
                r#type,
                price,
                price2,
                displayvol,
                trigger,
                leverage,
                *reduce_only,
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
                *validate,
                asset_class,
            )?;
            let data = client
                .private_post("AddOrder", params, creds, otp, false, verbose)
                .await?;
            Ok(parse_order_result(&data))
        }
        OrderCommand::Batch {
            json_file,
            pair,
            asset_class,
            deadline,
            validate,
        } => {
            let contents = std::fs::read_to_string(json_file)?;
            let orders: Value = serde_json::from_str(&contents)?;
            let arr = validate_batch_orders(&orders)?;

            let resolved_pair = if let Some(p) = pair {
                Some(p.clone())
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
                body.insert("asset_class".into(), Value::String(ac.clone()));
            }
            if let Some(dl) = deadline {
                body.insert("deadline".into(), Value::String(dl.clone()));
            }
            if *validate {
                body.insert("validate".into(), Value::Bool(true));
            }
            let data = client
                .private_post_json("AddOrderBatch", body, creds, otp, false, verbose)
                .await?;
            Ok(parse_order_result(&data))
        }
        OrderCommand::Amend {
            txid,
            cl_ord_id,
            order_qty,
            display_qty,
            limit_price,
            trigger_price,
            pair,
            post_only,
            deadline,
        } => {
            validate_amend_ids(txid, cl_ord_id)?;
            let mut params = HashMap::new();
            if let Some(id) = txid {
                params.insert("txid".into(), id.clone());
            }
            if let Some(id) = cl_ord_id {
                params.insert("cl_ord_id".into(), id.clone());
            }
            if let Some(qty) = order_qty {
                params.insert("order_qty".into(), qty.clone());
            }
            if let Some(dq) = display_qty {
                params.insert("display_qty".into(), dq.clone());
            }
            if let Some(lp) = limit_price {
                params.insert("limit_price".into(), lp.clone());
            }
            if let Some(tp) = trigger_price {
                params.insert("trigger_price".into(), tp.clone());
            }
            if let Some(p) = pair {
                params.insert("pair".into(), p.clone());
            }
            if *post_only {
                params.insert("post_only".into(), "true".into());
            }
            if let Some(dl) = deadline {
                params.insert("deadline".into(), dl.clone());
            }
            let data = client
                .private_post("AmendOrder", params, creds, otp, false, verbose)
                .await?;
            Ok(parse_order_result(&data))
        }
        OrderCommand::Edit {
            txid,
            volume,
            price,
            pair,
        } => {
            let mut params = HashMap::new();
            params.insert("txid".into(), txid.clone());
            if let Some(v) = volume {
                params.insert("volume".into(), v.clone());
            }
            if let Some(p) = price {
                params.insert("price".into(), p.clone());
            }
            if let Some(pair) = pair {
                params.insert("pair".into(), pair.clone());
            }
            let data = client
                .private_post("EditOrder", params, creds, otp, false, verbose)
                .await?;
            Ok(parse_order_result(&data))
        }
        OrderCommand::Cancel { txids, cl_ord_id } => {
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
            let mut params = HashMap::new();
            if !txids.is_empty() {
                params.insert("txid".into(), txids.join(","));
            }
            if let Some(id) = cl_ord_id {
                params.insert("cl_ord_id".into(), id.clone());
            }
            let data = client
                .private_post("CancelOrder", params, creds, otp, false, verbose)
                .await?;
            Ok(parse_order_result(&data))
        }
        OrderCommand::CancelBatch { txids, cl_ord_ids } => {
            validate_cancel_batch(txids, cl_ord_ids)?;
            let mut body = serde_json::Map::new();
            let all_ids: Vec<Value> = txids
                .iter()
                .chain(cl_ord_ids.iter())
                .map(|id| Value::String(id.clone()))
                .collect();
            body.insert("orders".into(), Value::Array(all_ids));
            let data = client
                .private_post_json("CancelOrderBatch", body, creds, otp, true, verbose)
                .await?;
            Ok(parse_order_result(&data))
        }
        OrderCommand::CancelAll => {
            if !force {
                confirm_destructive("Cancel ALL open orders?")?;
            }
            let data = client
                .private_post("CancelAll", HashMap::new(), creds, otp, false, verbose)
                .await?;
            Ok(parse_order_result(&data))
        }
        OrderCommand::CancelAfter { timeout } => {
            let mut params = HashMap::new();
            params.insert("timeout".into(), timeout.to_string());
            let data = client
                .private_post("CancelAllOrdersAfter", params, creds, otp, false, verbose)
                .await?;
            Ok(parse_order_result(&data))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_order_params(
    direction: &str,
    pair: &str,
    volume: &str,
    order_type: &str,
    price: &Option<String>,
    price2: &Option<String>,
    displayvol: &Option<String>,
    trigger: &Option<String>,
    leverage: &Option<String>,
    reduce_only: bool,
    timeinforce: &Option<String>,
    start_time: &Option<String>,
    expire_time: &Option<String>,
    userref: &Option<String>,
    cl_ord_id: &Option<String>,
    oflags: &Option<String>,
    stptype: &Option<String>,
    close_ordertype: &Option<String>,
    close_price: &Option<String>,
    close_price2: &Option<String>,
    deadline: &Option<String>,
    validate: bool,
    asset_class: &Option<String>,
) -> Result<HashMap<String, String>> {
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
    if let Some(t) = trigger {
        if !trigger_types.contains(&order_type) {
            return Err(KrakenError::Validation(format!(
                "trigger is only valid for {} orders",
                trigger_types.join(", ")
            )));
        }
        if !["last", "index"].contains(&t.as_str()) {
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
    params.insert("pair".into(), pair.to_string());
    params.insert("type".into(), direction.to_string());
    params.insert("ordertype".into(), order_type.to_string());
    params.insert("volume".into(), volume.to_string());

    if let Some(p) = price {
        params.insert("price".into(), p.clone());
    }
    if let Some(p2) = price2 {
        params.insert("price2".into(), p2.clone());
    }
    if let Some(dv) = displayvol {
        params.insert("displayvol".into(), dv.clone());
    }
    if let Some(t) = trigger {
        params.insert("trigger".into(), t.clone());
    }
    if let Some(lev) = leverage {
        params.insert("leverage".into(), lev.clone());
    }
    if reduce_only {
        params.insert("reduce_only".into(), "true".into());
    }
    if let Some(tif) = timeinforce {
        params.insert("timeinforce".into(), tif.clone());
    }
    if let Some(st) = start_time {
        params.insert("starttm".into(), st.clone());
    }
    if let Some(et) = expire_time {
        params.insert("expiretm".into(), et.clone());
    }
    if let Some(ur) = userref {
        params.insert("userref".into(), ur.clone());
    }
    if let Some(id) = cl_ord_id {
        params.insert("cl_ord_id".into(), id.clone());
    }
    if let Some(of) = oflags {
        params.insert("oflags".into(), of.clone());
    }
    if let Some(stp) = stptype {
        params.insert("stptype".into(), stp.clone());
    }
    if let Some(cot) = close_ordertype {
        params.insert("close[ordertype]".into(), cot.clone());
    }
    if let Some(cp) = close_price {
        params.insert("close[price]".into(), cp.clone());
    }
    if let Some(cp2) = close_price2 {
        params.insert("close[price2]".into(), cp2.clone());
    }
    if let Some(dl) = deadline {
        params.insert("deadline".into(), dl.clone());
    }
    if validate {
        params.insert("validate".into(), "true".into());
    }
    if let Some(ac) = asset_class {
        params.insert("asset_class".into(), ac.clone());
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

    fn default_params() -> BuildOrderArgs {
        BuildOrderArgs {
            direction: "buy",
            pair: "XBTUSD",
            volume: "0.01",
            order_type: "limit",
            price: Some("50000".into()),
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

    struct BuildOrderArgs {
        direction: &'static str,
        pair: &'static str,
        volume: &'static str,
        order_type: &'static str,
        price: Option<String>,
        price2: Option<String>,
        displayvol: Option<String>,
        trigger: Option<String>,
        leverage: Option<String>,
        reduce_only: bool,
        timeinforce: Option<String>,
        start_time: Option<String>,
        expire_time: Option<String>,
        userref: Option<String>,
        cl_ord_id: Option<String>,
        oflags: Option<String>,
        stptype: Option<String>,
        close_ordertype: Option<String>,
        close_price: Option<String>,
        close_price2: Option<String>,
        deadline: Option<String>,
        validate: bool,
        asset_class: Option<String>,
    }

    fn call(a: &BuildOrderArgs) -> Result<HashMap<String, String>> {
        build_order_params(
            a.direction,
            a.pair,
            a.volume,
            a.order_type,
            &a.price,
            &a.price2,
            &a.displayvol,
            &a.trigger,
            &a.leverage,
            a.reduce_only,
            &a.timeinforce,
            &a.start_time,
            &a.expire_time,
            &a.userref,
            &a.cl_ord_id,
            &a.oflags,
            &a.stptype,
            &a.close_ordertype,
            &a.close_price,
            &a.close_price2,
            &a.deadline,
            a.validate,
            &a.asset_class,
        )
    }

    fn expect_err(a: &BuildOrderArgs, substr: &str) {
        let err = call(a).unwrap_err().to_string();
        assert!(
            err.contains(substr),
            "expected error containing {substr:?}, got: {err}"
        );
    }

    // -- valid baseline --

    #[test]
    fn valid_limit_order() {
        let a = default_params();
        let params = call(&a).unwrap();
        assert_eq!(params["ordertype"], "limit");
        assert_eq!(params["price"], "50000");
    }

    #[test]
    fn valid_market_order_no_price() {
        let mut a = default_params();
        a.order_type = "market";
        a.price = None;
        call(&a).unwrap();
    }

    // -- invalid order type --

    #[test]
    fn invalid_order_type() {
        let mut a = default_params();
        a.order_type = "bogus";
        expect_err(&a, "Invalid order type");
    }

    // -- price required for non-market --

    #[test]
    fn price_required_for_limit() {
        let mut a = default_params();
        a.price = None;
        expect_err(&a, "Price is required");
    }

    // -- iceberg requires displayvol --

    #[test]
    fn iceberg_requires_displayvol() {
        let mut a = default_params();
        a.order_type = "iceberg";
        expect_err(&a, "displayvol is required for iceberg");
    }

    #[test]
    fn iceberg_with_displayvol_ok() {
        let mut a = default_params();
        a.order_type = "iceberg";
        a.displayvol = Some("0.001".into());
        let params = call(&a).unwrap();
        assert_eq!(params["displayvol"], "0.001");
    }

    // -- price2 restricted to *-limit types --

    #[test]
    fn price2_rejected_on_limit_order() {
        let mut a = default_params();
        a.price2 = Some("60000".into());
        expect_err(&a, "price2 is only valid for");
    }

    #[test]
    fn price2_accepted_on_stop_loss_limit() {
        let mut a = default_params();
        a.order_type = "stop-loss-limit";
        a.price2 = Some("49000".into());
        let params = call(&a).unwrap();
        assert_eq!(params["price2"], "49000");
    }

    #[test]
    fn price2_accepted_on_take_profit_limit() {
        let mut a = default_params();
        a.order_type = "take-profit-limit";
        a.price2 = Some("55000".into());
        call(&a).unwrap();
    }

    #[test]
    fn price2_accepted_on_trailing_stop_limit() {
        let mut a = default_params();
        a.order_type = "trailing-stop-limit";
        a.price2 = Some("100".into());
        call(&a).unwrap();
    }

    // -- trigger type restriction --

    #[test]
    fn trigger_rejected_on_limit_order() {
        let mut a = default_params();
        a.trigger = Some("last".into());
        expect_err(&a, "trigger is only valid for");
    }

    #[test]
    fn trigger_accepted_on_stop_loss() {
        let mut a = default_params();
        a.order_type = "stop-loss";
        a.trigger = Some("last".into());
        let params = call(&a).unwrap();
        assert_eq!(params["trigger"], "last");
    }

    #[test]
    fn trigger_accepted_on_trailing_stop() {
        let mut a = default_params();
        a.order_type = "trailing-stop";
        a.trigger = Some("index".into());
        call(&a).unwrap();
    }

    // -- trigger value validation --

    #[test]
    fn trigger_invalid_value() {
        let mut a = default_params();
        a.order_type = "stop-loss";
        a.trigger = Some("las".into());
        expect_err(&a, "Invalid trigger value");
    }

    #[test]
    fn trigger_value_last_ok() {
        let mut a = default_params();
        a.order_type = "stop-loss";
        a.trigger = Some("last".into());
        call(&a).unwrap();
    }

    #[test]
    fn trigger_value_index_ok() {
        let mut a = default_params();
        a.order_type = "take-profit";
        a.trigger = Some("index".into());
        call(&a).unwrap();
    }

    // -- userref / cl_ord_id exclusivity --

    #[test]
    fn userref_and_cl_ord_id_mutually_exclusive() {
        let mut a = default_params();
        a.userref = Some("12345".into());
        a.cl_ord_id = Some("my-order".into());
        expect_err(&a, "userref and cl_ord_id are mutually exclusive");
    }

    #[test]
    fn userref_alone_ok() {
        let mut a = default_params();
        a.userref = Some("12345".into());
        let params = call(&a).unwrap();
        assert_eq!(params["userref"], "12345");
        assert!(!params.contains_key("cl_ord_id"));
    }

    #[test]
    fn cl_ord_id_alone_ok() {
        let mut a = default_params();
        a.cl_ord_id = Some("my-order".into());
        let params = call(&a).unwrap();
        assert_eq!(params["cl_ord_id"], "my-order");
        assert!(!params.contains_key("userref"));
    }

    // -- close order requires close_ordertype --

    #[test]
    fn close_price_without_close_ordertype() {
        let mut a = default_params();
        a.close_price = Some("55000".into());
        expect_err(&a, "close_ordertype is required");
    }

    #[test]
    fn close_price2_without_close_ordertype() {
        let mut a = default_params();
        a.close_price2 = Some("60000".into());
        expect_err(&a, "close_ordertype is required");
    }

    #[test]
    fn close_order_with_ordertype_ok() {
        let mut a = default_params();
        a.close_ordertype = Some("limit".into());
        a.close_price = Some("55000".into());
        let params = call(&a).unwrap();
        assert_eq!(params["close[ordertype]"], "limit");
        assert_eq!(params["close[price]"], "55000");
    }

    // -- param mapping spot checks --

    #[test]
    fn all_optional_params_mapped() {
        let mut a = default_params();
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
        let params = call(&a).unwrap();
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

    // -- amend validation --

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

    // -- batch order validation --

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

    // -- cancel batch validation --

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
}
