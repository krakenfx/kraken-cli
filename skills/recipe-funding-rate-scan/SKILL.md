---
name: recipe-funding-rate-scan
version: 1.0.0
description: "Scan perpetual contracts for attractive funding rate carry opportunities."
metadata:
  openclaw:
    category: "recipe"
    domain: "strategy"
  requires:
    bins: ["kraken"]
    skills: ["kraken-funding-carry", "kraken-futures-risk"]
---

# Funding Rate Scan

> **PREREQUISITE:** Load the following skills to execute this recipe: `kraken-funding-carry`, `kraken-futures-risk`

Scan available perpetual contracts, identify those with high funding rates, and evaluate carry potential.

## Steps

1. List all futures contracts: `kraken futures instruments -o json 2>/dev/null`
2. Get all tickers: `kraken futures tickers -o json 2>/dev/null`
3. Check funding rates for top-volume contracts:
   - `kraken futures historical-funding-rates PF_XBTUSD -o json 2>/dev/null`
   - `kraken futures historical-funding-rates PF_ETHUSD -o json 2>/dev/null`
   - `kraken futures historical-funding-rates PF_SOLUSD -o json 2>/dev/null`
4. Rank by annualized yield (rate * periods_per_year * 100)
5. Filter for consistently positive or negative rates (avoid noisy oscillations)
6. Check spot price for hedge calculation: `kraken ticker BTCUSD ETHUSD SOLUSD -o json 2>/dev/null`
7. Check futures account margin: `kraken futures accounts -o json 2>/dev/null`
8. Present ranked opportunities with annualized yield, direction, and required margin

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
