//! Public market data commands — no auth required.

use super::{Execute, rest_bridge};
use crate::{
    cli::AppContext,
    errors::{KrakenError, Result},
    output::{CommandOutput, OutputFormat, json_field, published_json, render, render_error},
};
use futures_util::future::try_join_all;
use kraken_contract::{
    market::{
        Assets, MarketCommand, Ohlc, Orderbook, OrderbookGrouped, Pairs, ServerTime, Spreads,
        Status, Ticker, Trades,
    },
    value_types::{AssetClass, Grouping},
};
use kraken_rest::{
    Pair,
    spot::{
        SpotRest,
        market::{self as rest, Tickers},
    },
};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    future::Future,
    time::Duration,
};
use tokio::time::MissedTickBehavior;

impl Execute for Status {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let status = ctx
            .spot()
            .query(&rest::SystemStatusQuery::default())
            .await?;
        let data = published_json(&status)?;
        Ok(CommandOutput::key_value(
            vec![
                ("Status".into(), json_field(&data, "status")),
                ("Timestamp".into(), status.timestamp),
            ],
            data,
        ))
    }
}

impl Execute for ServerTime {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let time = ctx.spot().query(&rest::ServerTimeQuery::default()).await?;
        let data = published_json(&time)?;
        Ok(CommandOutput::key_value(
            vec![
                ("Unix Time".into(), time.unixtime.to_string()),
                ("RFC 1123".into(), time.rfc1123),
            ],
            data,
        ))
    }
}

impl Execute for Assets {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { asset, asset_class } = self;
        let query = rest::AssetsQuery {
            asset,
            aclass: asset_class.map(rest_bridge::asset_class),
        };
        let data = published_json(&ctx.spot().query(&query).await?)?;
        let (headers, rows) = parse_asset_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

impl Execute for Pairs {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            pair,
            info,
            asset_class,
        } = self;
        let query = rest::AssetPairsQuery {
            pair,
            info: info.as_deref().map(rest_bridge::pair_info).transpose()?,
            aclass: asset_class.map(rest_bridge::asset_class),
        };
        let data = published_json(&ctx.spot().query(&query).await?)?;
        let (headers, rows) = parse_pairs_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

impl Execute for Ticker {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            pairs,
            all,
            asset_class,
            watch,
        } = self;
        if watch.is_some() {
            // `--watch` streams; `dispatch` intercepts it, so only the
            // render-free executor (the MCP path) can reach this rejection.
            return Err(super::streaming::streaming_unsupported());
        }
        ticker_snapshot(ctx.spot(), &pairs, all, asset_class).await
    }
}

/// One fetch+shape pass shared verbatim by the one-shot path and every
/// `--watch` tick. Supports mixed asset-class batches by classifying and
/// grouping pairs, then merging responses. With --all, fetches every
/// tradable pair (currency + tokenized_asset only; forex is not enumerable).
async fn ticker_snapshot(
    rest: &SpotRest,
    pairs: &[String],
    all: bool,
    asset_class: Option<AssetClass>,
) -> Result<CommandOutput> {
    let tickers = if all {
        all_tickers(rest, asset_class).await?
    } else if let Some(class) = asset_class {
        // Explicit asset-class: send all pairs to that namespace as-is.
        ticker_request(rest, pairs, Some(class)).await?
    } else {
        // No asset-class specified: classify each pair via the catalog.
        mixed_class_ticker(rest, pairs).await?
    };
    let data = published_json(&tickers)?;
    let (headers, rows) = parse_ticker_table(&data);
    let mut out = CommandOutput::new(data, headers, rows);
    if tickers.len() > 50 {
        let omitted = tickers.len().saturating_sub(50);
        out = out.with_footer(vec![format!(
            "{omitted} more pairs omitted from the table; JSON output carries all"
        )]);
    }
    Ok(out)
}

