---
name: recipe-paper-strategy-backtest
version: 1.0.0
description: "Backtest a trading strategy using paper trading against live prices."
metadata:
  openclaw:
    category: "recipe"
    domain: "strategy"
  requires:
    bins: ["kraken"]
    skills: ["kraken-paper-strategy"]
---

# Paper Strategy Backtest

> **PREREQUISITE:** Load the following skill to execute this recipe: `kraken-paper-strategy`

Run a strategy through multiple paper runs to validate consistency before live deployment.

## Important: Paper Results May Overstate Live Performance

Paper trading applies configurable fees (default 0.26% taker) and optional flat-rate slippage to market orders. However, it does not simulate partial fills, order book depth, or latency. Live trading on Kraken has variable maker/taker fees by volume tier, depth-dependent slippage, and possible partial fills on limit orders. Enable `--slippage-rate` (e.g., `0.001` for 0.1%) at `workspace create` for more conservative estimates. A strategy that is only marginally profitable on paper with fees and slippage enabled is likely unprofitable live.

## Steps

1. Define strategy parameters (pair, entry/exit rules, position size)
2. Initialize paper account: `kraken workspace create sandbox --capital 10000 --mode paper --slippage-rate 0.001 -o json 2>/dev/null
export KRAKEN_WORKSPACE=sandbox`
3. Round 1: execute strategy logic using paper buy/sell commands
4. Record round 1 results: `kraken workspace status -o json 2>/dev/null` + `kraken paper history -o json 2>/dev/null`
5. Reset: `kraken workspace reset -o json 2>/dev/null`
6. Round 2 with same parameters at a different time (different market conditions)
7. Record round 2 results
8. Repeat for 3-5 rounds minimum
9. Compare results: win rate, average P&L per trade, max drawdown, Sharpe-like ratio
10. If results are consistent and positive, the strategy is a candidate for live promotion
11. If results vary wildly, adjust parameters and re-test

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
