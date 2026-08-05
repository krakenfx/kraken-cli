---
name: recipe-morning-market-brief
version: 1.0.0
description: "Generate a morning market summary with prices, volume, and portfolio state."
metadata:
  openclaw:
    category: "recipe"
    domain: "market-data"
  requires:
    bins: ["kraken"]
    skills: ["kraken-market-intel", "kraken-portfolio-intel"]
---

# Morning Market Brief

> **PREREQUISITE:** Load the following skills to execute this recipe: `kraken-market-intel`, `kraken-portfolio-intel`

Generate a concise morning summary covering market conditions and portfolio state.

## Steps

1. Check system status: `kraken status -o json 2>/dev/null`
2. Get prices for core watchlist: `kraken ticker BTCUSD ETHUSD SOLUSD -o json 2>/dev/null`
3. Get 24h OHLC for trend context: `kraken ohlc BTCUSD --interval 1440 -o json 2>/dev/null`
4. Check portfolio balances: `kraken balance -o json 2>/dev/null`
5. Check open orders: `kraken open-orders -o json 2>/dev/null`
6. Check futures positions if applicable: `kraken futures positions -o json 2>/dev/null`
7. Check earn allocations: `kraken earn allocations --hide-zero-allocations -o json 2>/dev/null`
8. Present summary: price table, 24h change, portfolio value, open orders count, position P&L

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