/// `ticker --watch`: snapshot on a fixed cadence until a shutdown signal.
/// `dispatch` routes here whenever `--watch` is set, mirroring the
/// `session start` interception.
pub(super) async fn watch(args: Ticker, ctx: &AppContext) -> Result<()> {
    let Ticker {
        pairs,
        all,
        asset_class,
        watch,
    } = args;
    let Some(secs) = watch else {
        // Reachable only if the dispatch guard drifts from this contract;
        // answered like any other executor misuse.
        return Err(super::streaming::streaming_unsupported());
    };
    let rest = ctx.spot();
    let format = ctx.format;
    watch_ticks(
        Duration::from_secs(secs),
        || {
            // Build the fetch future before the async block: the block then
            // owns it instead of borrowing the closure environment, which a
            // lending `FnMut` cannot express.
            let snapshot = ticker_snapshot(rest, &pairs, all, asset_class);
            async move { emit_snapshot(format, snapshot.await) }
        },
        crate::stream::shutdown_signal(),
    )
    .await;
    Ok(())
}

/// One tick's output: stdout carries the data line (or, in JSON mode, the
/// error envelope for that tick); table-mode errors go to stderr. Either way
/// the loop keeps its cadence — ticker is public data and the client already
/// retried transient failures.
fn emit_snapshot(format: OutputFormat, result: Result<CommandOutput>) {
    match result {
        Ok(out) => render(format, &out),
        Err(error) => match format {
            OutputFormat::Json => render_error(format, &error),
            OutputFormat::Table => tracing::warn!(%error, "ticker snapshot failed"),
        },
    }
}

/// Runs `work` once per tick until `shutdown` resolves. The loop owns only
/// cadence and shutdown; what a tick does — and its failure policy — is
/// composed by the caller. Shutdown is also armed against in-flight work: a
/// signal during a slow tick (client-internal retries can stretch minutes)
/// still exits promptly.
async fn watch_ticks<Fut>(
    period: Duration,
    mut work: impl FnMut() -> Fut,
    shutdown: impl Future<Output = ()>,
) where
    Fut: Future<Output = ()>,
{
    let mut ticks = tokio::time::interval(period);
    // After an overrun, Skip fires the overdue tick once and realigns to the
    // next period multiple — no back-to-back burst of stale snapshots.
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = ticks.tick() => {}
            () = &mut shutdown => return,
        }
        tokio::select! {
            () = work() => {}
            () = &mut shutdown => return,
        }
    }
}

async fn ticker_request(
    rest: &SpotRest,
    pairs: &[String],
    asset_class: Option<AssetClass>,
) -> Result<Tickers> {
    let query = rest_bridge::ticker_query(pairs, asset_class);
    Ok(rest.query(&query).await?)
}

/// Upstream scopes each Ticker request to a single asset-class namespace, so
/// a mixed batch can never succeed in one call: classify every pair, then
/// issue one request per class present and merge the responses.
/// `--all`: one map holding every tradable pair across the requested classes,
/// fetched concurrently.
async fn all_tickers(rest: &SpotRest, asset_class: Option<AssetClass>) -> Result<Tickers> {
    let classes = match asset_class {
        Some(class) => vec![class],
        None => vec![AssetClass::Currency, AssetClass::TokenizedAsset],
    };
    let per_class =
        try_join_all(classes.into_iter().map(|class| class_tickers(rest, class))).await?;
    Ok(merge_tickers(per_class))
}

/// Fetch all tickers for a single asset class. For currency (the default),
/// send no asset_class param; for others, include it. Forex is unsupported
/// and must be queried by explicit pair.
async fn class_tickers(rest: &SpotRest, class: AssetClass) -> Result<Tickers> {
    match class {
        // Currency is the endpoint default: no param sends the whole catalog.
        AssetClass::Currency => ticker_request(rest, &[], None).await,
        AssetClass::TokenizedAsset => ticker_request(rest, &[], Some(class)).await,
        // Verified live 2026-08-10: upstream silently ignores the forex filter
        // on both `Ticker` (a no-pair request returns the currency set) and
        // `AssetPairs` (`aclass=forex` returns the crypto catalog), so forex
        // pairs cannot be enumerated — only queried by explicit name.
        AssetClass::Forex => Err(KrakenError::Validation(
            "forex pairs cannot be enumerated: the upstream catalog ignores the forex filter; \
             pass explicit pairs with --asset-class forex"
                .into(),
        )),
    }
}

