//! Pins every REST command's published output — JSON payload and table
//! projection — against captured venue payloads, so a change to how the REST
//! layer decodes can never reshape what callers read unnoticed.
//!
//! Data-driven: a case is an argv plus the venue routes it hits. Adding
//! coverage is adding a row.

use super::{Command, test_support::venue_ctx};
use crate::cli::Cli;
use clap::Parser;
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{path, query_param},
};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../kraken-rest/fixtures");

/// A venue route: requests to `path` (carrying `query`, when set) answer with
/// the fixture file at `fixture`, relative to [`FIXTURES`].
#[derive(Clone, Copy)]
struct Route {
    path: &'static str,
    query: Option<(&'static str, &'static str)>,
    fixture: &'static str,
}

const fn at(path: &'static str, fixture: &'static str) -> Route {
    Route {
        path,
        query: None,
        fixture,
    }
}

const fn at_query(
    path: &'static str,
    query: (&'static str, &'static str),
    fixture: &'static str,
) -> Route {
    Route {
        path,
        query: Some(query),
        fixture,
    }
}

struct Case {
    name: &'static str,
    argv: &'static [&'static str],
    answers: Answers,
}

/// The venue endpoints one case hits, each answered by its fixture.
enum Answers {
    Public(&'static [Route]),
    /// A private endpoint, by name.
    Private(&'static str),
    /// A Futures trading route and its fixture under `futures/`.
    Futures {
        path: &'static str,
        fixture: &'static str,
    },
    /// A Futures history endpoint, named without its API prefix, and its
    /// fixture under `futures/`.
    History {
        endpoint: &'static str,
        fixture: &'static str,
    },
}

/// The two catalogs `ticker` reads to classify each pair before fetching.
const TICKER_CATALOGS: [Route; 2] = [
    at_query(
        "/0/public/AssetPairs",
        ("aclass", "tokenized_asset"),
        "spot/AssetPairs.tokenized.json",
    ),
    at_query(
        "/0/public/AssetPairs",
        ("aclass", "forex"),
        "spot/AssetPairs.forex.json",
    ),
];

const CASES: &[Case] = &[
    public(
        "status",
        &["status"],
        &[at("/0/public/SystemStatus", "spot/SystemStatus.json")],
    ),
    public(
        "server_time",
        &["server-time"],
        &[at("/0/public/Time", "spot/Time.json")],
    ),
    public(
        "assets",
        &["assets"],
        &[at("/0/public/Assets", "spot/Assets.json")],
    ),
    public(
        "pairs",
        &["pairs"],
        &[at("/0/public/AssetPairs", "spot/AssetPairs.json")],
    ),
    public(
        "pairs_tokenized",
        &["pairs", "--asset-class", "tokenized_asset"],
        &[at("/0/public/AssetPairs", "spot/AssetPairs.tokenized.json")],
    ),
    public(
        "ticker",
        &["ticker", "XBTUSD", "ETHUSD"],
        &[
            TICKER_CATALOGS[0],
            TICKER_CATALOGS[1],
            at("/0/public/Ticker", "spot/Ticker.json"),
        ],
    ),
    public(
        "ticker_tokenized",
        &["ticker", "AAPLx/USD", "--asset-class", "tokenized_asset"],
        &[at("/0/public/Ticker", "spot/Ticker.tokenized.json")],
    ),
    public(
        "ticker_forex",
        &["ticker", "EURUSD", "--asset-class", "forex"],
        &[at("/0/public/Ticker", "spot/Ticker.forex.json")],
    ),
    public(
        "ohlc",
        &["ohlc", "XBTUSD", "--interval", "60"],
        &[at("/0/public/OHLC", "spot/OHLC.json")],
    ),
    public(
        "orderbook",
        &["orderbook", "XBTUSD", "--count", "3"],
        &[at("/0/public/Depth", "spot/Depth.json")],
    ),
    public(
        "orderbook_grouped",
        &["orderbook-grouped", "XBTUSD"],
        &[at("/0/public/GroupedBook", "spot/GroupedBook.json")],
    ),
    public(
        "trades",
        &["trades", "XBTUSD", "--count", "3"],
        &[at("/0/public/Trades", "spot/Trades.json")],
    ),
    public(
        "spreads",
        &["spreads", "XBTUSD"],
        &[at("/0/public/Spread", "spot/Spread.json")],
    ),
    // Account reads.
    private("balance", &["balance"], "Balance"),
    private("extended_balance", &["extended-balance"], "BalanceEx"),
    private(
        "orderbook_l3",
        &["orderbook-l3", "BTCUSD", "--depth", "100"],
        "Level3",
    ),
    private("credit_lines", &["credit-lines"], "CreditLines"),
    private(
        "trade_balance",
        &["trade-balance", "--asset", "ZUSD"],
        "TradeBalance",
    ),
    private("open_orders", &["open-orders"], "OpenOrders"),
    private("closed_orders", &["closed-orders"], "ClosedOrders"),
    private(
        "query_orders",
        &["query-orders", "OABC12-DEFG3-HIJKL4"],
        "QueryOrders",
    ),
    private("trades_history", &["trades-history"], "TradesHistory"),
    private(
        "query_trades",
        &["query-trades", "TABC12-DEFG3-HIJKL4"],
        "QueryTrades",
    ),
    private("positions", &["positions"], "OpenPositions"),
    private("ledgers", &["ledgers"], "Ledgers"),
    private(
        "query_ledgers",
        &["query-ledgers", "LABC12-DEFG3-HIJKL4"],
        "QueryLedgers",
    ),
    private("volume", &["volume", "--pair", "BTCUSD"], "TradeVolume"),
    private(
        "export_report",
        &[
            "export-report",
            "--report",
            "trades",
            "--description",
            "Q1 trades",
        ],
        "AddExport",
    ),
    private(
        "export_status",
        &["export-status", "--report", "trades"],
        "ExportStatus",
    ),
    private("export_delete", &["export-delete", "ABCD"], "RemoveExport"),
    // Trading that writes no local journal: validate-only placement and the
    // in-place modifications.
    private(
        "order_buy_validate",
        &[
            "order",
            "buy",
            "BTCUSD",
            "0.001",
            "--type",
            "limit",
            "--price",
            "50000",
            "--validate",
        ],
        "AddOrder",
    ),
    private(
        "order_amend",
        &[
            "order",
            "amend",
            "--txid",
            "OABC12-DEFG3-HIJKL4",
            "--limit-price",
            "51000",
        ],
        "AmendOrder",
    ),
    private(
        "order_edit",
        &["order", "edit", "OABC12-DEFG3-HIJKL4", "--price", "52000"],
        "EditOrder",
    ),
    private(
        "order_cancel_after",
        &["order", "cancel-after", "60"],
        "CancelAllOrdersAfter",
    ),
    // Funding.
    private(
        "deposit_methods",
        &["deposit", "methods", "BTC"],
        "DepositMethods",
    ),
    private(
        "deposit_addresses",
        &["deposit", "addresses", "BTC", "Bitcoin"],
        "DepositAddresses",
    ),
    private(
        "deposit_status",
        &["deposit", "status", "--asset", "BTC"],
        "DepositStatus",
    ),
    private(
        "withdrawal_methods",
        &["withdrawal", "methods", "--asset", "BTC"],
        "WithdrawMethods",
    ),
    private(
        "withdrawal_addresses",
        &["withdrawal", "addresses", "--asset", "ETH"],
        "WithdrawAddresses",
    ),
    private(
        "withdrawal_info",
        &["withdrawal", "info", "BTC", "my-btc-wallet", "0.5"],
        "WithdrawInfo",
    ),
    private(
        "withdrawal_status",
        &["withdrawal", "status", "--asset", "BTC"],
        "WithdrawStatus",
    ),
    private(
        "withdrawal_cancel",
        &["withdrawal", "cancel", "BTC", "AGBSO6T-WGH4X-CQXB5J"],
        "WithdrawCancel",
    ),
    private(
        "withdraw",
        &["withdraw", "BTC", "my-btc-wallet", "0.1"],
        "Withdraw",
    ),
    private(
        "wallet_transfer",
        &[
            "wallet-transfer",
            "USD",
            "1000",
            "--from",
            "Spot",
            "--to",
            "Futures",
        ],
        "WalletTransfer",
    ),
    // Earn.
    private(
        "earn_strategies",
        &["earn", "strategies", "--asset", "ETH"],
        "Earn/Strategies",
    ),
    private(
        "earn_allocations",
        &["earn", "allocations"],
        "Earn/Allocations",
    ),
    private(
        "earn_allocate",
        &["earn", "allocate", "ESXUM7H-SJHQ6-KOBAL7", "1.5"],
        "Earn/Allocate",
    ),
    private(
        "earn_deallocate",
        &["earn", "deallocate", "ESXUM7H-SJHQ6-KOBAL7", "0.5"],
        "Earn/Deallocate",
    ),
    private(
        "earn_allocate_status",
        &["earn", "allocate-status", "ESXUM7H-SJHQ6-KOBAL7"],
        "Earn/AllocateStatus",
    ),
    private(
        "earn_deallocate_status",
        &["earn", "deallocate-status", "ESXUM7H-SJHQ6-KOBAL7"],
        "Earn/DeallocateStatus",
    ),
    // Subaccounts.
    private(
        "subaccount_create",
        &["subaccount", "create", "trading-bot", "bot@example.com"],
        "CreateSubaccount",
    ),
    private(
        "subaccount_transfer",
        &[
            "subaccount",
            "transfer",
            "USD",
            "5000",
            "--from",
            "main",
            "--to",
            "trading-bot",
        ],
        "AccountTransfer",
    ),
    // Futures: trading API at the base URL, history API under it at
    // `/api/history/v3`.
    futures(
        "futures_instruments",
        &["futures", "instruments"],
        "/instruments",
        "trading.get_instruments.json",
    ),
    futures(
        "futures_tickers",
        &["futures", "tickers"],
        "/tickers",
        "trading.get_tickers.json",
    ),
    futures(
        "futures_ticker",
        &["futures", "ticker", "PF_XBTUSD"],
        "/tickers/PF_XBTUSD",
        "trading.get_tickers_symbol.json",
    ),
    futures(
        "futures_orderbook",
        &["futures", "orderbook", "PF_XBTUSD"],
        "/orderbook",
        "trading.get_orderbook.json",
    ),
    futures(
        "futures_history",
        &["futures", "history", "PF_XBTUSD"],
        "/history",
        "trading.get_history.json",
    ),
    futures(
        "futures_feeschedules",
        &["futures", "feeschedules"],
        "/feeschedules",
        "trading.get_feeschedules.json",
    ),
    futures(
        "futures_instrument_status",
        &["futures", "instrument-status"],
        "/instruments/status",
        "trading.get_instruments_status.json",
    ),
    futures(
        "futures_instrument_status_symbol",
        &["futures", "instrument-status", "--symbol", "PF_XBTUSD"],
        "/instruments/PF_XBTUSD/status",
        "trading.get_instruments_symbol_status.json",
    ),
    futures(
        "futures_funding_rates",
        &["futures", "historical-funding-rates", "PI_XBTUSD"],
        "/historical-funding-rates",
        "trading.get_historical-funding-rates.json",
    ),
    futures(
        "futures_trading_instruments",
        &["futures", "trading-instruments"],
        "/trading/instruments",
        "trading.get_trading_instruments.synthetic.json",
    ),
    futures(
        "futures_accounts",
        &["futures", "accounts"],
        "/accounts",
        "trading.get_accounts.json",
    ),
    futures(
        "futures_open_orders",
        &["futures", "open-orders"],
        "/openorders",
        "trading.get_openorders.json",
    ),
    futures(
        "futures_positions",
        &["futures", "positions"],
        "/openpositions",
        "trading.get_openpositions.json",
    ),
    futures(
        "futures_fills",
        &["futures", "fills"],
        "/fills",
        "trading.get_fills.json",
    ),
    futures(
        "futures_leverage",
        &["futures", "leverage"],
        "/leveragepreferences",
        "trading.get_leveragepreferences.json",
    ),
    futures(
        "futures_set_leverage",
        &["futures", "set-leverage", "PF_XBTUSD", "5"],
        "/leveragepreferences",
        "trading.put_leveragepreferences.json",
    ),
    futures(
        "futures_pnl_preferences",
        &["futures", "pnl-preferences"],
        "/pnlpreferences",
        "trading.get_pnlpreferences.json",
    ),
    futures(
        "futures_set_pnl_preference",
        &["futures", "set-pnl-preference", "PF_XBTUSD", "USD"],
        "/pnlpreferences",
        "trading.put_pnlpreferences.json",
    ),
    futures(
        "futures_notifications",
        &["futures", "notifications"],
        "/notifications",
        "trading.get_notifications.json",
    ),
    futures(
        "futures_transfers",
        &["futures", "transfers"],
        "/transfers",
        "trading.get_transfers.synthetic.json",
    ),
    futures(
        "futures_unwind_queue",
        &["futures", "unwind-queue"],
        "/unwindqueue",
        "trading.get_unwindqueue.json",
    ),
    futures(
        "futures_assignment_programs",
        &["futures", "assignment-programs"],
        "/assignmentprogram/current",
        "trading.get_assignmentprogram_current.synthetic.json",
    ),
    futures(
        "futures_fee_schedule_volumes",
        &["futures", "fee-schedule-volumes"],
        "/feeschedules/volumes",
        "trading.get_feeschedules_volumes.json",
    ),
    futures(
        "futures_subaccounts",
        &["futures", "subaccounts"],
        "/subaccounts",
        "trading.get_subaccounts.json",
    ),
    futures(
        "futures_subaccount_status",
        &["futures", "subaccount-status", "abc123"],
        "/subaccount/abc123/trading-enabled",
        "trading.get_subaccount_trading-enabled.synthetic.json",
    ),
    futures(
        "futures_set_subaccount_status",
        &["futures", "set-subaccount-status", "abc123", "false"],
        "/subaccount/abc123/trading-enabled",
        "trading.put_subaccount_trading-enabled.synthetic.json",
    ),
    futures(
        "futures_order_status",
        &[
            "futures",
            "order-status",
            "2ce038ae-c144-4de7-a0f1-82f7f4fca864",
        ],
        "/orders/status",
        "trading.post_orders_status.synthetic.json",
    ),
    futures(
        "futures_order_buy",
        &[
            "futures",
            "order",
            "buy",
            "PF_XBTUSD",
            "1",
            "--type",
            "limit",
            "--price",
            "50000",
        ],
        "/sendorder",
        "trading.post_sendorder.json",
    ),
    futures(
        "futures_edit_order",
        &[
            "futures",
            "edit-order",
            "--order-id",
            "abc123",
            "--price",
            "52000",
        ],
        "/editorder",
        "trading.post_editorder.json",
    ),
    futures(
        "futures_cancel",
        &["futures", "cancel", "--order-id", "abc123"],
        "/cancelorder",
        "trading.post_cancelorder.json",
    ),
    futures(
        "futures_cancel_all",
        &["futures", "cancel-all", "--yes"],
        "/cancelallorders",
        "trading.post_cancelallorders.json",
    ),
    futures(
        "futures_cancel_after",
        &["futures", "cancel-after", "120"],
        "/cancelallordersafter",
        "trading.post_cancelallordersafter.json",
    ),
    futures(
        "futures_batch_order",
        &[
            "futures",
            "batch-order",
            r#"{"batchOrder":[{"order":"send","order_tag":"1","orderType":"lmt","symbol":"PF_XBTUSD","side":"buy","size":1,"limitPrice":50000}]}"#,
        ],
        "/batchorder",
        "trading.post_batchorder.json",
    ),
    futures(
        "futures_transfer",
        &["futures", "transfer", "1000", "USD"],
        "/withdrawal",
        "trading.post_withdrawal.json",
    ),
    futures(
        "futures_wallet_transfer",
        &["futures", "wallet-transfer", "main", "flex", "USD", "100"],
        "/transfer",
        "trading.post_transfer.synthetic.json",
    ),
    futures_history(
        "futures_history_executions",
        &["futures", "history-executions"],
        "executions",
        "history.get_executions.synthetic.json",
    ),
    futures_history(
        "futures_history_orders",
        &["futures", "history-orders"],
        "orders",
        "history.get_orders.synthetic.json",
    ),
    futures_history(
        "futures_history_triggers",
        &["futures", "history-triggers"],
        "triggers",
        "history.get_triggers.synthetic.json",
    ),
    futures_history(
        "futures_account_log_csv",
        &["futures", "history-account-log-csv"],
        "accountlogcsv",
        "history.get_accountlogcsv.synthetic.csv",
    ),
];

/// A command answered by public routes.
const fn public(
    name: &'static str,
    argv: &'static [&'static str],
    routes: &'static [Route],
) -> Case {
    Case {
        name,
        argv,
        answers: Answers::Public(routes),
    }
}

/// A command answered by one Futures trading endpoint.
const fn futures(
    name: &'static str,
    argv: &'static [&'static str],
    path: &'static str,
    fixture: &'static str,
) -> Case {
    Case {
        name,
        argv,
        answers: Answers::Futures { path, fixture },
    }
}

