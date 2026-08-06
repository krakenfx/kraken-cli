---
name: kraken-multi-pair
version: 1.0.0
description: "Monitor multiple trading pairs simultaneously for screening and comparison."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-multi-pair

Use this skill for:
- screening multiple pairs at once (price, spread, volume)
- comparing relative performance across assets
- building watchlists for agent-driven monitoring
- identifying opportunity across a pair universe

## Multi-Pair Ticker Snapshot

Pass multiple pairs to a single ticker call:

```bash
kraken ticker BTCUSD ETHUSD SOLUSD DOTUSD -o json 2>/dev/null
```

The response contains one object per pair. Extract and compare:

```bash
kraken ticker BTCUSD ETHUSD SOLUSD -o json 2>/dev/null | jq 'to_entries[] | {pair: .key, last: .value.last_price, volume: .value.volume_24h}'
```

## Multi-Pair Streaming

Stream tickers for several pairs simultaneously:

```bash
kraken ws ticker BTC/USD ETH/USD SOL/USD -o json 2>/dev/null
```

Each NDJSON line includes a pair identifier. Route updates by pair in the agent loop:

```bash
kraken ws ticker BTC/USD ETH/USD SOL/USD -o json 2>/dev/null | while read -r line; do
  PAIR=$(echo "$line" | jq -r '.data[0].symbol // empty')
  LAST=$(echo "$line" | jq -r '.data[0].last // empty')
  [ -n "$PAIR" ] && echo "$PAIR: $LAST"
done
```

## Pair Discovery

List all tradable pairs:

```bash
kraken pairs -o json 2>/dev/null
```

Filter to USD-quoted pairs for a consistent base:

```bash
kraken pairs -o json 2>/dev/null | jq '[to_entries[] | select(.key | endswith("USD")) | .key]'
```

## Spread Comparison

Compare bid-ask spreads across pairs to gauge liquidity:

```bash
kraken ticker BTCUSD ETHUSD SOLUSD -o json 2>/dev/null | jq 'to_entries[] | {pair: .key, spread: (.value.ask_price - .value.bid_price)}'
```

## Volume Screening

Identify high-volume pairs from a watchlist:

```bash
kraken ticker BTCUSD ETHUSD SOLUSD ADAUSD DOTUSD -o json 2>/dev/null | jq 'to_entries | sort_by(-.value.volume_24h) | .[] | {pair: .key, vol_24h: .value.volume_24h}'
```

## Futures Multi-Symbol

Monitor multiple futures contracts:

```bash
kraken futures tickers -o json 2>/dev/null
```

Stream multiple futures tickers:

```bash
kraken futures ws ticker PF_XBTUSD PF_ETHUSD -o json 2>/dev/null
```

## Watchlist Pattern

Define a watchlist as a space-separated string and reuse across commands:

```bash
WATCHLIST="BTCUSD ETHUSD SOLUSD ADAUSD DOTUSD"
kraken ticker $WATCHLIST -o json 2>/dev/null
kraken ws ticker $WATCHLIST -o json 2>/dev/null
```

## Context Efficiency

- Batch pairs into a single command instead of one call per pair.
- Use streaming for continuous monitoring; use REST snapshots for periodic checks.
- Limit watchlist size to reduce context window consumption when passing results to an agent.

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
