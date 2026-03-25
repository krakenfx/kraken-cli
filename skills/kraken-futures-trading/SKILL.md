---
name: kraken-futures-trading
version: 1.0.0
description: "Place, manage, and monitor futures orders across the full lifecycle."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-futures-trading

Use this skill for:
- placing futures buy and sell orders (market, limit, post, stop, take-profit, ioc, trailing-stop, fok)
- editing and cancelling futures orders
- batch order placement for multi-leg strategies
- monitoring open positions and fills

## Authentication

Futures use separate credentials from spot:

```bash
export KRAKEN_FUTURES_API_KEY="your-futures-key"
export KRAKEN_FUTURES_API_SECRET="your-futures-secret"
```

## Safe Execution Flow

1. Check available contracts:
   ```bash
   kraken futures instruments -o json 2>/dev/null
   ```
2. Read current price:
   ```bash
   kraken futures ticker PF_XBTUSD -o json 2>/dev/null
   ```
3. Check account state:
   ```bash
   kraken futures accounts -o json 2>/dev/null
   ```
4. Place order (requires human approval):
   ```bash
   kraken futures order buy PF_XBTUSD 1 --type limit --price 50000 -o json 2>/dev/null
   ```
5. Verify placement:
   ```bash
   kraken futures open-orders -o json 2>/dev/null
   ```

## Order Types

Market order:

```bash
kraken futures order buy PF_XBTUSD 1 --type market -o json 2>/dev/null
```

Limit order:

```bash
kraken futures order sell PF_XBTUSD 1 --type limit --price 70000 -o json 2>/dev/null
```

Stop order:

```bash
kraken futures order buy PF_XBTUSD 1 --type stop --stop-price 55000 --trigger-signal mark -o json 2>/dev/null
```

Trailing stop:

```bash
kraken futures order sell PF_XBTUSD 1 --type trailing-stop --stop-price 68000 --trailing-stop-max-deviation 500 --trailing-stop-deviation-unit quote_currency -o json 2>/dev/null
```

Reduce-only (close position without opening new exposure):

```bash
kraken futures order sell PF_XBTUSD 1 --type market --reduce-only -o json 2>/dev/null
```

## Batch Orders

Place multiple orders atomically (useful for bracket entries):

```bash
kraken futures batch-order '[
  {"order":"sendorder","orderTag":"entry","symbol":"PF_XBTUSD","side":"buy","size":1,"orderType":"lmt","limitPrice":50000},
  {"order":"sendorder","orderTag":"tp","symbol":"PF_XBTUSD","side":"sell","size":1,"orderType":"lmt","limitPrice":55000,"reduceOnly":true}
]' -o json 2>/dev/null
```

## Edit and Cancel

Edit in place:

```bash
kraken futures edit-order --order-id <ID> --price 51000 -o json 2>/dev/null
```

Cancel one:

```bash
kraken futures cancel --order-id <ID> -o json 2>/dev/null
```

Cancel all:

```bash
kraken futures cancel-all -o json 2>/dev/null
```

Cancel by symbol:

```bash
kraken futures cancel-all --symbol PF_XBTUSD -o json 2>/dev/null
```

## Position Monitoring

Open positions:

```bash
kraken futures positions -o json 2>/dev/null
```

Recent fills:

```bash
kraken futures fills --since 2024-01-01T00:00:00Z -o json 2>/dev/null
```

Execution history:

```bash
kraken futures history-executions --since 2024-01-01T00:00:00Z --sort desc -o json 2>/dev/null
```

Order history:

```bash
kraken futures history-orders --sort desc -o json 2>/dev/null
```

## Dead Man's Switch

Enable for unattended sessions. All orders cancel if the timer expires:

```bash
kraken futures cancel-after 600 -o json 2>/dev/null
```

Refresh periodically. If the agent crashes, orders auto-cancel.

## Paper Trading

Test futures strategies without real money using `kraken futures paper`. Near-parity with live futures: all 8 order types (market, limit, post, stop, take-profit, ioc, trailing-stop, fok); leverage, margin tracking, liquidation simulation, and funding rates. Known differences: single-ID `order-status`, post-only orders are cancelled rather than queued, fills use the bid/ask snapshot with no depth-based slippage, and partial fills are not modeled.

```bash
kraken futures paper init --balance 10000 -o json 2>/dev/null
kraken futures paper buy PF_XBTUSD 1 --leverage 10 --type market -o json 2>/dev/null
kraken futures paper positions -o json 2>/dev/null
kraken futures paper status -o json 2>/dev/null
kraken futures paper reset -o json 2>/dev/null
```

Switch between paper and live by replacing `futures paper` with `futures order`. No credentials required for paper trading.

## Hard Rules

- Never place futures orders without explicit human approval.
- Always check `futures accounts` before trading to confirm margin availability.
- Use `--reduce-only` when closing positions to prevent accidental flips.
- Enable `cancel-after` for any automated session.
- Test strategies with `kraken futures paper` before going live.
