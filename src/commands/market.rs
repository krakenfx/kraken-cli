/// Public market data commands — no auth required.
use clap::Subcommand;
use serde_json::Value;

use super::helpers::jstr;
use crate::client::SpotClient;
use crate::errors::{KrakenError, Result};
use crate::output::CommandOutput;

#[derive(Debug, Subcommand)]
pub(crate) enum MarketCommand {
    /// Get system status and trading mode.
    Status,
    /// Get server time.
    ServerTime,
    /// Get asset info.
    Assets {
        /// Comma-separated list of assets to query.
        #[arg(long)]
        asset: Option<String>,
        /// Asset class filter.
        #[arg(long, alias = "aclass")]
        asset_class: Option<String>,
    },
    /// Get tradable asset pairs.
    Pairs {
        /// Comma-separated pairs to query.
        #[arg(long)]
        pair: Option<String>,
        /// Info level: info, leverage, fees, margin.
        #[arg(long)]
        info: Option<String>,
        /// Asset class filter.
        #[arg(long, alias = "aclass")]
        asset_class: Option<String>,
    },
    /// Get ticker information for one or more pairs.
    Ticker {
        /// One or more trading pairs.
        pairs: Vec<String>,
        /// Asset class filter.
        #[arg(long, value_parser = ["tokenized_asset", "forex"])]
        asset_class: Option<String>,
    },
    /// Get OHLC candle data.
    Ohlc {
        /// Trading pair.
        pair: String,
        /// Candle interval in minutes (1, 5, 15, 30, 60, 240, 1440, 10080, 21600).
        #[arg(long, default_value = "60")]
        interval: u32,
        /// Unix timestamp to fetch data since.
        #[arg(long)]
        since: Option<String>,
        /// Asset class filter.
        #[arg(long, value_parser = ["tokenized_asset", "forex"])]
        asset_class: Option<String>,
    },
    /// Get L2 order book.
    Orderbook {
        /// Trading pair.
        pair: String,
        /// Number of price levels (max 500).
        #[arg(long, default_value = "25")]
        count: u32,
        /// Asset class filter.
        #[arg(long, value_parser = ["tokenized_asset"])]
        asset_class: Option<String>,
    },
    /// Get L3 order book (authenticated).
    OrderbookL3 {
        /// Trading pair.
        pair: String,
        /// Number of price levels per side (0 for full book).
        #[arg(long, default_value = "100", value_parser = ["0", "10", "25", "100", "250", "1000"])]
        depth: String,
    },
    /// Get grouped order book.
    OrderbookGrouped {
        /// Trading pair.
        pair: String,
        /// Number of price levels per side.
        #[arg(long, default_value = "10", value_parser = ["10", "25", "100", "250", "1000"])]
        depth: String,
        /// Tick levels within each price level (bids rounded down, asks up).
        #[arg(long, default_value = "1", value_parser = ["1", "5", "10", "25", "50", "100", "250", "500", "1000"])]
        grouping: String,
    },
    /// Get recent trades.
    Trades {
        /// Trading pair.
        pair: String,
        /// Fetch trades since this timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Maximum number of trades.
        #[arg(long, default_value = "1000")]
        count: u32,
        /// Asset class filter.
        #[arg(long, value_parser = ["tokenized_asset"])]
        asset_class: Option<String>,
    },
    /// Get recent spreads.
    Spreads {
        /// Trading pair.
        pair: String,
        /// Fetch spreads since this timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Asset class filter.
        #[arg(long, value_parser = ["tokenized_asset"])]
        asset_class: Option<String>,
    },
}