async fn mixed_class_ticker(rest: &SpotRest, pairs: &[String]) -> Result<Tickers> {
    // Two extra public AssetPairs calls (both catalogs are small) buy
    // deterministic classification; anything unmatched defaults to currency,
    // so the ~1000-pair currency catalog is never fetched.
    let catalog = |class| rest::AssetPairsQuery {
        aclass: Some(rest_bridge::asset_class(class)),
        ..rest::AssetPairsQuery::default()
    };
    let (tokenized, forex) = (
        catalog(AssetClass::TokenizedAsset),
        catalog(AssetClass::Forex),
    );
    let (tokenized, forex) = tokio::try_join!(rest.query(&tokenized), rest.query(&forex))?;
    let tokenized = catalog_symbols(&tokenized);
    let forex = catalog_symbols(&forex);

    let mut tokenized_pairs = Vec::new();
    let mut forex_pairs = Vec::new();
    let mut currency_pairs = Vec::new();
    for pair in pairs {
        match classify(pair, &tokenized, &forex) {
            AssetClass::TokenizedAsset => tokenized_pairs.push(pair.clone()),
            AssetClass::Forex => forex_pairs.push(pair.clone()),
            AssetClass::Currency => currency_pairs.push(pair.clone()),
        }
    }
    // Currency is the endpoint default: sending no param keeps the degenerate
    // all-currency batch identical to today's single request.
    let groups = [
        (Some(AssetClass::TokenizedAsset), tokenized_pairs),
        (Some(AssetClass::Forex), forex_pairs),
        (None, currency_pairs),
    ];
    let responses = try_join_all(
        groups
            .into_iter()
            .filter(|(_, group)| !group.is_empty())
            .map(|(class, group)| async move { ticker_request(rest, &group, class).await }),
    )
    .await?;
    Ok(merge_tickers(responses))
}

/// One map from per-class batches. A pair two batches both return is
/// unexpected; the later batch's ticker replaces the earlier one in place.
fn merge_tickers(batches: Vec<Tickers>) -> Tickers {
    let mut merged: Vec<(String, rest::Ticker)> = Vec::new();
    // `--all` merges ~1000 pairs; a position index keeps replace-in-place O(1).
    let mut position: HashMap<String, usize> = HashMap::new();
    for (pair, ticker) in batches.into_iter().flatten() {
        match position.entry(pair) {
            Entry::Occupied(seen) => {
                tracing::warn!(pair = %seen.key(), "pair returned by more than one asset-class request; keeping the last");
                merged[*seen.get()].1 = ticker;
            }
            Entry::Vacant(slot) => {
                merged.push((slot.key().clone(), ticker));
                slot.insert(merged.len() - 1);
            }
        }
    }
    merged.into_iter().collect()
}

/// Multi-match precedence is tokenized_asset, then forex; anything unmatched
/// defaults to currency.
fn classify(pair: &str, tokenized: &HashSet<String>, forex: &HashSet<String>) -> AssetClass {
    let name = normalize_pair(pair);
    if tokenized.contains(&name) {
        AssetClass::TokenizedAsset
    } else if forex.contains(&name) {
        AssetClass::Forex
    } else {
        AssetClass::Currency
    }
}

/// Spellings match case- and slash-insensitively: users write `TSLAx/USD`
/// where the catalog key is `TSLAxUSD`.
fn normalize_pair(pair: &str) -> String {
    pair.replace('/', "").to_uppercase()
}

/// Every spelling an AssetPairs catalog admits for its pairs — wire key,
/// `altname`, and `wsname` — normalized like [`normalize_pair`].
fn catalog_symbols(catalog: &rest::AssetPairs) -> HashSet<String> {
    catalog
        .iter()
        .flat_map(|(key, pair)| {
            [Some(key), pair.altname.as_deref(), pair.wsname.as_deref()]
                .into_iter()
                .flatten()
                .map(normalize_pair)
        })
        .collect()
}