/// A command answered by one Futures history endpoint, named without its API
/// prefix.
const fn futures_history(
    name: &'static str,
    argv: &'static [&'static str],
    endpoint: &'static str,
    fixture: &'static str,
) -> Case {
    Case {
        name,
        argv,
        answers: Answers::History { endpoint, fixture },
    }
}

/// A command answered by one private endpoint and its captured fixture, both
/// named by the endpoint (`Earn/Allocate` → `spot/Earn_Allocate.json`).
const fn private(
    name: &'static str,
    argv: &'static [&'static str],
    endpoint: &'static str,
) -> Case {
    Case {
        name,
        argv,
        answers: Answers::Private(endpoint),
    }
}

async fn mount(server: &MockServer, route: &Route) {
    mount_at(server, route.path, route.query, route.fixture).await;
}

async fn mount_private(server: &MockServer, endpoint: &str) {
    let fixture = format!("spot/{}.json", endpoint.replace('/', "_"));
    mount_at(server, &format!("/0/private/{endpoint}"), None, &fixture).await;
}

async fn mount_at(server: &MockServer, at: &str, query: Option<(&str, &str)>, fixture: &str) {
    let body = std::fs::read_to_string(format!("{FIXTURES}/{fixture}"))
        .unwrap_or_else(|e| panic!("fixture {fixture}: {e}"));
    let mock = Mock::given(path(at));
    let mock = match query {
        Some((key, value)) => mock.and(query_param(key, value)),
        None => mock,
    };
    let content_type = if fixture.ends_with(".csv") {
        "text/csv"
    } else {
        "application/json"
    };
    mock.respond_with(ResponseTemplate::new(200).set_body_raw(body, content_type))
        .mount(server)
        .await;
}

