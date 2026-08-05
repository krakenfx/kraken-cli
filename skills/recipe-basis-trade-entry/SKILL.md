---
name: recipe-basis-trade-entry
version: 1.0.0
description: "Enter a spot-futures basis trade when the premium exceeds a target threshold."
metadata:
  openclaw:
    category: "recipe"
    domain: "strategy"
  requires:
    bins: ["kraken"]
    skills: ["kraken-basis-trading", "kraken-futures-risk"]
---

# Basis Trade Entry

> **PREREQUISITE:** Load the following skills to execute this recipe: `kraken-basis-trading`, `kraken-futures-risk`

Enter a delta-neutral basis trade (long spot, short futures) when the basis premium is attractive.

> **CAUTION:** Both legs must fill to be market-neutral. A single-leg fill is a directional bet.

## Steps

1. Get spot price: `SPOT=$(kraken ticker BTCUSD -o json 2>/dev/null | jq -r '.[].last_price')`
2. Get futures price: `FUTURES=$(kraken futures ticker PF_XBTUSD -o json 2>/dev/null | jq -r '.ticker.last')`
3. Calculate basis: `BASIS=$(echo "scale=4; ($FUTURES - $SPOT) / $SPOT * 100" | bc)`
4. If basis < target threshold (e.g., 0.5% annualized), skip
5. Check spot balance for capital: `kraken balance -o json 2>/dev/null`
6. Check futures margin availability: `kraken futures accounts -o json 2>/dev/null`
7. Calculate matched position sizes
8. Execute spot buy (requires human approval): `kraken order buy BTCUSD 0.01 --type limit --price $SPOT -o json 2>/dev/null`
9. Execute futures short: `kraken futures order sell PF_XBTUSD 1 --type limit --price $FUTURES -o json 2>/dev/null`
10. Verify both legs filled: `kraken open-orders -o json 2>/dev/null` + `kraken futures open-orders -o json 2>/dev/null`
11. If one leg unfilled, cancel it to avoid directional exposure

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