impl Execute for Ohlc {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            pair,
            interval,
            since,
            asset_class,
        } = self;
        let query = rest::OhlcQuery {
            pair: Pair::from(pair.as_str()),
            interval,
            since,
            asset_class: asset_class.map(rest_bridge::asset_class),
        };
        let mut ohlc = ctx.spot().query(&query).await?;
        // The page echoes the caller's spelling, not Kraken's wire name.
        ohlc.pair = pair.into();
        let data = published_json(&ohlc)?;
        let (headers, rows) = parse_ohlc_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

impl Execute for Orderbook {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            pair,
            count,
            asset_class,
        } = self;
        let query = rest::DepthQuery {
            pair: Pair::from(pair),
            count: Some(count),
            asset_class: asset_class.map(rest_bridge::asset_class),
        };
        let data = published_json(&ctx.spot().query(&query).await?)?;
        let (headers, rows) = parse_orderbook_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

impl Execute for OrderbookGrouped {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            pair,
            depth,
            grouping,
        } = self;
        let query = rest::GroupedBookQuery {
            pair: Pair::from(pair),
            depth: Some(rest_bridge::grouped_depth(depth)),
            // One tick is the endpoint's default; omitting it keeps the request
            // what it has always been.
            grouping: (grouping != Grouping::G1).then(|| rest_bridge::grouping(grouping)),
        };
        let data = published_json(&ctx.spot().query(&query).await?)?;
        Ok(CommandOutput::new(
            data,
            vec!["Data".into()],
            vec![vec![
                "Grouped orderbook data returned (see JSON output)".into(),
            ]],
        ))
    }
}

impl Execute for Trades {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            pair,
            since,
            count,
            asset_class,
        } = self;
        let query = rest::TradesQuery {
            pair: Pair::from(pair.as_str()),
            since,
            count: Some(count),
            asset_class: asset_class.map(rest_bridge::asset_class),
        };
        let mut trades = ctx.spot().query(&query).await?;
        trades.pair = pair.into();
        let data = published_json(&trades)?;
        let (headers, rows) = parse_trades_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

impl Execute for Spreads {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            pair,
            since,
            asset_class,
        } = self;
        let query = rest::SpreadsQuery {
            pair: Pair::from(pair.as_str()),
            since,
            asset_class: asset_class.map(rest_bridge::asset_class),
        };
        let mut spreads = ctx.spot().query(&query).await?;
        spreads.pair = pair.into();
        let data = published_json(&spreads)?;
        let (headers, rows) = parse_spreads_table(&data);
        Ok(CommandOutput::new(data, headers, rows))
    }
}

fn parse_asset_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec![
        "Asset".into(),
        "Altname".into(),
        "Decimals".into(),
        "Display Decimals".into(),
        "Status".into(),
    ];
    let mut rows = Vec::new();
    if let Some(obj) = data.as_object() {
        for (key, val) in obj {
            rows.push(vec![
                key.clone(),
                json_field(val, "altname"),
                json_field(val, "decimals"),
                json_field(val, "display_decimals"),
                json_field(val, "status"),
            ]);
        }
    }
    rows.sort_by(|a, b| a[0].cmp(&b[0]));
    (headers, rows)
}

fn parse_pairs_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec![
        "Pair".into(),
        "Status".into(),
        "Tick Size".into(),
        "Order Min".into(),
        "Cost Min".into(),
    ];
    let mut rows = Vec::new();
    if let Some(obj) = data.as_object() {
        for (key, val) in obj {
            rows.push(vec![
                key.clone(),
                json_field(val, "status"),
                json_field(val, "tick_size"),
                json_field(val, "ordermin"),
                json_field(val, "costmin"),
            ]);
        }
    }
    rows.sort_by(|a, b| a[0].cmp(&b[0]));
    (headers, rows)
}