fn parse(argv: &[&str]) -> Command {
    let cli = Cli::try_parse_from(std::iter::once("kraken").chain(argv.iter().copied()))
        .unwrap_or_else(|e| panic!("{argv:?} does not parse: {e}"));
    cli.command
        .unwrap_or_else(|| panic!("{argv:?} names no command"))
}

#[tokio::test]
async fn every_rest_command_keeps_its_published_output() {
    for case in CASES {
        let server = MockServer::start().await;
        match case.answers {
            Answers::Public(routes) => {
                for route in routes {
                    mount(&server, route).await;
                }
            }
            Answers::Private(endpoint) => mount_private(&server, endpoint).await,
            Answers::Futures { path, fixture } => {
                mount_at(&server, path, None, &format!("futures/{fixture}")).await;
            }
            Answers::History { endpoint, fixture } => {
                let at = format!("/api/history/v3/{endpoint}");
                mount_at(&server, &at, None, &format!("futures/{fixture}")).await;
            }
        }
        let ctx = venue_ctx(server.uri(), "test-api-key");
        let output = super::execute_command(&ctx, parse(case.argv))
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
        insta::assert_json_snapshot!(
            case.name,
            json!({
                "data": output.data,
                "headers": output.headers,
                "rows": output.rows,
                "footer": output.footer,
            })
        );
    }
}
