---
name: kraken-basis-trading
version: 1.0.0
description: "Capture the spot-futures price spread with delta-neutral basis trades."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
    skills: ["kraken-spot-execution", "kraken-futures-trading"]
---

# kraken-basis-trading

Use this skill for:
- identifying positive basis (futures premium over spot)
- entering delta-neutral positions (long spot, short futures)
- monitoring basis convergence
- closing both legs when basis narrows or contracts expire

## Core Concept

When futures trade at a premium to spot (positive basis), you can buy spot and sell futures for the same notional amount. The profit comes from basis convergence as the contract approaches expiry, or from collecting funding on perpetuals. The position is market-neutral: price movement does not affect P&L, only the spread does.

## Basis Calculation

```bash
SPOT=$(kraken ticker BTCUSD -o json 2>/dev/null | jq -r '.[].last_price')
FUTURES=$(kraken futures ticker PF_XBTUSD -o json 2>/dev/null | jq -r '.ticker.last')
BASIS=$(echo "scale=4; ($FUTURES - $SPOT) / $SPOT * 100" | bc)
echo "Basis: ${BASIS}%"
```

A positive basis means futures are more expensive than spot (contango). A negative basis means futures are cheaper (backwardation).

## Entry (Long Spot + Short Futures)

1. Check spot price and futures price.
2. Ensure the basis is attractive (e.g., > 0.5% annualized).
3. Calculate matched position sizes.
4. Execute both legs (requires human approval):

```bash
# Long spot
kraken order buy BTCUSD 0.01 --type limit --price $SPOT -o json 2>/dev/null

# Short futures (matched notional)
kraken futures order sell PF_XBTUSD 1 --type limit --price $FUTURES -o json 2>/dev/null
```

## Monitoring

Watch basis convergence:

```bash
# Periodic check
kraken ticker BTCUSD -o json 2>/dev/null
kraken futures ticker PF_XBTUSD -o json 2>/dev/null
```

Check positions:

```bash
kraken balance -o json 2>/dev/null
kraken futures positions -o json 2>/dev/null
```

## Exit

Close both legs when basis narrows to target:

```bash
# Sell spot
kraken order sell BTCUSD 0.01 --type market -o json 2>/dev/null

# Close futures short
kraken futures order buy PF_XBTUSD 1 --reduce-only -o json 2>/dev/null
```

## Risk Considerations

- **Execution risk**: legs may fill at different times. The basis can move between fills.
- **Funding risk**: on perpetuals, negative funding rates mean the short pays the long, eating into profit.
- **Margin risk**: a sharp spot rally increases margin requirement on the short futures leg.
- **Leg risk**: if one leg fails to fill, the position is directional, not neutral.

## Hard Rules

- Never enter a basis trade without confirming both legs will execute.
- Monitor futures margin continuously; a large spot rally can trigger liquidation on the short.
- Close both legs together; leaving one open converts to a directional bet.
- Requires human approval for all live entries and exits.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
