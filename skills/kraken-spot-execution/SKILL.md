---
name: kraken-spot-execution
version: 1.0.0
description: "Execute spot orders with validation, confirmation gates, and post-trade checks."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-spot-execution

Use this skill for:
- placing spot buy or sell orders
- validating order payloads before submit
- checking open orders and trade history

## Safe Execution Flow

1. Read price:
   ```bash
   kraken ticker BTCUSD -o json 2>/dev/null
   ```
2. Validate order:
   ```bash
   kraken order buy BTCUSD 0.001 --type limit --price 50000 --validate -o json 2>/dev/null
   ```
3. Ask for explicit human confirmation.
4. Execute order:
   ```bash
   kraken order buy BTCUSD 0.001 --type limit --price 50000 -o json 2>/dev/null
   ```
5. Verify placement:
   ```bash
   kraken open-orders -o json 2>/dev/null
   ```

## Cancel and Safety

- Cancel one:
  ```bash
  kraken order cancel <TXID> -o json 2>/dev/null
  ```
- Dead-man switch:
  ```bash
  kraken order cancel-after 60 -o json 2>/dev/null
  ```

## Hard Rules

- Never execute live order commands without explicit user approval.
- Route failures by `.error` category.
- On `rate_limit`, read `suggestion` and `docs_url` fields, then adapt strategy.
