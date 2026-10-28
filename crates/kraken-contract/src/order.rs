//! Spot trading: the `order` command family.

use clap::Subcommand;
use std::path::PathBuf;

/// The `order` family: placing and managing spot orders. Mode-routed — the
/// same verbs reach the simulator, a live workspace's ledger, or the venue.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OrderCommand {
    /// Place a buy order.
    ///
    /// Routed by workspace mode: fills on the paper journal inside a paper
    /// workspace, trades the real account otherwise. Inside a live
    /// workspace, an order spending beyond what the allocation ledger holds
    /// free of resting orders is refused before the venue (a validation
    /// envelope); the order that pays the allocated asset back in is never
    /// refused.
    Buy(Buy),
    /// Place a sell order.
    ///
    /// Routed by workspace mode: fills on the paper journal inside a paper
    /// workspace, trades the real account otherwise. Inside a live
    /// workspace, an order spending beyond what the allocation ledger holds
    /// free of resting orders is refused before the venue (a validation
    /// envelope); the order that pays the allocated asset back in is never
    /// refused.
    Sell(Sell),
    /// Submit batch orders from a JSON file (2-15 orders).
    Batch(Batch),
    /// Amend a live order in-place (preserves queue priority and identifiers).
    Amend(Amend),
    /// Edit (cancel+replace) an order.
    Edit(Edit),
    /// Cancel an open order by txid, userref, or client order ID.
    ///
    /// Routed by workspace mode: inside a paper workspace it cancels on the
    /// workspace journal (a single txid only). Against the real account it
    /// sends one CancelOrder per transaction ID so each order's outcome is
    /// known — a sweep that only partly succeeds exits 1 with an `api`
    /// envelope carrying `cancelled` and `failed` lists,
    /// and only venue-confirmed cancels are recorded locally. Inside a live
    /// workspace it reconciles first and cancels only the order IDs that
    /// workspace recorded, never the account-wide CancelAll.
    Cancel(Cancel),
    /// Cancel a batch of orders (max 50 total across txids and cl-ord-ids).
    CancelBatch(CancelBatch),
    /// Cancel all open orders.
    ///
    /// Routed by workspace mode: inside a paper workspace it cancels on the
    /// workspace journal. Against the real account it sends Kraken's
    /// account-wide CancelAll and returns the `count` Kraken reports, then
    /// records every open order the local journal knows as cancelled. Inside
    /// a live workspace it reconciles first and cancels only the order IDs
    /// that workspace recorded, one CancelOrder each, never the account-wide
    /// CancelAll; a sweep that only partly succeeds exits 1 with an `api`
    /// envelope carrying `cancelled` and `failed` lists.
    CancelAll(CancelAll),
    /// Dead man's switch — cancel all orders after timeout.
    CancelAfter(CancelAfter),
}

