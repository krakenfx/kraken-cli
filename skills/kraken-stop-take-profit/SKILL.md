---
name: kraken-stop-take-profit
version: 1.0.0
description: "Manage stop-loss and take-profit orders for risk-bounded positions."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
    skills: ["kraken-order-types"]
---

# kraken-stop-take-profit

Use this skill for:
- placing stop-loss orders to limit downside
- placing take-profit orders to lock in gains
- building bracket orders (entry + stop + target)
- trailing stops that follow a rising price

## Simple Stop-Loss

After buying, place a stop-loss below entry:

```bash
# Entry
kraken order buy BTCUSD 0.01 --type limit --price 60000 -o json 2>/dev/null

# Stop-loss (triggers market sell if price drops to 57000)
kraken order sell BTCUSD 0.01 --type stop-loss --price 57000 -o json 2>/dev/null
```

## Simple Take-Profit

Place a take-profit above entry:

```bash
# Take-profit (triggers market sell if price rises to 65000)
kraken order sell BTCUSD 0.01 --type take-profit --price 65000 -o json 2>/dev/null
```

## Bracket Order (Entry + Stop + Target)

Place all three as separate orders:

```bash
# 1. Entry
kraken order buy BTCUSD 0.01 --type limit --price 60000 -o json 2>/dev/null

# 2. Stop-loss
kraken order sell BTCUSD 0.01 --type stop-loss --price 57000 -o json 2>/dev/null

# 3. Take-profit
kraken order sell BTCUSD 0.01 --type take-profit --price 65000 -o json 2>/dev/null
```

When one exit fills, cancel the other to avoid double exposure:

```bash
kraken order cancel <OTHER_TXID> -o json 2>/dev/null
```

## Stop-Loss Limit (Tighter Control)

A stop-loss-limit triggers a limit order instead of market, giving price control but risking no fill in fast moves:

```bash
kraken order sell BTCUSD 0.01 --type stop-loss-limit --price 57000 --price2 56800 -o json 2>/dev/null
```

`--price` is the trigger, `--price2` is the limit price for the resulting order.

## Trailing Stop

Follows the market up, sells on reversal:

```bash
# Trail $500 below the high
kraken order sell BTCUSD 0.01 --type trailing-stop --price +500 -o json 2>/dev/null
```

As BTC rises from 60000 to 65000, the stop moves from 59500 to 64500. On a $500 drop from any high, it triggers.

## Futures Stop-Loss

```bash
kraken futures order sell PF_XBTUSD 1 --type stop --stop-price 57000 --trigger-signal mark --reduce-only -o json 2>/dev/null
```

Use `--trigger-signal mark` or `index` to avoid stop hunts on last-trade wicks. Use `--reduce-only` to prevent the stop from opening a short.

## Management Loop

Monitor stops after placement:

```bash
kraken open-orders -o json 2>/dev/null
```

Stream execution updates to detect when a stop triggers:

```bash
kraken ws executions -o json 2>/dev/null
```

When one side of a bracket fills, immediately cancel the other.

## Hard Rules

- Never place live stop/take-profit orders without explicit human approval.
- Always cancel the opposite leg when one side of a bracket fills.
- Use `--reduce-only` on futures exits to prevent accidental position flips.
- Validate all orders before submission with `--validate`.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
