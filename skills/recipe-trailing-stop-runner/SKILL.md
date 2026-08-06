---
name: recipe-trailing-stop-runner
version: 1.0.0
description: "Ride a trend with a trailing stop that locks in profits on reversal."
metadata:
  openclaw:
    category: "recipe"
    domain: "strategy"
  requires:
    bins: ["kraken"]
    skills: ["kraken-stop-take-profit", "kraken-spot-execution"]
---

# Trailing Stop Runner

> **PREREQUISITE:** Load the following skills to execute this recipe: `kraken-stop-take-profit`, `kraken-spot-execution`

Enter a position and attach a trailing stop to capture trend profits while limiting downside.

## Steps

1. Get current price: `kraken ticker BTCUSD -o json 2>/dev/null`
2. Enter position (requires human approval): `kraken order buy BTCUSD 0.01 --type market -o json 2>/dev/null`
3. Verify fill: `kraken trades-history -o json 2>/dev/null`
4. Set trailing stop (e.g., $500 trail distance): `kraken order sell BTCUSD 0.01 --type trailing-stop --price +500 -o json 2>/dev/null`
5. Verify stop is placed: `kraken open-orders -o json 2>/dev/null`
6. Monitor via stream: `kraken ws ticker BTC/USD -o json 2>/dev/null`
7. When the trailing stop triggers, verify the exit fill: `kraken trades-history -o json 2>/dev/null`
8. Report entry price, exit price, and net P&L

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
