---
name: recipe-futures-hedge-spot
version: 1.0.0
description: "Hedge a spot holding with a short futures position to lock in value."
metadata:
  openclaw:
    category: "recipe"
    domain: "strategy"
  requires:
    bins: ["kraken"]
    skills: ["kraken-spot-execution", "kraken-futures-trading", "kraken-futures-risk"]
---

# Hedge Spot with Futures

> **PREREQUISITE:** Load the following skills to execute this recipe: `kraken-spot-execution`, `kraken-futures-trading`, `kraken-futures-risk`

Protect a spot BTC holding from downside by shorting an equivalent futures position.

> **CAUTION:** This locks in current value but also caps upside. Futures margin requirements apply.

## Steps

1. Check spot balance: `kraken balance -o json 2>/dev/null`
2. Get spot price: `kraken ticker BTCUSD -o json 2>/dev/null`
3. Get futures price: `kraken futures ticker PF_XBTUSD -o json 2>/dev/null`
4. Check futures margin availability: `kraken futures accounts -o json 2>/dev/null`
5. Calculate matching futures size: `BTC_BAL=$(kraken balance -o json 2>/dev/null | jq -r '.BTC // "0"')` and `FUT_PRICE=$(kraken futures ticker PF_XBTUSD -o json 2>/dev/null | jq -r '.ticker.last')`
6. Open short futures position (requires human approval): `kraken futures order sell PF_XBTUSD $BTC_BAL --type limit --price $FUT_PRICE -o json 2>/dev/null`
7. Verify the position: `kraken futures positions -o json 2>/dev/null`
8. Enable dead man's switch: `kraken futures cancel-after 3600 -o json 2>/dev/null`
9. Monitor margin: `kraken futures accounts -o json 2>/dev/null`
10. To remove hedge, close futures: `kraken futures order buy PF_XBTUSD $BTC_BAL --reduce-only -o json 2>/dev/null`

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
