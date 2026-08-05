---
name: recipe-portfolio-snapshot-csv
version: 1.0.0
description: "Export a portfolio snapshot with balances and valuations to CSV."
metadata:
  openclaw:
    category: "recipe"
    domain: "portfolio"
  requires:
    bins: ["kraken"]
    skills: ["kraken-portfolio-intel"]
---

# Portfolio Snapshot to CSV

> **PREREQUISITE:** Load the following skill to execute this recipe: `kraken-portfolio-intel`

Capture a point-in-time portfolio snapshot combining balances, prices, and earn allocations.

## Steps

1. Get all balances: `kraken balance -o json 2>/dev/null`
2. Identify held assets (non-zero balances)
3. Get prices for all held assets: `kraken ticker BTCUSD ETHUSD SOLUSD -o json 2>/dev/null`
4. Get earn allocations: `kraken earn allocations --hide-zero-allocations --converted-asset USD -o json 2>/dev/null`
5. Calculate USD value per asset
6. Check futures positions: `kraken futures positions -o json 2>/dev/null`
7. Format as CSV: `asset, balance, price_usd, value_usd, earn_allocated, total_exposure`
8. Output to stdout or write to file
9. Include timestamp and total portfolio value as header row

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