fn parse_ticker_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec![
        "Pair".into(),
        "Ask".into(),
        "Bid".into(),
        "Last".into(),
        "Volume (24h)".into(),
        "High (24h)".into(),
        "Low (24h)".into(),
    ];
    let mut rows = Vec::new();
    if let Some(obj) = data.as_object() {
        for (key, val) in obj {
            rows.push(vec![
                key.clone(),
                json_field(val, "ask_price"),
                json_field(val, "bid_price"),
                json_field(val, "last_price"),
                json_field(val, "volume_24h"),
                json_field(val, "high_24h"),
                json_field(val, "low_24h"),
            ]);
        }
    }
    (headers, rows)
}

fn parse_ohlc_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec![
        "Time".into(),
        "Open".into(),
        "High".into(),
        "Low".into(),
        "Close".into(),
        "Volume".into(),
    ];
    let mut rows = Vec::new();
    if let Some(Value::Array(candles)) = data.get("candles") {
        for candle in candles {
            rows.push(vec![
                json_field(candle, "time"),
                json_field(candle, "open"),
                json_field(candle, "high"),
                json_field(candle, "low"),
                json_field(candle, "close"),
                json_field(candle, "volume"),
            ]);
        }
    }
    (headers, rows)
}

/// Table shows top-of-book only (10 levels a side); the JSON payload carries
/// the full requested depth.
fn parse_orderbook_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    const TABLE_LEVELS: usize = 10;
    let headers = vec!["Side".into(), "Price".into(), "Volume".into()];
    let mut rows = Vec::new();
    for (side, label) in [("asks", "ASK"), ("bids", "BID")] {
        if let Some(Value::Array(levels)) = data.get(side) {
            for level in levels.iter().take(TABLE_LEVELS) {
                rows.push(vec![
                    label.into(),
                    json_field(level, "price"),
                    json_field(level, "volume"),
                ]);
            }
        }
    }
    (headers, rows)
}

/// Table shows the newest 20 trades (Kraken returns oldest-first); the JSON
/// payload carries the full result plus the `last` pagination cursor.
fn parse_trades_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    const TABLE_TRADES: usize = 20;
    let headers = vec![
        "Price".into(),
        "Volume".into(),
        "Time".into(),
        "Side".into(),
        "Type".into(),
    ];
    let mut rows = Vec::new();
    if let Some(Value::Array(trades)) = data.get("trades") {
        for trade in trades.iter().rev().take(TABLE_TRADES) {
            // JSON keeps the raw wire token (`l`/`m`); the human table
            // spells it out.
            let order_type = match trade.get("order_type").and_then(Value::as_str) {
                Some("l") => "limit".into(),
                Some("m") => "market".into(),
                _ => json_field(trade, "order_type"),
            };
            rows.push(vec![
                json_field(trade, "price"),
                json_field(trade, "volume"),
                json_field(trade, "time"),
                json_field(trade, "side"),
                order_type,
            ]);
        }
    }
    (headers, rows)
}

fn parse_spreads_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec!["Time".into(), "Bid".into(), "Ask".into()];
    let mut rows = Vec::new();
    if let Some(Value::Array(spreads)) = data.get("spreads") {
        for spread in spreads.iter().rev().take(20) {
            rows.push(vec![
                json_field(spread, "time"),
                json_field(spread, "bid"),
                json_field(spread, "ask"),
            ]);
        }
    }
    (headers, rows)
}

delegate_execute!(MarketCommand {
    Status,
    ServerTime,
    Assets,
    Pairs,
    Ticker,
    Ohlc,
    Orderbook,
    OrderbookGrouped,
    Trades,
    Spreads
});

