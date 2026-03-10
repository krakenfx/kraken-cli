---
name: kraken-ws-streaming
version: 1.0.0
description: "Real-time data streaming via WebSocket for spot and futures."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-ws-streaming

Use this skill for:
- streaming live price, trade, and order book data
- monitoring authenticated feeds (executions, balances)
- futures-specific streaming (fills, positions, balances)
- building event-driven agent loops

## Output Format

All WebSocket commands emit NDJSON (one JSON object per line) to stdout. Parse line by line:

```bash
kraken ws ticker BTC/USD -o json 2>/dev/null | while read -r line; do
  echo "$line" | jq -r '.data[0].last // empty'
done
```

Do not attempt to parse the full stream as a single JSON object.

## Spot Public Streams

Ticker (best bid/ask, last price, volume):

```bash
kraken ws ticker BTC/USD -o json 2>/dev/null
```

Ticker with BBO trigger (fires only on best-bid/offer changes):

```bash
kraken ws ticker BTC/USD --event-trigger bbo -o json 2>/dev/null
```

Trades:

```bash
kraken ws trades BTC/USD -o json 2>/dev/null
```

Order book (L2, configurable depth):

```bash
kraken ws book BTC/USD --depth 10 -o json 2>/dev/null
```

OHLC candles:

```bash
kraken ws ohlc BTC/USD --interval 1 -o json 2>/dev/null
```

Instrument metadata:

```bash
kraken ws instrument BTC/USD -o json 2>/dev/null
```

## Spot Private Streams (Authenticated)

Execution updates (fills, order state changes):

```bash
kraken ws executions -o json 2>/dev/null
```

Balance updates:

```bash
kraken ws balances -o json 2>/dev/null
```

L3 order book:

```bash
kraken ws level3 BTC/USD -o json 2>/dev/null
```

## Futures Streams

Futures ticker:

```bash
kraken futures ws ticker PF_XBTUSD -o json 2>/dev/null
```

Futures trades:

```bash
kraken futures ws trade PF_XBTUSD -o json 2>/dev/null
```

Futures order book:

```bash
kraken futures ws book PF_XBTUSD -o json 2>/dev/null
```

## Futures Private Streams (Authenticated)

Fills:

```bash
kraken futures ws fills -o json 2>/dev/null
```

Open orders:

```bash
kraken futures ws open-orders -o json 2>/dev/null
```

Open positions:

```bash
kraken futures ws open-positions -o json 2>/dev/null
```

Balances and margins:

```bash
kraken futures ws balances -o json 2>/dev/null
```

Notifications:

```bash
kraken futures ws notifications -o json 2>/dev/null
```

Account log:

```bash
kraken futures ws account-log -o json 2>/dev/null
```

## WebSocket Order Mutations (Spot)

Place, amend, and cancel orders over WebSocket for lower latency:

```bash
kraken ws add-order -o json 2>/dev/null
kraken ws amend-order -o json 2>/dev/null
kraken ws cancel-order -o json 2>/dev/null
kraken ws cancel-all -o json 2>/dev/null
kraken ws batch-add -o json 2>/dev/null
kraken ws batch-cancel -o json 2>/dev/null
```

Dead man's switch over WebSocket:

```bash
kraken ws cancel-after 60 -o json 2>/dev/null
```

## Agent Loop Pattern

A typical event-driven agent reads from a stream and acts on each event:

```bash
kraken ws ticker BTC/USD -o json 2>/dev/null | while read -r line; do
  LAST=$(echo "$line" | jq -r '.data[0].last // empty')
  [ -z "$LAST" ] && continue
  # Agent logic: compare price to thresholds, trigger actions
done
```

For multi-feed agents, run each stream in a background process and merge events.

## Context Efficiency

- Use `--depth` to limit order book snapshot size.
- Use `--event-trigger bbo` on tickers to reduce noise.
- Prefer streaming over high-frequency REST polling.
- Close streams when no longer needed; each holds a connection.

## Hard Rules

- WebSocket order mutations are flagged as dangerous. Require human approval.
- Never treat NDJSON stream output as a single JSON document.
- Handle stream disconnects gracefully; the CLI reconnects automatically with paced exponential backoff and reconnect safety budgeting (up to 12 attempts per stream lifecycle).
