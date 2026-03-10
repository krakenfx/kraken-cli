---
name: kraken-error-recovery
version: 1.0.0
description: "Handle order failures, network errors, and duplicate submissions safely."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
    skills: ["kraken-shared"]
---

# kraken-error-recovery

Use this skill for:
- recovering from failed order submissions
- preventing duplicate orders after network errors
- handling partial fills and stuck states
- building resilient agent loops

## Error Categories

Parse the `.error` field from the JSON response:

| Category | Meaning | Recovery |
|----------|---------|----------|
| `auth` | Credentials invalid or expired | Re-authenticate, do not retry |
| `rate_limit` | Too many requests | Read `suggestion` and `docs_url` fields, adapt strategy |
| `network` | Connection failed | Retry with exponential backoff |
| `validation` | Invalid request parameters | Fix inputs, do not retry unchanged |
| `api` | Exchange-side rejection | Inspect error message, adjust request |

## Duplicate Order Prevention

After a network error during order submission, the order may or may not have reached the exchange. Check before resubmitting:

```bash
kraken open-orders -o json 2>/dev/null
```

If the order appears in open-orders, do not resubmit. If absent:

```bash
kraken trades-history -o json 2>/dev/null
```

If the order filled immediately, it appears in trades. Only resubmit if the order is absent from both.

## Client Order ID for Idempotency

Use `--cl-ord-id` to tag orders with a unique identifier:

```bash
kraken order buy BTCUSD 0.001 --type limit --price 50000 --cl-ord-id "dca-2024-01-15-001" -o json 2>/dev/null
```

If the submission fails, query by client order ID to check if it reached the exchange:

```bash
kraken open-orders --cl-ord-id "dca-2024-01-15-001" -o json 2>/dev/null
```

Cancel by client order ID if needed:

```bash
kraken order cancel --cl-ord-id "dca-2024-01-15-001" -o json 2>/dev/null
```

## Network Error Recovery Pattern

```
1. Submit order (capture exit code and response)
2. If exit code != 0 and error == "network":
   a. Wait 2 seconds
   b. Check open-orders for the order (by cl-ord-id or recent orders)
   c. If found → order succeeded, proceed
   d. If not found → check trades-history
   e. If in trades → order filled, proceed
   f. If absent from both → safe to retry
3. Retry with same cl-ord-id
```

## Rate Limit Recovery

The CLI returns rate limit errors immediately with no internal retry. The error includes actionable fields for the agent to decide next steps.

On `rate_limit` error:

1. Read the `suggestion` field for specific guidance on what limit was hit and how to adapt.
2. Read the `docs_url` field for the relevant Kraken documentation.
3. Decide whether to retry (and when), reduce request frequency, or switch to WebSocket streaming for real-time data.
4. Resume with a single test call before continuing the loop.

```bash
# { "error": "rate_limit", "suggestion": "...", "docs_url": "...", "retryable": true }
kraken status -o json 2>/dev/null
```

If status succeeds, the rate limit has cleared.

## Partial Fill Handling

Limit orders may partially fill. Check order status:

```bash
kraken query-orders <TXID> -o json 2>/dev/null
```

Fields: `vol` (requested volume), `vol_exec` (filled volume), `status` (open, closed, canceled).

If partially filled and the remaining volume is needed, amend or place a new order for the remaining amount.

## Stuck Order Resolution

An order stuck in `open` state that should have filled or been canceled:

1. Check the order:
   ```bash
   kraken query-orders <TXID> -o json 2>/dev/null
   ```
2. If still open, cancel:
   ```bash
   kraken order cancel <TXID> -o json 2>/dev/null
   ```
3. Verify cancellation:
   ```bash
   kraken open-orders -o json 2>/dev/null
   ```

## Hard Rules

- Never blind-retry an order after a network error; always check state first.
- Use `--cl-ord-id` for all orders in automated loops.
- On auth errors, stop all activity and re-authenticate.
- Log every error and recovery action for post-session audit.
