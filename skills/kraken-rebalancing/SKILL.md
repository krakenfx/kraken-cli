---
name: kraken-rebalancing
version: 1.0.0
description: "Portfolio rebalancing to maintain target allocations across assets."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-rebalancing

Use this skill for:
- checking current portfolio allocation vs target weights
- calculating rebalance trades
- executing rebalance orders with minimum trade thresholds
- scheduling periodic rebalance checks

## Define Target Allocation

Example target: 50% BTC, 30% ETH, 20% SOL (by USD value).

The agent maintains these weights and compares against current holdings.

## Read Current Portfolio

1. Get balances:
   ```bash
   kraken balance -o json 2>/dev/null
   ```
2. Get current prices for all held assets:
   ```bash
   kraken ticker BTCUSD ETHUSD SOLUSD -o json 2>/dev/null
   ```
3. Calculate USD value of each holding and total portfolio value.
4. Compute current weights: asset_value / total_value for each.

## Calculate Rebalance Trades

For each asset:
- Target value = total_portfolio * target_weight
- Current value = holdings * current_price
- Delta = target_value - current_value
- If delta > threshold: buy (delta / price) units
- If delta < -threshold: sell (|delta| / price) units

Set a minimum trade threshold (e.g., $50) to avoid placing tiny orders that waste fees.

## Paper Rebalance Test

```bash
kraken workspace create sandbox --capital 10000 --mode paper -o json 2>/dev/null
export KRAKEN_WORKSPACE=sandbox

# Initial buys to establish positions
kraken paper buy BTCUSD 0.05 -o json 2>/dev/null
kraken paper buy ETHUSD 1.0 -o json 2>/dev/null
kraken paper buy SOLUSD 10 -o json 2>/dev/null

kraken workspace status -o json 2>/dev/null

# After price drift, rebalance
# Sell overweight asset, buy underweight asset
kraken paper sell BTCUSD 0.005 -o json 2>/dev/null
kraken paper buy SOLUSD 2 -o json 2>/dev/null

kraken workspace status -o json 2>/dev/null
```

## Live Rebalance Execution

1. Calculate all required trades.
2. Present the rebalance plan to the user:
   - Current allocation vs target
   - Proposed trades with estimated fees
3. Validate each order:
   ```bash
   kraken order sell BTCUSD 0.005 --type market --validate -o json 2>/dev/null
   kraken order buy SOLUSD 2 --type market --validate -o json 2>/dev/null
   ```
4. Execute sells first (to free capital), then buys (requires human approval):
   ```bash
   kraken order sell BTCUSD 0.005 --type market -o json 2>/dev/null
   kraken order buy SOLUSD 2 --type market -o json 2>/dev/null
   ```
5. Verify final state:
   ```bash
   kraken balance -o json 2>/dev/null
   ```

## Drift Detection

Periodically compare current weights to targets. Trigger rebalance when any asset drifts beyond a threshold (e.g., 5% absolute drift):

```bash
# Agent checks balance and prices on a schedule
# If |current_weight - target_weight| > 0.05 for any asset → rebalance
```

## Rebalance with Limit Orders

For lower fees, use limit orders instead of market:

```bash
PRICE=$(kraken ticker SOLUSD -o json 2>/dev/null | jq -r '.[].ask_price')
kraken order buy SOLUSD 2 --type limit --price $PRICE -o json 2>/dev/null
```

Check fills before declaring rebalance complete:

```bash
kraken open-orders -o json 2>/dev/null
```

## Hard Rules

- Never execute rebalance trades without explicit human approval.
- Always present the full rebalance plan (sells and buys) before execution.
- Execute sells before buys to ensure sufficient quote currency.
- Respect minimum order sizes for each pair (check pair info with `kraken pairs`).
- Apply a minimum trade threshold to skip negligible adjustments.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
