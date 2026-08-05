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

Paper trading runs locally against live market prices. It simulates fees and slippage but has other limitations.

- **Fees are configurable.** A 0.26% taker fee (Kraken Starter tier) is applied by default. Override with `--fee-rate` at `workspace create` (fixed for the account's life). Real fees vary by volume tier and maker/taker status. Live maker fees are lower (0.16%).
- **Slippage is configurable.** A flat slippage rate can be applied to market orders with `--slippage-rate` (default: 0.0). Market buys fill at `ask * (1 + slippage_rate)`, sells at `bid * (1 - slippage_rate)`. This is a conservative approximation; real slippage depends on order book depth, size, and volatility.
- **No partial fills or rejection.** Paper orders always fill in full immediately. Live orders may partially fill, queue, or be rejected.

When presenting paper results to the user, note that partial fills and order book dynamics are not simulated. If slippage is set to 0.0 (default), remind the user that live market orders will fill at worse prices.

## Baseline Workflow

```bash
kraken workspace create sandbox --capital 10000 --mode paper --currency USD --slippage-rate 0.001 -o json 2>/dev/null
export KRAKEN_WORKSPACE=sandbox
kraken paper buy BTCUSD 0.01 -o json 2>/dev/null
kraken workspace status -o json 2>/dev/null
kraken paper sell BTCUSD 0.005 --type limit --price 70000 -o json 2>/dev/null
kraken paper orders -o json 2>/dev/null
kraken paper history -o json 2>/dev/null
```

## Reset Between Runs

```bash
kraken workspace reset -o json --yes 2>/dev/null
```

## Migration Rule

Only move a strategy to live trading after:
1. repeated paper runs with stable behavior
2. explicit user sign-off
3. `--validate` checks pass for live order payloads

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
