---
name: kraken-market-intel
version: 1.0.0
description: "Read market state with low-noise data pulls and streaming updates."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-market-intel

Use this skill to answer questions like:
- "What is the current BTCUSD price?"
- "Is spread widening?"
- "What is short-term trend from candles?"

## Snapshot Pattern

```bash
kraken ticker BTCUSD -o json 2>/dev/null
kraken orderbook BTCUSD --count 10 -o json 2>/dev/null
kraken ohlc BTCUSD --interval 60 -o json 2>/dev/null
```

## Streaming Pattern

```bash
kraken ws ticker BTC/USD -o json 2>/dev/null
```

Parse NDJSON line by line. Do not treat stream output as one JSON object.

## Context Efficiency

- Prefer one pair at a time unless comparison is needed.
- Use `--count` and `--depth` flags to limit payload size.
- Prefer streaming over high-frequency polling.

## Typical Output Use

- Ticker: last price, bid, ask
- Orderbook: near-book depth and imbalance
- OHLC: trend and volatility windows

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