// The watch-loop tests drive `watch_ticks` through an injected per-tick work
// closure under paused time: no live socket, no real sleep.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::KrakenError;
    use clap::Parser;
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };
    use tokio::sync::{Notify, mpsc};

    const PERIOD: Duration = Duration::from_secs(30);

    fn snapshot() -> CommandOutput {
        CommandOutput::message("tick")
    }

    fn ctx() -> AppContext {
        crate::commands::test_support::offline_ctx(None, None)
    }

    fn ticker_args(watch: Option<u64>) -> Ticker {
        Ticker {
            pairs: vec!["BTC/USD".into()],
            all: false,
            asset_class: None,
            watch,
        }
    }

    #[tokio::test]
    async fn watch_is_rejected_by_direct_execute() {
        let Err(err) = ticker_args(Some(5)).execute(&ctx()).await else {
            panic!("the render-free executor must not stream --watch");
        };
        assert_eq!(
            err.to_string(),
            super::super::streaming::streaming_unsupported().to_string()
        );
    }

    #[tokio::test]
    async fn a_none_watch_is_rejected_by_the_watch_path() {
        let Err(err) = watch(ticker_args(None), &ctx()).await else {
            panic!("watch() without --watch must be answered as executor misuse");
        };
        assert_eq!(
            err.to_string(),
            super::super::streaming::streaming_unsupported().to_string()
        );
    }

    #[test]
    fn the_watch_floor_rejects_zero_seconds() {
        let Err(err) = crate::Cli::try_parse_from(["kraken", "ticker", "BTC/USD", "--watch", "0"])
        else {
            panic!("--watch 0 must not parse");
        };
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[tokio::test(start_paused = true)]
    async fn watch_runs_the_work_once_per_tick() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let stop = Arc::new(Notify::new());
        let stopper = Arc::clone(&stop);
        let watcher = tokio::spawn(async move {
            watch_ticks(
                PERIOD,
                move || {
                    let tx = tx.clone();
                    async move { tx.send(tokio::time::Instant::now()).unwrap() }
                },
                async move { stopper.notified().await },
            )
            .await;
        });
        let mut ticks = Vec::new();
        for _ in 0..3 {
            ticks.push(rx.recv().await.unwrap());
        }
        stop.notify_one();
        watcher.await.unwrap();
        // Paused time only advances to timer deadlines, so per-tick work
        // shows as exact period spacing.
        assert_eq!(ticks[1] - ticks[0], PERIOD);
        assert_eq!(ticks[2] - ticks[1], PERIOD);
    }

    /// The caller-composed work closure owns the failure policy, exactly as
    /// `watch()` wires `ticker_snapshot` into `emit_snapshot`: a failed tick
    /// emits its envelope and the cadence carries on.
    #[tokio::test(start_paused = true)]
    async fn watch_keeps_its_cadence_after_a_failed_tick() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let stop = Arc::new(Notify::new());
        let stopper = Arc::clone(&stop);
        let attempts = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&attempts);
        let watcher = tokio::spawn(async move {
            watch_ticks(
                PERIOD,
                move || {
                    let attempt = counter.fetch_add(1, Ordering::SeqCst);
                    let tx = tx.clone();
                    async move {
                        let result: Result<CommandOutput> = if attempt == 0 {
                            Err(KrakenError::Network("venue unreachable".into()))
                        } else {
                            Ok(snapshot())
                        };
                        tx.send(result.is_ok()).unwrap();
                    }
                },
                async move { stopper.notified().await },
            )
            .await;
        });
        assert_eq!(rx.recv().await, Some(false), "the failed tick still emits");
        assert_eq!(rx.recv().await, Some(true), "the loop survives the error");
        stop.notify_one();
        watcher.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn watch_exits_on_shutdown_during_in_flight_work() {
        let stop = Arc::new(Notify::new());
        let stopper = Arc::clone(&stop);
        let work_started = Arc::new(Notify::new());
        let started = Arc::clone(&work_started);
        let watcher = tokio::spawn(async move {
            watch_ticks(
                PERIOD,
                move || {
                    let started = Arc::clone(&started);
                    async move {
                        started.notify_one();
                        std::future::pending::<()>().await
                    }
                },
                async move { stopper.notified().await },
            )
            .await;
        });
        work_started.notified().await;
        stop.notify_one();
        watcher.await.unwrap();
    }
}
