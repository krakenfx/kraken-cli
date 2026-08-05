---
name: recipe-price-level-alerts
version: 1.0.0
description: "Set up price level alerts that notify when key levels are crossed."
metadata:
  openclaw:
    category: "recipe"
    domain: "market-data"
  requires:
    bins: ["kraken"]
    skills: ["kraken-alert-patterns"]
---

# Price Level Alerts

> **PREREQUISITE:** Load the following skill to execute this recipe: `kraken-alert-patterns`

Define upper and lower price levels for a pair and get notified when they are crossed.

## Steps

1. Get current price for reference: `kraken ticker BTCUSD -o json 2>/dev/null`
2. Define alert levels:
   - Upper: e.g., 70000 (breakout)
   - Lower: e.g., 55000 (breakdown)
3. Start streaming: `kraken ws ticker BTC/USD --event-trigger bbo -o json 2>/dev/null`
4. Parse each line: extract last price
5. Compare against upper level: if price >= upper, alert "BTCUSD broke above 70000"
6. Compare against lower level: if price <= lower, alert "BTCUSD broke below 55000"
7. After alert fires, optionally stop monitoring or set new levels
8. Agent delivers alert through its notification channel (chat, Slack, etc.)

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
