---
name: kraken-liquidation-guard
version: 1.0.0
description: "Prevent futures liquidation through margin monitoring and emergency procedures."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
    skills: ["kraken-futures-risk"]
---

# kraken-liquidation-guard

Use this skill for:
- monitoring margin ratio to detect liquidation risk
- automated position reduction when thresholds are breached
- emergency flatten procedures
- proactive margin top-up patterns

## Margin Health Check

```bash
kraken futures accounts -o json 2>/dev/null
```

Key fields:
- `equity`: total account value
- `initialMargin`: margin held for positions
- `maintenanceMargin`: minimum margin before liquidation
- `availableFunds`: free margin for new positions

Margin ratio = maintenanceMargin / equity. Liquidation approaches as this ratio nears 1.0.

## Alert Thresholds

| Ratio | Status | Action |
|-------|--------|--------|
| < 0.3 | Healthy | Normal operation |
| 0.3 - 0.5 | Caution | Reduce position or add margin |
| 0.5 - 0.7 | Warning | Reduce position immediately |
| > 0.7 | Critical | Emergency flatten |

## Monitoring Loop

```bash
kraken futures ws balances -o json 2>/dev/null | while read -r line; do
  # Parse equity and maintenanceMargin
  # Calculate ratio
  # Alert or act when threshold is breached
done
```

Or poll periodically:

```bash
kraken futures accounts -o json 2>/dev/null
```

## Unwind Queue Check

If a position enters the unwind queue, liquidation is imminent:

```bash
kraken futures unwind-queue -o json 2>/dev/null
```

Any result here requires immediate action.

## Emergency Flatten

Cancel all open orders, then close all positions:

```bash
# Cancel all orders to free held margin
kraken futures cancel-all --yes -o json 2>/dev/null

# Close each position with reduce-only market orders
kraken futures positions -o json 2>/dev/null
# For each position:
kraken futures order sell PF_XBTUSD <SIZE> --reduce-only -o json 2>/dev/null
```

## Margin Top-Up

Transfer funds from spot wallet to futures to increase margin:

```bash
kraken futures transfer 5000 USD -o json 2>/dev/null
```

Or from main account:

```bash
kraken wallet-transfer USD 5000 --from <SPOT_IIBAN> --to <FUTURES_IIBAN> -o json 2>/dev/null
```

## Leverage Reduction

Lower leverage to reduce margin requirements:

```bash
kraken futures set-leverage PF_XBTUSD 2 -o json 2>/dev/null
```

## Dead Man's Switch

Always run a dead man's switch during futures sessions:

```bash
kraken futures cancel-after 600 -o json 2>/dev/null
```

If the agent crashes, orders auto-cancel, preventing further margin consumption.

## Hard Rules

- Monitor margin ratio continuously during any futures session.
- Alert at 0.5 ratio; act at 0.7. Do not wait for the exchange to liquidate.
- Emergency flatten requires human approval unless the agent operates at autonomy level 4+.
- Never increase position size when margin ratio is above 0.3.
- `futures set-leverage` is flagged dangerous: it changes the margin requirement on open positions, so de-risking with it needs the same human approval as flattening.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
