---
name: recipe-track-orderbook-depth
version: 1.0.0
description: "Monitor order book depth and bid-ask imbalance for liquidity signals."
metadata:
  openclaw:
    category: "recipe"
    domain: "market-data"
  requires:
    bins: ["kraken"]
    skills: ["kraken-market-intel", "kraken-ws-streaming"]
---

# Track Order Book Depth

> **PREREQUISITE:** Load the following skills to execute this recipe: `kraken-market-intel`, `kraken-ws-streaming`

Monitor order book depth for a pair and detect bid-ask imbalance as a liquidity signal.

## Steps

1. Get a snapshot: `kraken orderbook BTCUSD --count 25 -o json 2>/dev/null`
2. Sum bid volume (top 10 levels) and ask volume (top 10 levels)
3. Calculate imbalance ratio: bid_volume / (bid_volume + ask_volume)
   - Ratio > 0.6: strong bid-side (buying pressure)
   - Ratio < 0.4: strong ask-side (selling pressure)
4. Start streaming for continuous monitoring: `kraken ws book BTC/USD --depth 10 -o json 2>/dev/null`
5. Parse each update and recalculate imbalance
6. Alert when imbalance crosses threshold
7. Optionally compare across pairs (one call per pair): `kraken orderbook BTCUSD --count 10 -o json 2>/dev/null` then `kraken orderbook ETHUSD --count 10 -o json 2>/dev/null`

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
