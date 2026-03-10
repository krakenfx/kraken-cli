---
name: kraken-twap-execution
version: 1.0.0
description: "Execute large orders as time-weighted slices to reduce market impact."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
    skills: ["kraken-spot-execution"]
---

# kraken-twap-execution

Use this skill for:
- breaking a large order into smaller time-spaced slices
- reducing market impact and slippage on size
- executing over minutes, hours, or days
- tracking average fill price across slices

## Core Concept

Time-Weighted Average Price (TWAP) splits a large order into N equal slices executed at regular intervals. The goal is to achieve an average price close to the time-weighted market average, reducing the impact a single large order would have on the book.

## Parameters

- **Total volume**: the full amount to buy or sell
- **Slices**: number of child orders (e.g., 10)
- **Interval**: time between slices (e.g., 60s, 300s)
- **Slice volume**: total volume / slices

## Paper TWAP Test

```bash
kraken paper init --balance 50000 -o json 2>/dev/null

# Simulate 5 slices of 0.01 BTC each, 60s apart
kraken paper buy BTCUSD 0.01 -o json 2>/dev/null
# wait 60s
kraken paper buy BTCUSD 0.01 -o json 2>/dev/null
# wait 60s
kraken paper buy BTCUSD 0.01 -o json 2>/dev/null
# repeat...

kraken paper history -o json 2>/dev/null
kraken paper status -o json 2>/dev/null
```

## Live TWAP Loop

The agent runs this loop externally (the CLI does not have a built-in scheduler):

```bash
TOTAL_VOLUME=0.05
SLICES=5
SLICE_VOL=$(echo "scale=8; $TOTAL_VOLUME / $SLICES" | bc)
INTERVAL=60

for i in $(seq 1 $SLICES); do
  kraken order buy BTCUSD $SLICE_VOL --type market -o json 2>/dev/null
  [ $i -lt $SLICES ] && sleep $INTERVAL
done
```

## Limit-Order TWAP Variant

Use limit orders at the current best bid/ask for potentially better fills:

```bash
PRICE=$(kraken ticker BTCUSD -o json 2>/dev/null | jq -r '.[].a[0]')
kraken order buy BTCUSD $SLICE_VOL --type limit --price $PRICE -o json 2>/dev/null
```

Check fill status before the next slice. Cancel unfilled orders and adjust:

```bash
kraken open-orders -o json 2>/dev/null
```

## Tracking Average Fill

After all slices, compute the volume-weighted average price from trade history:

```bash
kraken trades-history --consolidate-taker -o json 2>/dev/null
```

Sum (price * volume) for each fill, divide by total volume filled.

## Rate Limit Awareness

The CLI does not pre-throttle requests. If a slice submission hits a rate limit, the error includes a `suggestion` field with tier-specific limits and a `docs_url` pointing to Kraken's documentation. On `rate_limit` error, pause the loop, read the suggestion, and adjust the interval before resuming. A 60-second interval between slices is well within budget for all tiers. For shorter intervals, consult the `kraken-rate-limits` skill for per-tier counter costs and decay rates.

## Hard Rules

- Each live slice requires human approval unless operating at autonomy level 4+.
- Track cumulative fill volume and stop if total exceeds target (handle partial fills).
- On error, pause the loop rather than skipping the slice; resume after recovery.
- Log every slice for post-execution analysis.
