---
name: recipe-weekly-rebalance
version: 1.0.0
description: "Run a weekly portfolio rebalance to maintain target asset allocations."
metadata:
  openclaw:
    category: "recipe"
    domain: "portfolio"
  requires:
    bins: ["kraken"]
    skills: ["kraken-rebalancing"]
---

# Weekly Rebalance

> **PREREQUISITE:** Load the following skill to execute this recipe: `kraken-rebalancing`

Check portfolio drift and rebalance to target weights once per week.

## Steps

1. Get balances: `kraken balance -o json 2>/dev/null`
2. Get prices: `kraken ticker BTCUSD ETHUSD SOLUSD -o json 2>/dev/null`
3. Calculate current weights (agent computes USD value per asset / total)
4. Compare to targets (e.g., 50% BTC, 30% ETH, 20% SOL)
5. If any asset drifts more than 5% from target, compute rebalance trades: `SELL_VOL=$(echo "scale=8; ($CURRENT_BTC_VALUE - $TARGET_BTC_VALUE) / $BTC_PRICE" | bc)` and `BUY_VOL=$(echo "scale=8; ($TARGET_SOL_VALUE - $CURRENT_SOL_VALUE) / $SOL_PRICE" | bc)`
6. Present the rebalance plan to the user with estimated fees
7. Validate sell orders: `kraken order sell BTCUSD $SELL_VOL --type market --validate -o json 2>/dev/null`
8. Execute sells first (requires human approval): `kraken order sell BTCUSD $SELL_VOL --type market -o json 2>/dev/null`
9. Execute buys with freed capital: `kraken order buy SOLUSD $BUY_VOL --type market -o json 2>/dev/null`
10. Verify final allocations: `kraken balance -o json 2>/dev/null`

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
