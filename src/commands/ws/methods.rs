//! Request method args → their typed params, fallible: the cross-field rules clap
//! cannot express (mutually-exclusive quantities, batch bounds, required identifiers)
//! are enforced here, before any socket opens.

use clap::Args;
use kraken_core::{
    AddOrderParams, AmendOrderParams, BatchAddParams, BatchCancelParams, BatchOrderParams,
    CancelAllOrdersAfterParams, CancelOrderParams, FeePreference, OrderSide, OrderType, PriceType,
    StpType, TimeInForce, TriggerReference, Triggers,
};
use rust_decimal::Decimal;

use crate::errors::{KrakenError, Result};

/// Kraken caps the dead man's switch timeout below one day.
const MAX_CANCEL_AFTER_SECS: u64 = 86_400;
/// Kraken bounds `batch_add` to 2–15 orders per request.
const BATCH_ADD_ORDERS: std::ops::RangeInclusive<usize> = 2..=15;
/// Kraken bounds `batch_cancel` to 2–50 identifiers per request.
const BATCH_CANCEL_IDS: std::ops::RangeInclusive<usize> = 2..=50;

#[derive(Debug, Args)]
pub(crate) struct AddOrder {
    /// Order type: limit, market, iceberg, stop-loss, stop-loss-limit, take-profit,
    /// take-profit-limit, trailing-stop, trailing-stop-limit, settle-position.
    #[arg(long)]
    order_type: OrderType,
    /// OrderSide: buy or sell.
    #[arg(long)]
    side: OrderSide,
    /// Order quantity in base asset (mutually exclusive with --cash-order-qty).
    #[arg(long)]
    order_qty: Option<Decimal>,
    /// Trading pair symbol (e.g. BTC/USD).
    #[arg(long)]
    symbol: String,
    /// Limit price.
    #[arg(long)]
    limit_price: Option<Decimal>,
    /// Time-in-force: gtc (default), gtd, ioc.
    #[arg(long)]
    time_in_force: Option<TimeInForce>,
    /// Fund on margin.
    #[arg(long)]
    margin: bool,
    /// Post-only order.
    #[arg(long)]
    post_only: bool,
    /// Reduce-only order.
    #[arg(long)]
    reduce_only: bool,
    /// Trigger reference price: last or index.
    #[arg(long)]
    trigger_reference: Option<TriggerReference>,
    /// Trigger price.
    #[arg(long)]
    trigger_price: Option<Decimal>,
    /// Trigger price type: static, pct, quote.
    #[arg(long)]
    trigger_price_type: Option<PriceType>,
    /// Effective time (RFC3339).
    #[arg(long)]
    effective_time: Option<String>,
    /// Expire time (RFC3339, for GTD orders).
    #[arg(long)]
    expire_time: Option<String>,
    /// Deadline (RFC3339, 500ms-60s from now).
    #[arg(long)]
    deadline: Option<String>,
    /// Client order ID.
    #[arg(long)]
    cl_ord_id: Option<String>,
    /// Numeric client order reference (signed 32-bit, per the API).
    #[arg(long)]
    order_userref: Option<i32>,
    /// Order volume in quote currency (buy market orders without margin).
    #[arg(long)]
    cash_order_qty: Option<Decimal>,
    /// Display quantity for iceberg orders.
    #[arg(long)]
    display_qty: Option<Decimal>,
    /// Fee preference: base or quote.
    #[arg(long)]
    fee_preference: Option<FeePreference>,
    /// Self-trade prevention: cancel_newest, cancel_oldest, cancel_both.
    #[arg(long)]
    stp_type: Option<StpType>,
    /// Sub-account/trader ID for granular STP (institutional accounts).
    #[arg(long)]
    sender_sub_id: Option<String>,
    /// Validate only, do not submit.
    #[arg(long)]
    validate: bool,
}

impl TryFrom<AddOrder> for AddOrderParams {
    type Error = KrakenError;

