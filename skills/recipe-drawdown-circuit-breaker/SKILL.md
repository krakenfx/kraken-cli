---
name: recipe-drawdown-circuit-breaker
version: 1.0.0
description: "Automatically stop trading when portfolio drawdown exceeds a threshold."
metadata:
  openclaw:
    category: "recipe"
    domain: "risk"
  requires:
    bins: ["kraken"]
    skills: ["kraken-risk-operations", "kraken-alert-patterns"]
---

# Drawdown Circuit Breaker

> **PREREQUISITE:** Load the following skills to execute this recipe: `kraken-risk-operations`, `kraken-alert-patterns`

Monitor portfolio value and halt trading if cumulative drawdown from peak exceeds a limit (e.g., 10%).

## Steps

1. Record starting portfolio value: `kraken balance -o json 2>/dev/null` + `kraken ticker BTCUSD ETHUSD SOLUSD -o json 2>/dev/null` (calculate total USD value)
2. Set this as the high-water mark
3. On each check interval (at least 5 seconds; see `kraken-rate-limits` for tier budgets):
   - Recalculate total portfolio value
   - Update high-water mark if value exceeds previous peak
   - Calculate drawdown: (peak - current) / peak * 100
4. If drawdown exceeds threshold (e.g., 10%):
   - Alert the user immediately
   - Cancel all open orders: `kraken order cancel-all -o json 2>/dev/null`
   - Cancel all futures orders: `kraken futures cancel-all -o json 2>/dev/null`
5. Present drawdown report: peak value, current value, drawdown percentage, orders cancelled
6. Cancel operations require explicit human approval unless operating at autonomy level 4+
7. Wait for explicit user instruction before resuming any trading activity

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
