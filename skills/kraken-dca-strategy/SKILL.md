---
name: kraken-dca-strategy
version: 1.0.0
description: "Dollar cost averaging with scheduled buys and performance tracking."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-dca-strategy

Use this skill for:
- implementing a recurring buy strategy at fixed intervals
- tracking average cost basis over time
- comparing DCA performance against lump-sum entry
- running DCA simulations in paper mode first

## Core Concept

Dollar cost averaging buys a fixed dollar amount of an asset at regular intervals regardless of price. This reduces timing risk and smooths entry cost over volatile periods.

## Paper DCA Loop (Test First)

Always validate the strategy in paper mode before going live.

The `paper buy` command takes base-asset volume, not dollar amount. Calculate volume first:

```bash
kraken workspace create sandbox --capital 10000 --mode paper --currency USD -o json 2>/dev/null
export KRAKEN_WORKSPACE=sandbox

# Calculate BTC volume for a $100 buy at current price
PRICE=$(kraken ticker BTCUSD -o json 2>/dev/null | jq -r '.[].last_price')
VOLUME=$(echo "scale=8; 100 / $PRICE" | bc)

# Simulate weekly buys
kraken paper buy BTCUSD $VOLUME -o json 2>/dev/null
kraken workspace status -o json 2>/dev/null

# Repeat buy, check status each iteration
kraken paper buy BTCUSD $VOLUME -o json 2>/dev/null
kraken workspace status -o json 2>/dev/null

kraken paper history -o json 2>/dev/null
```

## Live DCA Single Buy

Each interval, the agent executes one market buy for the fixed amount:

1. Check current price:
   ```bash
   kraken ticker BTCUSD -o json 2>/dev/null
   ```
2. Calculate volume from dollar amount (e.g., $100 at current price):
   ```bash
   PRICE=$(kraken ticker BTCUSD -o json 2>/dev/null | jq -r '.[].last_price')
   VOLUME=$(echo "scale=8; 100 / $PRICE" | bc)
   ```
3. Validate the order:
   ```bash
   kraken order buy BTCUSD $VOLUME --type market --validate -o json 2>/dev/null
   ```
4. Execute (requires human approval):
   ```bash
   kraken order buy BTCUSD $VOLUME --type market -o json 2>/dev/null
   ```
5. Log the trade for cost basis tracking.

## Cost Basis Tracking

After each buy, query trade history to compute running average:

```bash
kraken trades-history --consolidate-taker -o json 2>/dev/null
```

The agent should maintain a running total:
- Total invested (sum of all dollar amounts)
- Total units acquired (sum of all volumes)
- Average cost = total invested / total units
- Current value = total units * current price
- Unrealized P&L = current value - total invested

## Limit-Order DCA Variant

Instead of market buys, place limit orders slightly below the current price for better fills:

```bash
PRICE=$(kraken ticker BTCUSD -o json 2>/dev/null | jq -r '.[].bid_price')
LIMIT=$(echo "scale=2; $PRICE * 0.998" | bc)
VOLUME=$(echo "scale=8; 100 / $LIMIT" | bc)
kraken order buy BTCUSD $VOLUME --type limit --price $LIMIT -o json 2>/dev/null
```

Check fill status at the next interval. Cancel unfilled orders before placing new ones:

```bash
kraken open-orders -o json 2>/dev/null
kraken order cancel <UNFILLED_TXID> -o json 2>/dev/null
```

## Multi-Asset DCA

Split a fixed budget across multiple assets (e.g., 60% BTC, 30% ETH, 10% SOL):

```bash
# $100 total: $60 BTC, $30 ETH, $10 SOL
# Calculate volumes for each, then place orders sequentially
```

## Scheduling

The CLI does not include a built-in scheduler. Agents should use external scheduling (cron, task scheduler, or the agent's own loop timer) to trigger DCA buys at the chosen interval (daily, weekly, bi-weekly, monthly).

## Hard Rules

- Never execute live DCA buys without explicit human approval for the strategy and per-trade confirmation (unless operating at autonomy level 4+).
- Always paper-test the DCA loop first.
- Track cost basis after every buy; do not lose history.
- Cancel stale limit orders before placing new ones to avoid duplicate exposure.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
