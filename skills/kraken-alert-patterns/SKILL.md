---
name: kraken-alert-patterns
version: 1.0.0
description: "Price alerts, threshold monitoring, and notification triggers for agents."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-alert-patterns

Use this skill for:
- monitoring price levels and triggering alerts
- detecting spread widening, volume spikes, and volatility shifts
- watching account state changes (fills, balance drops)
- building notification-driven agent workflows

## Price Alert (Polling)

Check price at intervals and compare against thresholds:

```bash
PRICE=$(kraken ticker BTCUSD -o json 2>/dev/null | jq -r '.[].last_price')
# Agent compares $PRICE to upper/lower thresholds
# If breached, notify the user
```

## Price Alert (Streaming)

More efficient for continuous monitoring. The agent reads the stream and fires when conditions are met:

```bash
kraken ws ticker BTC/USD -o json 2>/dev/null | while read -r line; do
  LAST=$(echo "$line" | jq -r '.data[0].last // empty')
  [ -z "$LAST" ] && continue
  # Compare against thresholds, break or notify on breach
done
```

## Spread Alert

Detect when the bid-ask spread widens beyond a threshold (liquidity warning):

```bash
kraken ws ticker BTC/USD --event-trigger bbo -o json 2>/dev/null | while read -r line; do
  ASK=$(echo "$line" | jq -r '.data[0].ask // empty')
  BID=$(echo "$line" | jq -r '.data[0].bid // empty')
  [ -z "$ASK" ] || [ -z "$BID" ] && continue
  SPREAD=$(echo "$ASK - $BID" | bc)
  # Alert if spread exceeds threshold
done
```

## Volume Spike Detection

Compare current 24h volume against a baseline:

```bash
kraken ticker BTCUSD -o json 2>/dev/null | jq -r '.[].volume_24h'
# Agent compares to historical average
# Alert if volume > 2x baseline
```

## Volatility Alert (OHLC-Based)

Read recent candles and compute range or standard deviation:

```bash
kraken ohlc BTCUSD --interval 60 -o json 2>/dev/null
# Agent calculates high-low range per candle
# Alert if range exceeds threshold
```

## Balance Change Alert

Monitor for unexpected balance changes:

```bash
INITIAL=$(kraken balance -o json 2>/dev/null | jq -r '.USD // "0"')
# On each check:
CURRENT=$(kraken balance -o json 2>/dev/null | jq -r '.USD // "0"')
# Alert if |CURRENT - INITIAL| exceeds threshold
```

Streaming alternative:

```bash
kraken ws balances -o json 2>/dev/null
# Each line is a balance update event
```

## Fill Notification

Alert on trade executions:

```bash
kraken ws executions -o json 2>/dev/null | while read -r line; do
  TYPE=$(echo "$line" | jq -r '.data[0].exec_type // empty')
  [ "$TYPE" = "trade" ] && echo "Fill: $line"
done
```

## Futures Alerts

Monitor futures positions for margin or P&L thresholds:

```bash
kraken futures positions -o json 2>/dev/null
# Agent checks unrealized P&L against stop-loss threshold
```

Stream futures balance changes:

```bash
kraken futures ws balances -o json 2>/dev/null
```

Futures notifications (margin calls, liquidation warnings):

```bash
kraken futures ws notifications -o json 2>/dev/null
```

## Multi-Pair Alert

Watch several pairs and alert on the first one that hits a condition:

```bash
kraken ws ticker BTC/USD ETH/USD SOL/USD -o json 2>/dev/null | while read -r line; do
  PAIR=$(echo "$line" | jq -r '.data[0].symbol // empty')
  LAST=$(echo "$line" | jq -r '.data[0].last // empty')
  [ -z "$PAIR" ] || [ -z "$LAST" ] && continue
  # Check pair-specific thresholds
done
```

## Notification Delivery

The CLI outputs alerts to stdout. The agent is responsible for delivering notifications through its own channels (Slack, email, push notification, or presenting to the user in chat).

## Pattern Summary

| Condition | Method | Command |
|-----------|--------|---------|
| Price crosses level | Stream | `ws ticker <PAIR>` |
| Spread widens | Stream | `ws ticker <PAIR> --event-trigger bbo` |
| Volume spike | Poll | `ticker <PAIR>` |
| Balance change | Stream | `ws balances` |
| Trade fill | Stream | `ws executions` |
| Futures margin | Stream | `futures ws balances` |
| Futures notification | Stream | `futures ws notifications` |

## Hard Rules

- Alerts are informational; they do not execute trades automatically.
- Prefer streaming over high-frequency polling to reduce API load.
- Close streams when monitoring is no longer needed.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