pub(crate) async fn execute(
    cmd: &MarketCommand,
    client: &SpotClient,
    verbose: bool,
) -> Result<CommandOutput> {
    match cmd {
        MarketCommand::Status => {
            let data = client.public_get("SystemStatus", &[], verbose).await?;
            let status = data
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let timestamp = data.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
            Ok(CommandOutput::key_value(
                vec![
                    ("Status".into(), status.to_string()),
                    ("Timestamp".into(), timestamp.to_string()),
                ],
                data,
            ))
        }
        MarketCommand::ServerTime => {
            let data = client.public_get("Time", &[], verbose).await?;
            let unixtime = data.get("unixtime").and_then(|v| v.as_u64()).unwrap_or(0);
            let rfc1123 = data.get("rfc1123").and_then(|v| v.as_str()).unwrap_or("");
            Ok(CommandOutput::key_value(
                vec![
                    ("Unix Time".into(), unixtime.to_string()),
                    ("RFC 1123".into(), rfc1123.to_string()),
                ],
                data,
            ))
        }
        MarketCommand::Assets { asset, asset_class } => {
            let mut params = Vec::new();
            if let Some(a) = asset {
                params.push(("asset", a.as_str()));
            }
            if let Some(ac) = asset_class {
                params.push(("aclass", ac.as_str()));
            }
            let data = client.public_get("Assets", &params, verbose).await?;
            let (headers, rows) = parse_asset_table(&data);
            Ok(CommandOutput::new(data, headers, rows))
        }
        MarketCommand::Pairs {
            pair,
            info,
            asset_class,
        } => {
            let mut params = Vec::new();
            if let Some(p) = pair {
                params.push(("pair", p.as_str()));
            }
            if let Some(i) = info {
                params.push(("info", i.as_str()));
            }
            if let Some(ac) = asset_class {
                params.push(("aclass", ac.as_str()));
            }
            let data = client.public_get("AssetPairs", &params, verbose).await?;
            let (headers, rows) = parse_pairs_table(&data);
            Ok(CommandOutput::new(data, headers, rows))
        }
        MarketCommand::Ticker { pairs, asset_class } => {
            let pair_str = pairs.join(",");
            let mut params = vec![("pair", &*pair_str)];
            if let Some(ac) = asset_class {
                params.push(("asset_class", ac.as_str()));
            }
            let data = client.public_get("Ticker", &params, verbose).await?;
            let (headers, rows) = parse_ticker_table(&data);
            Ok(CommandOutput::new(data, headers, rows))
        }
        MarketCommand::Ohlc {
            pair,
            interval,
            since,
            asset_class,
        } => {
            let interval_str = interval.to_string();
            let mut params = vec![("pair", pair.as_str()), ("interval", &interval_str)];
            if let Some(s) = since {
                params.push(("since", s.as_str()));
            }
            if let Some(ac) = asset_class {
                params.push(("asset_class", ac.as_str()));
            }
            let data = client.public_get("OHLC", &params, verbose).await?;
            let (headers, rows) = parse_ohlc_table(&data, pair);
            Ok(CommandOutput::new(data, headers, rows))
        }
        MarketCommand::Orderbook {
            pair,
            count,
            asset_class,
        } => {
            let count_str = count.to_string();
            let mut params = vec![("pair", pair.as_str()), ("count", &count_str)];
            if let Some(ac) = asset_class {
                params.push(("asset_class", ac.as_str()));
            }
            let data = client.public_get("Depth", &params, verbose).await?;
            let (headers, rows) = parse_orderbook_table(&data, pair);
            Ok(CommandOutput::new(data, headers, rows))
        }
        MarketCommand::OrderbookL3 { .. } => Err(KrakenError::Validation(
            "L3 orderbook requires authentication; use the top-level dispatch".into(),
        )),
        MarketCommand::OrderbookGrouped {
            pair,
            depth,
            grouping,
        } => {
            let mut params = vec![("pair", pair.as_str()), ("depth", depth.as_str())];
            if grouping != "1" {
                params.push(("grouping", grouping.as_str()));
            }
            let data = client.public_get("GroupedBook", &params, verbose).await?;
            Ok(CommandOutput::new(
                data.clone(),
                vec!["Data".into()],
                vec![vec![
                    "Grouped orderbook data returned (see JSON output)".into()
                ]],
            ))
        }
        MarketCommand::Trades {
            pair,
            since,
            count,
            asset_class,
        } => {
            let count_str = count.to_string();
            let mut params = vec![("pair", pair.as_str()), ("count", &count_str)];
            if let Some(s) = since {
                params.push(("since", s.as_str()));
            }
            if let Some(ac) = asset_class {
                params.push(("asset_class", ac.as_str()));
            }
            let data = client.public_get("Trades", &params, verbose).await?;
            let (headers, rows) = parse_trades_table(&data, pair);
            Ok(CommandOutput::new(data, headers, rows))
        }
        MarketCommand::Spreads {
            pair,
            since,
            asset_class,
        } => {
            let mut params = vec![("pair", pair.as_str())];
            if let Some(s) = since {
                params.push(("since", s.as_str()));
            }
            if let Some(ac) = asset_class {
                params.push(("asset_class", ac.as_str()));
            }
            let data = client.public_get("Spread", &params, verbose).await?;
            let (headers, rows) = parse_spreads_table(&data, pair);
            Ok(CommandOutput::new(data, headers, rows))
        }
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
                jstr(val, "altname"),
                jstr(val, "decimals"),
                jstr(val, "display_decimals"),
                jstr(val, "status"),
            ]);
        }
    }
    rows.sort_by(|a, b| a[0].cmp(&b[0]));
    (headers, rows)
}

