---
name: recipe-launch-grid-bot
version: 1.0.0
description: "Deploy a grid trading bot with paper validation and live safety controls."
metadata:
  openclaw:
    category: "recipe"
    domain: "strategy"
  requires:
    bins: ["kraken"]
    skills: ["kraken-grid-trading", "kraken-paper-to-live"]
---

# Launch a Grid Bot

> **PREREQUISITE:** Load the following skills to execute this recipe: `kraken-grid-trading`, `kraken-paper-to-live`

Deploy a grid of buy and sell orders across a price range.

> **CAUTION:** Grids profit in ranging markets but lose in strong trends. Always paper-test first.

## Steps

1. Get current price: `PRICE=$(kraken ticker BTCUSD -o json 2>/dev/null | jq -r '.[].last_price')`
2. Compute grid levels (e.g., 3 levels at $1000 spacing): `BUY1=$(echo "$PRICE - 1000" | bc)`, `BUY2=$(echo "$PRICE - 2000" | bc)`, `BUY3=$(echo "$PRICE - 3000" | bc)`, `SELL1=$(echo "$PRICE + 1000" | bc)`, `SELL2=$(echo "$PRICE + 2000" | bc)`, `SELL3=$(echo "$PRICE + 3000" | bc)`
3. Paper-test the grid in its own workspace: `kraken workspace create sandbox --capital 10000 --mode paper -o json 2>/dev/null`
4. Place paper buy levels: `kraken --workspace sandbox paper buy BTCUSD 0.001 --type limit --price $BUY1 -o json 2>/dev/null` (repeat for BUY2, BUY3)
5. Place paper sell levels: `kraken --workspace sandbox paper sell BTCUSD 0.001 --type limit --price $SELL1 -o json 2>/dev/null` (repeat for SELL2, SELL3)
6. Monitor paper fills: `kraken --workspace sandbox paper orders -o json 2>/dev/null`
7. Review paper results: `kraken workspace status sandbox -o json 2>/dev/null`
8. Run the live steps with no workspace active: `unset KRAKEN_WORKSPACE` in the shell that runs them, or prefix each live command with `KRAKEN_WORKSPACE=`. Inside a paper workspace, `balance` reads the paper ledger, `order cancel-after` is refused, and `order buy` fills on paper.
9. Pre-flight live: `kraken auth test -o json 2>/dev/null && kraken balance -o json 2>/dev/null`
10. Enable dead man's switch: `kraken order cancel-after 3600 -o json 2>/dev/null`
11. Validate live grid orders: `kraken order buy BTCUSD 0.001 --type limit --price $BUY1 --validate -o json 2>/dev/null`
12. Place live grid (requires human approval): place buy and sell levels sequentially using $BUY1-3 and $SELL1-3
13. Monitor fills: `kraken ws executions -o json 2>/dev/null`
14. Replace filled levels (requires human approval): when a buy fills, place a sell one level up, and vice versa

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