    fn try_from(args: AddOrder) -> Result<Self> {
        let AddOrder {
            order_type,
            side,
            order_qty,
            symbol,
            limit_price,
            time_in_force,
            margin,
            post_only,
            reduce_only,
            trigger_reference,
            trigger_price,
            trigger_price_type,
            effective_time,
            expire_time,
            deadline,
            cl_ord_id,
            order_userref,
            cash_order_qty,
            display_qty,
            fee_preference,
            stp_type,
            sender_sub_id,
            validate,
        } = args;
        order_qty_rule(order_qty, cash_order_qty, order_type, side)?;
        Ok(AddOrderParams {
            order_type,
            side,
            symbol,
            order_qty,
            cash_order_qty,
            limit_price,
            limit_price_type: None,
            time_in_force,
            // Wire defaults are false; send Some(true) only when the flag is
            // set so unset flags stay off the frame.
            margin: margin.then_some(true),
            post_only: post_only.then_some(true),
            reduce_only: reduce_only.then_some(true),
            // No CLI flags yet: conditional closes and no_mpp ride the --orders JSON
            // path (batch_add) or an SDK-constructed params value, not add-order args.
            no_mpp: None,
            triggers: triggers(trigger_reference, trigger_price, trigger_price_type)?,
            conditional: None,
            effective_time,
            expire_time,
            deadline,
            cl_ord_id,
            order_userref,
            display_qty,
            fee_preference,
            stp_type,
            sender_sub_id,
            validate: validate.then_some(true),
        })
    }
}

#[derive(Debug, Default, Args)]
pub(crate) struct AmendOrder {
    /// Kraken order ID (required unless --cl-ord-id is given).
    #[arg(long)]
    order_id: Option<String>,
    /// New order quantity.
    #[arg(long)]
    order_qty: Option<Decimal>,
    /// New limit price.
    #[arg(long)]
    limit_price: Option<Decimal>,
    /// New trigger price (triggered order types).
    #[arg(long)]
    trigger_price: Option<Decimal>,
    /// Reject the amend if the new limit price cannot post passively.
    #[arg(long)]
    post_only: bool,
    /// New display quantity (iceberg).
    #[arg(long)]
    display_qty: Option<Decimal>,
    /// Deadline (RFC3339).
    #[arg(long)]
    deadline: Option<String>,
    /// Client order ID (alternative to --order-id).
    #[arg(long)]
    cl_ord_id: Option<String>,
    /// Symbol (required for non-crypto pairs).
    #[arg(long)]
    symbol: Option<String>,
}

impl TryFrom<AmendOrder> for AmendOrderParams {
    type Error = KrakenError;

    fn try_from(args: AmendOrder) -> Result<Self> {
        let AmendOrder {
            order_id,
            order_qty,
            limit_price,
            trigger_price,
            post_only,
            display_qty,
            deadline,
            cl_ord_id,
            symbol,
        } = args;
        if order_id.is_none() && cl_ord_id.is_none() {
            return Err(KrakenError::Validation(
                "Either --order-id or --cl-ord-id is required for amend-order".into(),
            ));
        }
        Ok(AmendOrderParams {
            order_id,
            cl_ord_id,
            order_qty,
            display_qty,
            limit_price,
            limit_price_type: None,
            post_only: post_only.then_some(true),
            trigger_price,
            trigger_price_type: None,
            deadline,
            symbol,
        })
    }
}

#[derive(Debug, Default, Args)]
pub(crate) struct CancelOrder {
    /// Kraken order IDs to cancel.
    #[arg(long, num_args = 1..)]
    order_id: Vec<String>,
    /// Client order IDs to cancel.
    #[arg(long, num_args = 1..)]
    cl_ord_id: Vec<String>,
    /// Numeric client order refs to cancel (signed 32-bit, per the API).
    #[arg(long, num_args = 1..)]
    order_userref: Vec<i32>,
}

impl TryFrom<CancelOrder> for CancelOrderParams {
    type Error = KrakenError;

