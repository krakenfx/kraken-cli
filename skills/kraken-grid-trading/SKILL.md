---
name: kraken-grid-trading
version: 1.0.0
description: "Grid trading strategy with layered buy and sell orders across a price range."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-grid-trading

Use this skill for:
- placing a grid of limit orders across a price range
- profiting from sideways/ranging markets
- managing grid state (filled orders, replacements)
- paper-testing grid parameters before live deployment

## Core Concept

Grid trading places buy orders below the current price and sell orders above it at fixed intervals. When a buy fills, a corresponding sell is placed one grid level higher. When a sell fills, a corresponding buy is placed one grid level lower. Profit comes from capturing the spread at each level.

## Grid Parameters

Define before starting:
- **Pair**: e.g., BTCUSD
- **Range**: lower bound to upper bound (e.g., 55000-65000)
- **Grid levels**: number of orders (e.g., 10)
- **Order size**: volume per grid level (e.g., 0.001 BTC)

Grid spacing = (upper - lower) / grid levels.

## Paper Grid Test

Always test in paper mode first:

```bash
kraken workspace create sandbox --capital 10000 --mode paper -o json 2>/dev/null
export KRAKEN_WORKSPACE=sandbox

# Place buy grid below current price
kraken paper buy BTCUSD 0.001 --type limit --price 58000 -o json 2>/dev/null
kraken paper buy BTCUSD 0.001 --type limit --price 57000 -o json 2>/dev/null
kraken paper buy BTCUSD 0.001 --type limit --price 56000 -o json 2>/dev/null

# Place sell grid above current price
kraken paper sell BTCUSD 0.001 --type limit --price 62000 -o json 2>/dev/null
kraken paper sell BTCUSD 0.001 --type limit --price 63000 -o json 2>/dev/null
kraken paper sell BTCUSD 0.001 --type limit --price 64000 -o json 2>/dev/null

kraken paper orders -o json 2>/dev/null
kraken workspace status -o json 2>/dev/null
```

## Live Grid Placement

1. Read current price to center the grid:
   ```bash
   kraken ticker BTCUSD -o json 2>/dev/null
   ```
2. Calculate grid levels (agent logic).
3. Validate each order:
   ```bash
   kraken order buy BTCUSD 0.001 --type limit --price 58000 --validate -o json 2>/dev/null
   ```
4. Place orders (requires human approval for the full grid):
   ```bash
   kraken order buy BTCUSD 0.001 --type limit --price 58000 -o json 2>/dev/null
   kraken order buy BTCUSD 0.001 --type limit --price 57000 -o json 2>/dev/null
   # ... remaining grid levels
   ```
5. Use batch orders when placing 2-15 orders:
   ```bash
   kraken order batch grid-orders.json --pair BTCUSD --validate -o json 2>/dev/null
   kraken order batch grid-orders.json --pair BTCUSD -o json 2>/dev/null
   ```

## Grid Maintenance Loop

Monitor fills and replace completed orders:

1. Check open orders:
   ```bash
   kraken open-orders -o json 2>/dev/null
   ```
2. Check recent trades for fills:
   ```bash
   kraken trades-history -o json 2>/dev/null
   ```
3. For each filled buy at level N, place a sell at level N+1.
4. For each filled sell at level N, place a buy at level N-1.

Stream executions for real-time fill detection:

```bash
kraken ws executions -o json 2>/dev/null
```

## Grid Shutdown

Cancel all grid orders cleanly:

```bash
kraken order cancel-all -o json 2>/dev/null
```

Or cancel specific orders by TXID:

```bash
kraken order cancel-batch <TXID1> <TXID2> <TXID3> -o json 2>/dev/null
```

## Risk Considerations

- Grid trading profits in ranging markets but loses in strong trends.
- If price moves below the entire grid, the agent accumulates the asset at a loss.
- If price moves above the entire grid, the agent sells all holdings and misses further upside.
- Set stop-loss boundaries outside the grid range to limit downside.

## Hard Rules

- Never place a live grid without explicit human approval.
- Paper-test every grid configuration before live deployment.
- Enable `cancel-after` for unattended grid sessions.
- Track total grid P&L, not just individual level fills.
- Cancel the entire grid before adjusting parameters; do not leave orphaned orders.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
