---
name: recipe-daily-pnl-report
version: 1.0.0
description: "Generate a daily profit and loss summary from trades and balances."
metadata:
  openclaw:
    category: "recipe"
    domain: "portfolio"
  requires:
    bins: ["kraken"]
    skills: ["kraken-portfolio-intel"]
---

# Daily P&L Report

> **PREREQUISITE:** Load the following skill to execute this recipe: `kraken-portfolio-intel`

Generate a daily summary of trading activity, fees, and portfolio change.

## Steps

1. Compute today's midnight timestamp: `TS=$(date -j -f '%Y-%m-%d' "$(date +%Y-%m-%d)" +%s 2>/dev/null || date -d 'today 00:00' +%s)`
2. Get current balances: `kraken balance -o json 2>/dev/null`
3. Get current prices: `kraken ticker BTCUSD ETHUSD SOLUSD -o json 2>/dev/null`
4. Calculate total portfolio value in USD
5. Get today's trades: `kraken trades-history --consolidate-taker -o json 2>/dev/null`
6. Get today's ledger entries: `kraken ledgers --start $TS -o json 2>/dev/null`
7. Calculate realized P&L from closed trades
8. Calculate fees paid today (sum of fee fields from trades)
9. Check futures positions if applicable: `kraken futures positions -o json 2>/dev/null`
10. Check futures funding accrued: `kraken futures accounts -o json 2>/dev/null`
11. Present summary: total value, daily change, realized P&L, fees, open positions

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
