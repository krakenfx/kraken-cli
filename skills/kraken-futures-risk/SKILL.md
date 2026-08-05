---
name: kraken-futures-risk
version: 1.0.0
description: "Futures-specific risk management: leverage, funding rates, margin, and liquidation awareness."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-futures-risk

Use this skill for:
- checking and setting leverage
- monitoring funding rates for carry cost
- tracking margin and account health
- position sizing relative to available margin
- understanding liquidation risk

## Leverage Management

Check current leverage preferences:

```bash
kraken futures leverage -o json 2>/dev/null
```

Check leverage for a specific symbol:

```bash
kraken futures leverage --symbol PF_XBTUSD -o json 2>/dev/null
```

Set maximum leverage (requires human approval):

```bash
kraken futures set-leverage PF_XBTUSD 5 -o json 2>/dev/null
```

Lower leverage reduces liquidation risk but requires more margin per position.

## Funding Rate Monitoring

Check historical funding rates:

```bash
kraken futures historical-funding-rates PF_XBTUSD -o json 2>/dev/null
```

Funding rates are periodic payments between longs and shorts. Positive rates mean longs pay shorts; negative rates mean shorts pay longs. High sustained rates increase carry cost for directional positions.

Use funding rates to:
- Estimate holding cost for a position over time.
- Identify crowded trades (extreme funding rates).
- Time entries when funding flips direction.

## Account and Margin Health

Check account balances and margin:

```bash
kraken futures accounts -o json 2>/dev/null
```

Key fields: `availableFunds`, `initialMargin`, `maintenanceMargin`, `unrealizedFunding`, `pnl`.

Margin ratio = maintenanceMargin / equity. When this approaches 1.0, liquidation risk is high.

## Position Monitoring

View all open positions:

```bash
kraken futures positions -o json 2>/dev/null
```

Stream position updates in real time:

```bash
kraken futures ws open-positions -o json 2>/dev/null
```

Monitor P&L and size continuously. Set agent-side thresholds for:
- Maximum unrealized loss per position
- Maximum total portfolio loss
- Maximum position size

## PnL Preferences

Check P&L calculation method:

```bash
kraken futures pnl-preferences -o json 2>/dev/null
```

Set preference for a specific symbol:

```bash
kraken futures set-pnl-preference PF_XBTUSD FIFO -o json 2>/dev/null
```

## Unwind Queue

Check if any positions are in the unwind queue (approaching liquidation):

```bash
kraken futures unwind-queue -o json 2>/dev/null
```

If positions appear here, the agent should alert immediately and consider reducing exposure.

## Pre-Trade Risk Check

Before placing a futures order, always:

1. Check available margin:
   ```bash
   kraken futures accounts -o json 2>/dev/null
   ```
2. Check current positions:
   ```bash
   kraken futures positions -o json 2>/dev/null
   ```
3. Check current leverage:
   ```bash
   kraken futures leverage --symbol PF_XBTUSD -o json 2>/dev/null
   ```
4. Estimate position margin requirement based on order size and leverage.
5. Confirm available funds exceed the required margin with a safety buffer.

## Emergency Procedures

Close all positions:

```bash
kraken futures cancel-all -o json 2>/dev/null
# Then close each position with a reduce-only market order
kraken futures order sell PF_XBTUSD <POSITION_SIZE> --reduce-only -o json 2>/dev/null
```

Dead man's switch:

```bash
kraken futures cancel-after 300 -o json 2>/dev/null
```

## Hard Rules

- `futures set-leverage` and `futures set-pnl-preference` are flagged dangerous. Never change either without explicit human approval — a leverage change also alters the margin requirement on open positions, in both directions.
- Monitor margin ratio continuously during live sessions.
- Alert when margin ratio exceeds 0.7 (conservative threshold).
- Alert when unrealized loss exceeds a pre-defined stop-loss level.
- Check funding rates before opening new positions to understand carry cost.
- Use `--reduce-only` when closing positions to prevent accidental reversals.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