// Buy and sell take the same arguments; only the order side differs. The
// executing crate implements its work trait per variant type, so each side is a
// distinct thin wrapper flattening the shared set rather than a field on one.
#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Buy {
    #[command(flatten)]
    #[serde(flatten)]
    pub order: OrderRequest,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Sell {
    #[command(flatten)]
    #[serde(flatten)]
    pub order: OrderRequest,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct OrderRequest {
    /// Trading pair (e.g. XBTUSD).
    pub pair: String,
    /// Order volume.
    pub volume: String,
    /// Order type (market, limit, iceberg, stop-loss, take-profit,
    /// stop-loss-limit, take-profit-limit, trailing-stop,
    /// trailing-stop-limit, settle-position).
    #[arg(long, default_value = "limit")]
    #[serde(default = "limit_order")]
    pub r#type: String,
    /// Price (required for non-market orders).
    #[arg(long)]
    pub price: Option<String>,
    /// Limit price for stop-loss-limit, take-profit-limit, and
    /// trailing-stop-limit orders.
    #[arg(long)]
    pub price2: Option<String>,
    /// Display volume for iceberg orders (visible portion in the order book).
    #[arg(long)]
    pub displayvol: Option<String>,
    /// Price signal for triggered orders (last or index). Applies to stop-loss,
    /// stop-loss-limit, take-profit, take-profit-limit, trailing-stop,
    /// trailing-stop-limit.
    #[arg(long)]
    pub trigger: Option<String>,
    /// Leverage.
    #[arg(long)]
    pub leverage: Option<String>,
    /// Reduce-only flag.
    #[arg(long)]
    #[serde(default)]
    pub reduce_only: bool,
    /// Time-in-force (GTC, IOC, GTD).
    #[arg(long)]
    pub timeinforce: Option<String>,
    /// Start time.
    #[arg(long)]
    pub start_time: Option<String>,
    /// Expire time.
    #[arg(long)]
    pub expire_time: Option<String>,
    /// User reference ID (signed 32-bit, for tagging groups of orders).
    #[arg(long)]
    pub userref: Option<i32>,
    /// Client order ID (mutually exclusive with userref).
    #[arg(long)]
    pub cl_ord_id: Option<String>,
    /// Order flags (comma-delimited: post, fcib, fciq, nompp, viqc).
    #[arg(long)]
    pub oflags: Option<String>,
    /// Self-trade prevention type (cancel-newest, cancel-oldest, cancel-both).
    #[arg(long)]
    pub stptype: Option<String>,
    /// Conditional close order type (limit, stop-loss, take-profit,
    /// stop-loss-limit, take-profit-limit).
    #[arg(long)]
    pub close_ordertype: Option<String>,
    /// Conditional close order price.
    #[arg(long)]
    pub close_price: Option<String>,
    /// Conditional close order secondary price.
    #[arg(long)]
    pub close_price2: Option<String>,
    /// RFC3339 deadline for the order to reach the matching engine.
    #[arg(long)]
    pub deadline: Option<String>,
    /// Validate only (do not submit).
    #[arg(long)]
    #[serde(default)]
    pub validate: bool,
    /// Asset class (required for non-crypto pairs, e.g. tokenized_asset for
    /// xstocks).
    #[arg(long)]
    pub asset_class: Option<String>,
    /// Rationale to record in the decision log: the active session's when one
    /// is recording, else the workspace's, or the real account's outside one.
    #[arg(long)]
    pub reason: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Batch {
    /// Path to JSON file containing order array.
    pub json_file: PathBuf,
    /// Asset pair (overrides pair from JSON; required if not in JSON).
    #[arg(long)]
    pub pair: Option<String>,
    /// Asset class (required for non-crypto pairs, e.g. tokenized_asset for
    /// xstocks).
    #[arg(long)]
    pub asset_class: Option<String>,
    /// RFC3339 deadline for the batch to reach the matching engine.
    #[arg(long)]
    pub deadline: Option<String>,
    /// Validate only (do not submit).
    #[arg(long)]
    #[serde(default)]
    pub validate: bool,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Amend {
    /// Kraken transaction ID of the order to amend (either txid or --cl-ord-id
    /// required).
    #[arg(long)]
    pub txid: Option<String>,
    /// Client order ID of the order to amend (either --txid or --cl-ord-id
    /// required).
    #[arg(long)]
    pub cl_ord_id: Option<String>,
    /// New order quantity in base asset.
    #[arg(long)]
    pub order_qty: Option<String>,
    /// New display quantity for iceberg orders (min 1/15 of remaining
    /// quantity).
    #[arg(long)]
    pub display_qty: Option<String>,
    /// New limit price (supports +/- relative and % suffix).
    #[arg(long)]
    pub limit_price: Option<String>,
    /// New trigger price for triggered order types (supports +/- relative and %
    /// suffix).
    #[arg(long)]
    pub trigger_price: Option<String>,
    /// Trading pair (required for non-crypto pairs, e.g. xstocks).
    #[arg(long)]
    pub pair: Option<String>,
    /// Reject amend if limit price change would cause immediate match.
    #[arg(long)]
    #[serde(default)]
    pub post_only: bool,
    /// RFC3339 deadline (min now()+2s, max now()+60s).
    #[arg(long)]
    pub deadline: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Edit {
    /// Transaction ID of the order to edit.
    pub txid: String,
    /// New volume.
    #[arg(long)]
    pub volume: Option<String>,
    /// New price.
    #[arg(long)]
    pub price: Option<String>,
    /// Trading pair.
    #[arg(long)]
    pub pair: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Cancel {
    /// Transaction ID(s) or userref (comma-separated for multiple).
    #[arg(num_args = 1..)]
    #[serde(default)]
    pub txids: Vec<String>,
    /// Cancel by client order ID instead of txid.
    #[arg(long)]
    pub cl_ord_id: Option<String>,
    /// Rationale to record in the decision log: the active session's when one
    /// is recording, else the workspace's, or the real account's outside one.
    #[arg(long)]
    pub reason: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CancelBatch {
    /// Transaction IDs or user references (up to 50).
    #[arg(num_args = 1..)]
    #[serde(default)]
    pub txids: Vec<String>,
    /// Client order IDs to cancel (up to 50 total combined with txids).
    #[arg(long, num_args = 1..)]
    #[serde(default)]
    pub cl_ord_ids: Vec<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CancelAll {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CancelAfter {
    /// Timeout in seconds (0 to disable).
    pub timeout: u64,
}

/// The order type Kraken assumes when the caller names none.
fn limit_order() -> String {
    "limit".to_owned()
}

crate::tools! {
    /// The `kraken order` commands.
    fn order in ["order"] of OrderCommand {
        Buy         as "buy"          => Trade, PaperSafe, Spot, Tool
            example "kraken order buy BTC/USD 0.001 --type limit --price 50000 --validate";
        Sell        as "sell"         => Trade, PaperSafe, Spot, Tool
            example "kraken order sell BTC/USD 0.001 --type limit --price 100000 --validate";
        Batch       as "batch"        => Trade, Dangerous, Spot, Tool
            example "kraken order batch orders.json --validate";
        Amend       as "amend"        => Trade, Dangerous, Spot, Tool
            example "kraken order amend --txid OABC12-DEFG3-HIJKL4 --limit-price 51000";
        Edit        as "edit"         => Trade, Dangerous, Spot, Tool
            example "kraken order edit OABC12-DEFG3-HIJKL4 --price 52000";
        Cancel      as "cancel"       => Trade, PaperSafe, Spot, Tool
            example "kraken order cancel OABC12-DEFG3-HIJKL4";
        CancelBatch as "cancel-batch" => Trade, Dangerous, Spot, Tool
            example "kraken order cancel-batch OABC12-DEFG3-HIJKL4 OEFGH56-IJKL7-MNOP8";
        CancelAll   as "cancel-all"   => Trade, PaperSafe, Spot, Tool
            example "kraken order cancel-all --yes";
        CancelAfter as "cancel-after" => Trade, Dangerous, Spot, Tool
            example "kraken order cancel-after 60";
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A root to parse against — the real one lives in the executing crate.
    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: OrderCommand,
    }

    fn parse(args: &[&str]) -> OrderCommand {
        let argv = std::iter::once("kraken").chain(args.iter().copied());
        Cli::try_parse_from(argv).expect("fixture parses").command
    }

    #[test]
    fn a_parsed_order_survives_the_round_trip() {
        let wire = serde_json::to_string(&parse(&[
            "buy",
            "BTCUSD",
            "0.5",
            "--type",
            "limit",
            "--price",
            "50000",
            "--userref",
            "42",
        ]))
        .expect("serialize");

        let decoded: OrderCommand = serde_json::from_str(&wire).expect("a proposal must decode");
        assert_eq!(
            serde_json::to_string(&decoded).expect("re-serialize"),
            wire,
            "the value decoded must be the value the CLI proposed"
        );
    }

    /// A shared arg group is flattened on both layers, so one declaration is
    /// one shape: the JSON keys are the CLI flags. Pinned because the tool
    /// surface derives its schema from this shape — re-nesting
    /// `OrderRequest` under an `order` key would move every order field one
    /// level down for every client.
    #[test]
    fn a_flattened_arg_group_is_flat_on_the_wire() {
        let wire = serde_json::to_value(parse(&["buy", "BTCUSD", "0.5"])).expect("serialize");
        assert_eq!(wire["buy"]["pair"], "BTCUSD");
        assert_eq!(wire["buy"]["volume"], "0.5");
        assert!(
            wire["buy"]["order"].is_null(),
            "the group name must not survive as a key: {wire}"
        );
    }

    /// An unknown command must fail to decode rather than arrive
    /// half-understood — a gate that cannot read a proposal has to deny it,
    /// and that only works if `serde` refuses instead of yielding a partial
    /// value.
    #[test]
    fn an_unknown_command_fails_to_decode() {
        serde_json::from_str::<OrderCommand>(r#"{"teleport":{"pair":"BTCUSD"}}"#)
            .expect_err("an unknown variant must not decode");
    }
}
