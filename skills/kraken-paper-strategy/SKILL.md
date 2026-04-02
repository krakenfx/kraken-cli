---
name: kraken-paper-strategy
version: 1.0.0
description: "Test strategy logic on paper trading before touching live funds."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-paper-strategy

Use this skill for:
- validating entry and exit logic on spot markets
- testing position sizing and simple rebalance loops
- rehearsing error handling without risk

For futures paper trading (leverage, margin, shorts, liquidation), see `kraken futures paper` commands. This skill covers spot paper trading only.

## Limitations

Paper trading runs locally against live market prices. A 0.26% taker fee (Kraken Starter tier default) is applied to every fill, but slippage and partial fills are not modeled.

- **Fees included.** A 0.26% taker fee is deducted from collateral on each fill. Live maker fees are lower (0.16%), so limit orders that provide liquidity on a live exchange cost less than paper predicts.
- **No slippage.** Paper orders fill at the exact quoted price. Live market orders can fill at worse prices, especially for larger sizes or thin order books.
- **No partial fills or rejection.** Paper orders always fill in full immediately. Live orders may partially fill, queue, or be rejected.

When presenting paper results to the user, note that slippage is not modeled and live performance will differ. The fee deduction is already reflected in paper P&L.

## Baseline Workflow

```bash
kraken paper init --balance 10000 --currency USD -o json 2>/dev/null
kraken paper buy BTCUSD 0.01 -o json 2>/dev/null
kraken paper status -o json 2>/dev/null
kraken paper sell BTCUSD 0.005 --type limit --price 70000 -o json 2>/dev/null
kraken paper orders -o json 2>/dev/null
kraken paper history -o json 2>/dev/null
```

## Reset Between Runs

```bash
kraken paper reset -o json 2>/dev/null
```

## Migration Rule

Only move a strategy to live trading after:
1. repeated paper runs with stable behavior
2. explicit user sign-off
3. `--validate` checks pass for live order payloads
