---
name: recipe-start-dca-bot
version: 1.0.0
description: "Set up and run a dollar cost averaging bot from paper test to live."
metadata:
  openclaw:
    category: "recipe"
    domain: "strategy"
  requires:
    bins: ["kraken"]
    skills: ["kraken-dca-strategy", "kraken-paper-to-live"]
---

# Start a DCA Bot

> **PREREQUISITE:** Load the following skills to execute this recipe: `kraken-dca-strategy`, `kraken-paper-to-live`

Set up a dollar cost averaging bot that buys a fixed dollar amount of BTC at regular intervals.

> **CAUTION:** Live orders spend real money. Paper-test first, then promote with explicit approval.

## Steps

1. Initialize paper account: `kraken workspace create sandbox --capital 10000 --mode paper -o json 2>/dev/null
export KRAKEN_WORKSPACE=sandbox`
2. Calculate volume from dollar amount: `PRICE=$(kraken ticker BTCUSD -o json 2>/dev/null | jq -r '.[].last_price') && VOLUME=$(echo "scale=8; 100 / $PRICE" | bc)`
3. Run 5 paper DCA buys to validate the loop: `kraken paper buy BTCUSD $VOLUME -o json 2>/dev/null`
4. Check paper results: `kraken workspace status -o json 2>/dev/null`
5. Review paper trade history: `kraken paper history -o json 2>/dev/null`
6. Verify live credentials: `kraken auth test -o json 2>/dev/null`
7. Check live balance: `kraken balance -o json 2>/dev/null`
8. Validate the live order: `kraken order buy BTCUSD $VOLUME --type market --validate -o json 2>/dev/null`
9. Execute first live buy (requires human approval): `kraken order buy BTCUSD $VOLUME --type market -o json 2>/dev/null`
10. Verify fill: `kraken trades-history -o json 2>/dev/null`

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