fn parse_pairs_table(data: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec![
        "Pair".into(),
        "Altname".into(),
        "Base".into(),
        "Quote".into(),
        "Status".into(),
    ];
    let mut rows = Vec::new();
    if let Some(obj) = data.as_object() {
        for (key, val) in obj {
            rows.push(vec![
                key.clone(),
                jstr(val, "altname"),
                jstr(val, "base"),
                jstr(val, "quote"),
                jstr(val, "status"),
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
            let ask = val
                .get("a")
                .and_then(|a| a.get(0))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let bid = val
                .get("b")
                .and_then(|a| a.get(0))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let last = val
                .get("c")
                .and_then(|a| a.get(0))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let vol = val
                .get("v")
                .and_then(|a| a.get(1))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let high = val
                .get("h")
                .and_then(|a| a.get(1))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let low = val
                .get("l")
                .and_then(|a| a.get(1))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            rows.push(vec![
                key.clone(),
                ask.to_string(),
                bid.to_string(),
                last.to_string(),
                vol.to_string(),
                high.to_string(),
                low.to_string(),
            ]);
        }
    }
    (headers, rows)
}

fn parse_ohlc_table(data: &Value, pair: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec![
        "Time".into(),
        "Open".into(),
        "High".into(),
        "Low".into(),
        "Close".into(),
        "Volume".into(),
    ];
    let mut rows = Vec::new();

    let candles = data.get(pair).or_else(|| {
        data.as_object()
            .and_then(|obj| obj.values().find(|v| v.is_array()))
    });

    if let Some(Value::Array(arr)) = candles {
        for candle in arr {
            if let Value::Array(c) = candle {
                let time = c.first().map(jval).unwrap_or_default();
                let open = c.get(1).map(jval).unwrap_or_default();
                let high = c.get(2).map(jval).unwrap_or_default();
                let low = c.get(3).map(jval).unwrap_or_default();
                let close = c.get(4).map(jval).unwrap_or_default();
                let vol = c.get(6).map(jval).unwrap_or_default();
                rows.push(vec![time, open, high, low, close, vol]);
            }
        }
    }
    (headers, rows)
}

fn parse_orderbook_table(data: &Value, pair: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec!["Side".into(), "Price".into(), "Volume".into()];
    let mut rows = Vec::new();

    let book = data
        .get(pair)
        .or_else(|| data.as_object().and_then(|obj| obj.values().next()));

    if let Some(book) = book {
        if let Some(Value::Array(asks)) = book.get("asks") {
            for ask in asks.iter().take(10) {
                if let Value::Array(a) = ask {
                    let price = a.first().map(jval).unwrap_or_default();
                    let vol = a.get(1).map(jval).unwrap_or_default();
                    rows.push(vec!["ASK".into(), price, vol]);
                }
            }
        }
        if let Some(Value::Array(bids)) = book.get("bids") {
            for bid in bids.iter().take(10) {
                if let Value::Array(b) = bid {
                    let price = b.first().map(jval).unwrap_or_default();
                    let vol = b.get(1).map(jval).unwrap_or_default();
                    rows.push(vec!["BID".into(), price, vol]);
                }
            }
        }
    }
    (headers, rows)
}

fn parse_trades_table(data: &Value, pair: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec![
        "Price".into(),
        "Volume".into(),
        "Time".into(),
        "Side".into(),
        "Type".into(),
    ];
    let mut rows = Vec::new();

    let trades = data.get(pair).or_else(|| {
        data.as_object()
            .and_then(|obj| obj.values().find(|v| v.is_array()))
    });

    if let Some(Value::Array(arr)) = trades {
        for trade in arr.iter().rev().take(20) {
            if let Value::Array(t) = trade {
                let price = t.first().map(jval).unwrap_or_default();
                let vol = t.get(1).map(jval).unwrap_or_default();
                let time = t.get(2).map(jval).unwrap_or_default();
                let side = t
                    .get(3)
                    .and_then(|v| v.as_str())
                    .map(|s| if s == "b" { "buy" } else { "sell" })
                    .unwrap_or("-");
                let order_type = t
                    .get(4)
                    .and_then(|v| v.as_str())
                    .map(|s| if s == "l" { "limit" } else { "market" })
                    .unwrap_or("-");
                rows.push(vec![
                    price,
                    vol,
                    time,
                    side.to_string(),
                    order_type.to_string(),
                ]);
            }
        }
    }
    (headers, rows)
}

fn parse_spreads_table(data: &Value, pair: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = vec!["Time".into(), "Bid".into(), "Ask".into()];
    let mut rows = Vec::new();

    let spreads = data.get(pair).or_else(|| {
        data.as_object()
            .and_then(|obj| obj.values().find(|v| v.is_array()))
    });

    if let Some(Value::Array(arr)) = spreads {
        for spread in arr.iter().rev().take(20) {
            if let Value::Array(s) = spread {
                let time = s.first().map(jval).unwrap_or_default();
                let bid = s.get(1).map(jval).unwrap_or_default();
                let ask = s.get(2).map(jval).unwrap_or_default();
                rows.push(vec![time, bid, ask]);
            }
        }
    }
    (headers, rows)
}

fn jval(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn orderbook_l3_returns_error_not_panic() {
        let client = SpotClient::new(Some("https://api.kraken.com")).unwrap();
        let cmd = MarketCommand::OrderbookL3 {
            pair: "BTCUSD".into(),
            depth: "100".into(),
        };
        let result = execute(&cmd, &client, false).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("L3 orderbook requires authentication"));
    }
}