    /// Exactly one identifier kind, with at least one value — clap cannot see across
    /// the three lists, and the v2 contract forbids mixing them in one request.
    fn try_from(args: CancelOrder) -> Result<Self> {
        let CancelOrder {
            order_id,
            cl_ord_id,
            order_userref,
        } = args;
        match (
            order_id.is_empty(),
            cl_ord_id.is_empty(),
            order_userref.is_empty(),
        ) {
            (false, true, true) => Ok(CancelOrderParams::OrderIds { order_id }),
            (true, false, true) => Ok(CancelOrderParams::ClOrdIds { cl_ord_id }),
            (true, true, false) => Ok(CancelOrderParams::UserRefs { order_userref }),
            (true, true, true) => Err(KrakenError::Validation(
                "At least one --order-id, --cl-ord-id, or --order-userref is required".into(),
            )),
            _ => Err(KrakenError::Validation(
                "--order-id, --cl-ord-id, and --order-userref are mutually exclusive".into(),
            )),
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct CancelAfter {
    /// Timeout in seconds (0 to disable, must be < 86400).
    timeout: u64,
}

impl TryFrom<CancelAfter> for CancelAllOrdersAfterParams {
    type Error = KrakenError;

    fn try_from(args: CancelAfter) -> Result<Self> {
        let CancelAfter { timeout } = args;
        if timeout >= MAX_CANCEL_AFTER_SECS {
            return Err(KrakenError::Validation(format!(
                "Timeout must be < {MAX_CANCEL_AFTER_SECS} seconds"
            )));
        }
        Ok(CancelAllOrdersAfterParams { timeout })
    }
}

#[derive(Debug, Args)]
pub(crate) struct BatchAdd {
    /// Trading pair symbol (e.g. BTC/USD).
    #[arg(long)]
    symbol: String,
    /// Orders as JSON array (see Kraken WS V2 batch_add docs for schema).
    #[arg(long)]
    orders: String,
    /// Validate only, do not submit.
    #[arg(long)]
    validate: bool,
    /// Deadline (RFC3339).
    #[arg(long)]
    deadline: Option<String>,
}

impl TryFrom<BatchAdd> for BatchAddParams {
    type Error = KrakenError;

    /// Parses the `--orders` JSON array and enforces Kraken's batch bound.
    fn try_from(args: BatchAdd) -> Result<Self> {
        let BatchAdd {
            symbol,
            orders,
            validate,
            deadline,
        } = args;
        let orders: Vec<BatchOrderParams> = serde_json::from_str(&orders)
            .map_err(|e| KrakenError::Validation(format!("Invalid orders JSON: {e}")))?;
        if !BATCH_ADD_ORDERS.contains(&orders.len()) {
            return Err(KrakenError::Validation(format!(
                "Batch requires {}-{} orders, got {}",
                BATCH_ADD_ORDERS.start(),
                BATCH_ADD_ORDERS.end(),
                orders.len()
            )));
        }
        Ok(BatchAddParams {
            symbol,
            orders,
            validate: validate.then_some(true),
            deadline,
        })
    }
}

#[derive(Debug, Default, Args)]
pub(crate) struct BatchCancel {
    /// Order IDs or user refs to cancel (required).
    #[arg(long, num_args = 1.., required = true)]
    orders: Vec<String>,
    /// Additional client order IDs to cancel.
    #[arg(long, num_args = 1..)]
    cl_ord_id: Vec<String>,
}

impl TryFrom<BatchCancel> for BatchCancelParams {
    type Error = KrakenError;

    /// The order-id and client-order-id lists count against one shared bound.
    fn try_from(args: BatchCancel) -> Result<Self> {
        let BatchCancel { orders, cl_ord_id } = args;
        let total = orders.len() + cl_ord_id.len();
        if !BATCH_CANCEL_IDS.contains(&total) {
            return Err(KrakenError::Validation(format!(
                "Batch cancel requires {}-{} identifiers, got {total}",
                BATCH_CANCEL_IDS.start(),
                BATCH_CANCEL_IDS.end(),
            )));
        }
        Ok(BatchCancelParams { orders, cl_ord_id })
    }
}

/// Cross-field quantity rules clap cannot express: exactly one of `--order-qty` /
/// `--cash-order-qty`, and the cash form only on market buys (the only shape Kraken
/// accepts it for).
fn order_qty_rule(
    order_qty: Option<Decimal>,
    cash_order_qty: Option<Decimal>,
    order_type: OrderType,
    side: OrderSide,
) -> Result<()> {
    match (order_qty, cash_order_qty) {
        (Some(_), Some(_)) => Err(KrakenError::Validation(
            "--order-qty and --cash-order-qty are mutually exclusive".into(),
        )),
        (None, None) => Err(KrakenError::Validation(
            "Either --order-qty or --cash-order-qty is required".into(),
        )),
        (None, Some(_)) if order_type != OrderType::Market || side != OrderSide::Buy => {
            Err(KrakenError::Validation(
                "--cash-order-qty is only available for market buy orders".into(),
            ))
        }
        _ => Ok(()),
    }
}

/// Fold the three trigger flags into one wire object, or `None` when none was given so
/// the frame carries no `triggers` key at all. `--trigger-price` anchors the group: the
/// other two only qualify it, so they are rejected without it rather than shipping a
/// priceless trigger the server will bounce.
fn triggers(
    reference: Option<TriggerReference>,
    price: Option<Decimal>,
    price_type: Option<PriceType>,
) -> Result<Option<Triggers>> {
    match price {
        Some(price) => Ok(Some(Triggers {
            reference,
            price,
            price_type,
        })),
        None if reference.is_some() || price_type.is_some() => Err(KrakenError::Validation(
            "--trigger-reference/--trigger-price-type require --trigger-price".into(),
        )),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use super::*;
    use crate::commands::ws::{MethodCommand, WsCommand};

    /// Assert the conversion result is a validation error whose message contains `needle`.
    fn expect_validation_err<T: std::fmt::Debug>(result: Result<T>, needle: &str) {
        match result {
            Err(KrakenError::Validation(msg)) => assert!(
                msg.contains(needle),
                "expected validation error containing {needle:?}, got {msg:?}"
            ),
            other => panic!("expected validation error containing {needle:?}, got {other:?}"),
        }
    }

    /// Parse a `kraken ws ...` argv and return the parsed `Ws` variant's pieces.
    fn parse_ws(
        argv: &[&str],
    ) -> std::result::Result<(Option<NonZeroU64>, WsCommand), clap::Error> {
        use clap::Parser;
        let mut full = vec!["kraken", "ws"];
        full.extend_from_slice(argv);
        crate::Cli::try_parse_from(full).map(|cli| match cli.command {
            Some(crate::commands::Command::Streaming(
                crate::commands::streaming::StreamingCommand::Ws(ws),
            )) => (ws.req_id, ws.cmd),
            _ => panic!("argv did not parse to a ws subcommand"),
        })
    }

    /// Parse a minimal `ws add-order` argv plus `extra` and return the parsed
    /// `validate` flag. Pins the `--validate` flag shape: a bare flag, matching
    /// the REST `order --validate`. The wire default is false, so a valued form
    /// (`--validate true`) would add no behaviour and must be rejected.
    fn add_order_validate_of(extra: &[&str]) -> std::result::Result<bool, clap::Error> {
        let mut argv = vec![
            "add-order",
            "--side",
            "buy",
            "--symbol",
            "BTC/USD",
            "--order-type",
            "limit",
        ];
        argv.extend_from_slice(extra);
        parse_ws(&argv).map(|(_, cmd)| match cmd {
            WsCommand::Method(MethodCommand::AddOrder(args)) => args.validate,
            _ => panic!("argv did not parse to ws add-order"),
        })
    }

    #[test]
    fn add_order_bare_validate_flag_parses_true() {
        assert!(add_order_validate_of(&["--validate"]).expect("bare flag must parse"));
    }

    #[test]
    fn add_order_absent_validate_flag_parses_false() {
        assert!(!add_order_validate_of(&[]).expect("absent flag must parse"));
    }

    #[test]
    fn add_order_valued_validate_flag_is_rejected() {
        assert!(add_order_validate_of(&["--validate", "true"]).is_err());
    }

    #[test]
    fn req_id_is_one_global_flag_on_every_subcommand() {
        // `--req-id` is declared once on `ws` and propagates to the subcommands, in
        // either position.
        let (req_id, _) = parse_ws(&["ping", "--req-id", "7"]).expect("global flag must parse");
        assert_eq!(req_id, NonZeroU64::new(7));
        let (req_id, _) = parse_ws(&["--req-id", "9", "ping"]).expect("pre-subcommand position");
        assert_eq!(req_id, NonZeroU64::new(9));
    }

    #[test]
    fn req_id_zero_is_a_parse_error() {
        // `NonZeroU64` makes 0 unrepresentable, so the reservation is enforced by clap at
        // the boundary instead of a runtime check.
        assert!(parse_ws(&["ping", "--req-id", "0"]).is_err());
    }

    #[test]
    fn order_qty_rule_requires_exactly_one_quantity() {
        let (qty, cash) = (Decimal::ONE, Decimal::from(100));
        expect_validation_err(
            order_qty_rule(Some(qty), Some(cash), OrderType::Limit, OrderSide::Buy),
            "mutually exclusive",
        );
        expect_validation_err(
            order_qty_rule(None, None, OrderType::Limit, OrderSide::Buy),
            "required",
        );
        assert!(order_qty_rule(Some(qty), None, OrderType::Limit, OrderSide::Sell).is_ok());
    }

    #[test]
    fn cash_order_qty_is_market_buy_only() {
        let cash = Decimal::from(100);
        expect_validation_err(
            order_qty_rule(None, Some(cash), OrderType::Limit, OrderSide::Buy),
            "market buy",
        );
        expect_validation_err(
            order_qty_rule(None, Some(cash), OrderType::Market, OrderSide::Sell),
            "market buy",
        );
        assert!(order_qty_rule(None, Some(cash), OrderType::Market, OrderSide::Buy).is_ok());
    }

    #[test]
    fn triggers_fold_to_none_when_no_flag_is_given() {
        assert_eq!(triggers(None, None, None).expect("no flags is valid"), None);
        let price = Decimal::from(50_000);
        let some = triggers(None, Some(price), None)
            .expect("a price alone is a valid trigger")
            .expect("a price alone builds triggers");
        assert_eq!(some.price, price);
    }

    #[test]
    fn trigger_qualifiers_without_a_price_are_rejected() {
        expect_validation_err(
            triggers(Some(TriggerReference::Last), None, None).map(|_| ()),
            "--trigger-price",
        );
    }

    #[test]
    fn amend_order_requires_an_identifier() {
        expect_validation_err(
            AmendOrderParams::try_from(AmendOrder::default()),
            "--order-id or --cl-ord-id",
        );
    }

    #[test]
    fn cancel_after_rejects_timeout_at_or_above_one_day() {
        expect_validation_err(
            CancelAllOrdersAfterParams::try_from(CancelAfter { timeout: 86_400 }),
            "86400",
        );
    }

    #[test]
    fn cancel_after_converts_below_the_limit() {
        let params = CancelAllOrdersAfterParams::try_from(CancelAfter { timeout: 60 })
            .expect("60s is in range");
        assert_eq!(params, CancelAllOrdersAfterParams { timeout: 60 });
    }

    #[test]
    fn cancel_order_requires_at_least_one_identifier() {
        expect_validation_err(
            CancelOrderParams::try_from(CancelOrder::default()),
            "At least one",
        );
    }

    #[test]
    fn batch_cancel_requires_two_to_fifty_identifiers() {
        expect_validation_err(
            BatchCancelParams::try_from(BatchCancel {
                orders: vec!["OID-1".into()],
                cl_ord_id: vec![],
            }),
            "2-50",
        );
    }

    #[test]
    fn batch_add_requires_two_to_fifteen_orders() {
        let one_order = BatchAdd {
            symbol: "BTC/USD".into(),
            orders: r#"[{"order_type":"limit","side":"buy","order_qty":1,"limit_price":1}]"#.into(),
            validate: false,
            deadline: None,
        };
        expect_validation_err(BatchAddParams::try_from(one_order), "2-15");
    }

    #[test]
    fn batch_add_rejects_malformed_orders_json() {
        let bad_json = BatchAdd {
            symbol: "BTC/USD".into(),
            orders: "not json".into(),
            validate: false,
            deadline: None,
        };
        expect_validation_err(BatchAddParams::try_from(bad_json), "Invalid orders JSON");
    }
}
