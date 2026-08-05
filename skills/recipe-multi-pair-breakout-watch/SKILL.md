---
name: recipe-multi-pair-breakout-watch
version: 1.0.0
description: "Monitor multiple pairs for price breakouts from defined ranges."
metadata:
  openclaw:
    category: "recipe"
    domain: "market-data"
  requires:
    bins: ["kraken"]
    skills: ["kraken-multi-pair", "kraken-alert-patterns"]
---

# Multi-Pair Breakout Watch

> **PREREQUISITE:** Load the following skills to execute this recipe: `kraken-multi-pair`, `kraken-alert-patterns`

Watch a set of pairs for breakouts above resistance or below support levels.

## Steps

1. Define watchlist pairs and levels (agent maintains these):
   - BTCUSD: support 55000, resistance 65000
   - ETHUSD: support 2800, resistance 3500
   - SOLUSD: support 80, resistance 120
2. Get baseline OHLC for context: `kraken ohlc BTCUSD --interval 60 -o json 2>/dev/null`
3. Start streaming: `kraken ws ticker BTC/USD ETH/USD SOL/USD -o json 2>/dev/null`
4. Parse each NDJSON line: extract pair and last price
5. Compare last price to support/resistance levels
6. On breakout detection: alert the user with pair, direction, price, and level crossed
7. Optionally check volume to confirm breakout strength: `kraken ticker BTCUSD ETHUSD SOLUSD -o json 2>/dev/null`
8. Present breakout candidates ranked by volume confirmation

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
