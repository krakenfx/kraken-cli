---
name: kraken-fee-optimization
version: 1.0.0
description: "Minimize trading fees through maker orders, volume tiers, and fee-aware execution."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-fee-optimization

Use this skill for:
- understanding maker vs taker fee structures
- using post-only orders to guarantee maker rates
- tracking 30-day volume for tier progression
- choosing fee currency to minimize cost

## Fee Tiers

Check current 30-day volume and fee rates:

```bash
kraken volume --pair BTCUSD -o json 2>/dev/null
```

The response includes `volume` (30-day USD equivalent) and `fees` / `fees_maker` arrays showing the tier schedule.

## Maker vs Taker

- **Taker**: you remove liquidity (market orders, limit orders that fill immediately). Higher fee.
- **Maker**: you add liquidity (limit orders that rest on the book). Lower fee, sometimes zero.

At starter tier: 0.26% taker, 0.16% maker. The difference compounds over many trades.

## Post-Only Orders

Force maker execution by rejecting orders that would immediately fill:

```bash
kraken order buy BTCUSD 0.001 --type limit --price 50000 --oflags post -o json 2>/dev/null
```

If the order would cross the spread and fill as taker, it is rejected instead. This guarantees maker fees on every fill.

## Fee Currency Selection

By default, fees are deducted from the received currency. Override with:

```bash
kraken order buy BTCUSD 0.001 --type limit --price 50000 --oflags fciq -o json 2>/dev/null
```

| Flag | Fee taken from |
|------|---------------|
| `fciq` | Quote currency (e.g., USD) |
| `fcib` | Base currency (e.g., BTC) |

Paying fees in quote currency preserves your base asset balance, useful when accumulating.

## Volume-Based Optimization

Higher 30-day volume unlocks lower fees. Track progress:

```bash
kraken volume -o json 2>/dev/null
```

If close to the next tier threshold, consolidating trading activity (rather than splitting across exchanges) can push you into a cheaper bracket.

## Futures Fee Schedules

```bash
kraken futures feeschedules -o json 2>/dev/null
```

Futures fee volumes:

```bash
kraken futures fee-schedule-volumes -o json 2>/dev/null
```

## Fee-Aware Order Sizing

When buying a fixed dollar amount, account for fees in the volume calculation:

```bash
# $100 buy at starter taker rate (0.26%)
# Effective spend: $100 / 1.0026 = $99.74 worth of BTC
# Fee: ~$0.26
```

For precise accounting, check the fee field in the trade response after execution.

## Batch Orders for Fee Efficiency

Batch orders share a single API call, reducing overhead. Use when placing related orders:

```bash
kraken order batch orders.json --pair BTCUSD -o json 2>/dev/null
```

## Hard Rules

- Order placement commands are dangerous. Never execute without explicit human approval.
- Always `--validate` before submitting live orders.
- Post-only orders can be rejected; handle rejection gracefully and retry.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
