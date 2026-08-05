---
name: kraken-order-types
version: 1.0.0
description: "Complete reference for all spot and futures order types and modifiers."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-order-types

Use this skill for:
- choosing the right order type for a given scenario
- understanding order modifiers (post-only, reduce-only, GTC, IOC, FOK)
- constructing complex orders with stop-loss and take-profit
- batch and conditional order patterns

## Spot Order Types

### Market

Fills immediately at best available price. Simple, guaranteed fill, but no price control.

```bash
kraken order buy BTCUSD 0.001 -o json 2>/dev/null
kraken order sell BTCUSD 0.001 --type market -o json 2>/dev/null
```

### Limit

Fills at the specified price or better. No fill guarantee if price never reaches the level.

```bash
kraken order buy BTCUSD 0.001 --type limit --price 50000 -o json 2>/dev/null
```

### Stop-Loss

Triggers a market order when the price crosses the stop level.

```bash
kraken order sell BTCUSD 0.001 --type stop-loss --price 48000 -o json 2>/dev/null
```

### Stop-Loss Limit

Triggers a limit order when the price crosses the stop level.

```bash
kraken order sell BTCUSD 0.001 --type stop-loss-limit --price 48000 --price2 47500 -o json 2>/dev/null
```

### Take-Profit

Triggers a market order when the price reaches the profit target.

```bash
kraken order sell BTCUSD 0.001 --type take-profit --price 55000 -o json 2>/dev/null
```

### Take-Profit Limit

Triggers a limit order at the profit target.

```bash
kraken order sell BTCUSD 0.001 --type take-profit-limit --price 55000 --price2 54800 -o json 2>/dev/null
```

### Trailing Stop

Stop level moves with the market to lock in profits.

```bash
kraken order sell BTCUSD 0.001 --type trailing-stop --price +500 -o json 2>/dev/null
```

### Trailing Stop Limit

Trailing stop that triggers a limit order instead of market.

```bash
kraken order sell BTCUSD 0.001 --type trailing-stop-limit --price +500 --price2 -100 -o json 2>/dev/null
```

## Spot Order Modifiers

| Flag | Effect |
|------|--------|
| `--oflags post` | Post-only: reject if it would immediately fill (maker only) |
| `--oflags fciq` | Fee in quote currency |
| `--oflags fcib` | Fee in base currency |
| `--oflags nompp` | No market price protection |
| `--timeinforce GTC` | Good-til-cancelled (default) |
| `--timeinforce IOC` | Immediate-or-cancel: fill what you can, cancel the rest |
| `--timeinforce GTD` | Good-til-date: cancel after specified expiry |
| `--validate` | Validate only, do not submit |

## Futures Order Types

### Market

```bash
kraken futures order buy PF_XBTUSD 1 -o json 2>/dev/null
```

### Limit

```bash
kraken futures order buy PF_XBTUSD 1 --type limit --price 50000 -o json 2>/dev/null
```

### Stop

```bash
kraken futures order sell PF_XBTUSD 1 --type stop --stop-price 48000 --trigger-signal mark -o json 2>/dev/null
```

### Trailing Stop (Futures)

```bash
kraken futures order sell PF_XBTUSD 1 --type stop --stop-price 68000 --trailing-stop-max-deviation 500 --trailing-stop-deviation-unit quote_currency -o json 2>/dev/null
```

## Futures Order Modifiers

| Flag | Effect |
|------|--------|
| `--reduce-only` | Close existing position only, never open new |
| `--trigger-signal mark` | Trigger on mark price |
| `--trigger-signal index` | Trigger on index price |
| `--trigger-signal last` | Trigger on last traded price |
| `--client-order-id <ID>` | Attach a custom ID for tracking |

## Order Selection Guide

| Goal | Order Type |
|------|-----------|
| Buy now at any price | market |
| Buy at a specific price or better | limit |
| Protect downside if price drops | stop-loss |
| Lock in profit if price rises | take-profit |
| Trail a rising price and sell on reversal | trailing-stop |
| Earn maker rebates | limit + `--oflags post` |
| Fill immediately or not at all | limit + `--timeinforce IOC` |

## Validation

Always validate before submitting:

```bash
kraken order buy BTCUSD 0.001 --type limit --price 50000 --validate -o json 2>/dev/null
```

## Hard Rules

- All order placement commands are dangerous. Never execute without explicit human approval.
- Always validate with `--validate` before live submission.
- Use `--reduce-only` on futures exits to prevent accidental position flips.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
