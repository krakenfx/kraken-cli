---
name: kraken-paper-to-live
version: 1.0.0
description: "Promote a validated paper strategy to live trading with safety checks."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
    skills: ["kraken-paper-strategy", "kraken-spot-execution", "kraken-risk-operations"]
---

# kraken-paper-to-live

Use this skill for:
- validating a strategy has stable paper results before going live
- running pre-flight checks before first live trade
- migrating paper commands to their live equivalents
- establishing safety controls for the live session

## Paper-to-Live Performance Gap

Both spot and futures paper trading simulate taker fees (0.26% spot, configurable for futures). Slippage and partial fills are not fully modeled. Live results may differ. Before promoting, factor in:

- **Slippage:** Market orders fill at available liquidity, not the mid-price. Thin books or larger sizes widen the gap.
- **Partial fills and latency:** Limit orders may fill partially or not at all. Network and matching-engine latency can cause missed entries or exits.
- **Maker fees:** Paper uses taker fee rates for all fills. Live limit orders that provide liquidity pay lower maker fees (0.16% spot).

When presenting promotion analysis to the user, explicitly state the expected performance reduction from these factors.

## Promotion Criteria

A strategy is ready for live promotion when:
1. Paper runs produce consistent results over multiple sessions.
2. Error handling works correctly (rate limits, network failures).
3. The strategy stays within defined risk parameters.
4. Paper returns remain positive after subtracting estimated fees (0.26% per fill for spot, 0.05% for futures).
5. The user explicitly approves the transition.

## Pre-Flight Checklist

### Spot

Before the first live spot trade:

1. **Verify credentials**:
   ```bash
   kraken auth test -o json 2>/dev/null
   ```

2. **Check balance**:
   ```bash
   kraken balance -o json 2>/dev/null
   ```

3. **Confirm pair is tradable**:
   ```bash
   kraken pairs --pair BTCUSD -o json 2>/dev/null
   ```

4. **Validate a sample order** (does not execute):
   ```bash
   kraken order buy BTCUSD 0.001 --type limit --price 50000 --validate -o json 2>/dev/null
   ```

5. **Enable dead man's switch**:
   ```bash
   kraken order cancel-after 600 -o json 2>/dev/null
   ```

### Futures

Before the first live futures trade:

1. **Verify futures credentials**:
   ```bash
   kraken futures accounts -o json 2>/dev/null
   ```

2. **Check margin availability**:
   ```bash
   kraken futures accounts -o json 2>/dev/null
   ```

3. **Confirm instrument is tradable**:
   ```bash
   kraken futures instrument-status --symbol PF_XBTUSD -o json 2>/dev/null
   ```

4. **Set leverage**:
   ```bash
   kraken futures set-leverage PF_XBTUSD 10 -o json 2>/dev/null
   ```

5. **Enable dead man's switch**:
   ```bash
   kraken futures cancel-after 600 -o json 2>/dev/null
   ```

## Command Migration

Paper and live commands differ only in the prefix.

### Spot

| Paper | Live |
|-------|------|
| `kraken paper buy BTCUSD 0.01` | `kraken order buy BTCUSD 0.01` |
| `kraken paper sell BTCUSD 0.01` | `kraken order sell BTCUSD 0.01` |
| `kraken paper status` | `kraken balance` + `kraken open-orders` |
| `kraken paper orders` | `kraken open-orders` |
| `kraken paper history` | `kraken trades-history` |
| `kraken paper cancel <ID>` | `kraken order cancel <TXID>` |

### Futures

| Paper | Live |
|-------|------|
| `kraken futures paper buy PF_XBTUSD 1 --leverage 10 --type market` | `kraken futures order buy PF_XBTUSD 1 --type market` |
| `kraken futures paper sell PF_XBTUSD 1 --leverage 10 --type market` | `kraken futures order sell PF_XBTUSD 1 --type market` |
| `kraken futures paper positions` | `kraken futures positions` |
| `kraken futures paper orders` | `kraken futures open-orders` |
| `kraken futures paper fills` | `kraken futures fills` |
| `kraken futures paper cancel --order-id <ID>` | `kraken futures cancel --order-id <ID>` |
| `kraken futures paper cancel-all` | `kraken futures cancel-all` |

**Leverage note:** Paper accepts `--leverage` inline on buy/sell commands. Live futures configures leverage separately via `kraken futures set-leverage <SYMBOL> <LEVERAGE>` before placing orders.

## Gradual Promotion

Start with smaller size than paper:

1. **Paper size**: the volume used during testing.
2. **Initial live size**: 10-25% of paper size.
3. **Scale up**: increase gradually after confirming live behavior matches paper.

## Live Session Safety

After going live, maintain these controls:

- Dead man's switch refreshed periodically.
- Balance check after every trade.
- Open orders verified after every placement.
- Error handling active for all error categories.
- Maximum loss threshold that triggers session shutdown.

## Rollback

If live behavior diverges from paper:

### Spot
1. Cancel all open spot orders:
   ```bash
   kraken order cancel-all -o json 2>/dev/null
   ```
2. Assess balances.
3. Return to `kraken paper` to debug.

### Futures
1. Cancel all open futures orders:
   ```bash
   kraken futures cancel-all -o json 2>/dev/null
   ```
2. Close open positions with `--reduce-only`.
3. Return to `kraken futures paper` to debug.

## Hard Rules

- Never promote without explicit user sign-off.
- Start at reduced size.
- Always validate live orders before executing.
- Maintain dead man's switch throughout the live session.
- First live session should run at autonomy level 3 (supervised) regardless of prior paper autonomy.
