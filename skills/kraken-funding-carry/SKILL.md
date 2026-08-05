---
name: kraken-funding-carry
version: 1.0.0
description: "Earn funding rate payments by positioning on the paying side of perpetuals."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
    skills: ["kraken-futures-trading", "kraken-futures-risk"]
---

# kraken-funding-carry

Use this skill for:
- scanning funding rates across perpetual contracts
- identifying carry opportunities (collect funding by being on the receiving side)
- entering hedged carry positions (spot hedge + futures position)
- monitoring ongoing carry yield

## Core Concept

Perpetual futures charge periodic funding payments between longs and shorts. When the funding rate is positive, longs pay shorts. When negative, shorts pay longs. A carry strategy positions on the receiving side and hedges with spot to remain market-neutral.

## Scan Funding Rates

```bash
kraken futures historical-funding-rates PF_XBTUSD -o json 2>/dev/null
kraken futures historical-funding-rates PF_ETHUSD -o json 2>/dev/null
```

Compare rates across contracts to find the highest yield.

## Carry Entry (Positive Funding)

When funding is positive (longs pay shorts):
1. Short the perpetual.
2. Buy spot as a hedge.

```bash
# Hedge: buy spot
kraken order buy BTCUSD 0.01 --type market -o json 2>/dev/null

# Carry: short perpetual (collect funding)
kraken futures order sell PF_XBTUSD 1 -o json 2>/dev/null
```

## Carry Entry (Negative Funding)

When funding is negative (shorts pay longs):
1. Long the perpetual.
2. Sell spot as a hedge (or skip hedge if already holding).

```bash
# Carry: long perpetual (collect funding)
kraken futures order buy PF_XBTUSD 1 -o json 2>/dev/null

# Hedge: sell spot
kraken order sell BTCUSD 0.01 --type market -o json 2>/dev/null
```

## Yield Calculation

Annualized yield = funding_rate * funding_periods_per_year * 100.

Kraken funding periods are typically every 4 or 8 hours (varies by contract). Check contract specs:

```bash
kraken futures instruments -o json 2>/dev/null
```

## Monitoring

Track funding accrual:

```bash
kraken futures accounts -o json 2>/dev/null
```

The `unrealizedFunding` field shows accumulated but unsettled funding.

Watch for funding rate flips:

```bash
kraken futures historical-funding-rates PF_XBTUSD -o json 2>/dev/null
```

If the rate flips direction, the position switches from collecting to paying. Exit or reverse.

## Exit

Close both legs when carry becomes unprofitable:

```bash
kraken futures order buy PF_XBTUSD 1 --reduce-only -o json 2>/dev/null
kraken order sell BTCUSD 0.01 --type market -o json 2>/dev/null
```

## Hard Rules

- Always hedge carry positions to avoid directional exposure.
- Monitor funding rates continuously; a flip turns profit into loss.
- Monitor margin on the futures leg; price moves require margin even when hedged.
- Requires human approval for all entries and exits.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
