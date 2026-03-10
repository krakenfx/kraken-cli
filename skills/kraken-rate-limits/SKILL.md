---
name: kraken-rate-limits
version: 2.0.0
description: "Understand Kraken API rate limits and adapt agent behavior when limits are hit."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-rate-limits

Use this skill for:
- understanding Kraken's rate limit systems (Spot REST, trading engine, Futures)
- adapting agent behavior when rate limit errors occur
- choosing optimal request patterns (WebSocket vs REST, batch vs individual)

## How Rate Limiting Works

The CLI does not pre-throttle or retry rate-limited requests. The Kraken API server enforces rate limits. When a limit is hit, the CLI returns the error immediately with structured fields:

```json
{
  "error": "rate_limit",
  "message": "Spot REST API rate limit exceeded ...",
  "suggestion": "Wait 5-15 seconds before retrying. ...",
  "retryable": true,
  "docs_url": "https://docs.kraken.com/api/docs/guides/spot-rest-ratelimits/"
}
```

Read the `suggestion` field to understand what limit was hit and how to adapt.

## Kraken's Rate Limit Systems

### 1. Spot REST Counter

Counter-decay model. Each call adds to a counter; the counter decays over time.

| Tier | Max Counter | Decay Rate |
|------|-------------|------------|
| Starter | 15 | 0.33/s |
| Intermediate | 20 | 0.5/s |
| Pro | 20 | 1.0/s |

Most calls cost 1 point. Ledgers, trade history, and query-ledgers cost 2. AddOrder and CancelOrder are excluded from this counter (they use the trading engine limiter instead).

Docs: https://docs.kraken.com/api/docs/guides/spot-rest-ratelimits/

### 2. Spot Trading Engine (per-pair)

Separate per-pair counter with penalties for short-lived orders:

| Tier | Threshold | Decay Rate |
|------|-----------|------------|
| Starter | 60 | 1/s |
| Intermediate | 125 | 2.34/s |
| Pro | 180 | 3.75/s |

Cancel within 5 seconds costs +8, amend within 5 seconds costs +3. Let orders rest longer to reduce cost.

Docs: https://docs.kraken.com/api/docs/guides/spot-ratelimits

### 3. Futures Cost Budget

Cost-based system with two separate pools:

- `/derivatives` endpoints: budget of 500 per 10 seconds. `sendorder` costs 10, `editorder` costs 10, `cancelorder` costs 10, `batchorder` costs 9 + batch size, `cancelallorders` costs 25, `accounts` costs 2.
- `/history` endpoints: pool of 100 tokens, refills at 100 per 10 minutes.

Docs: https://docs.kraken.com/api/docs/guides/futures-rate-limits/

## Agent Strategies

### Prefer WebSocket over REST polling

Streaming does not consume REST rate limit points. For real-time data, always prefer:

```bash
kraken ws ticker BTC/USD -o json 2>/dev/null
```

### Use batch orders

Batch orders cost less per-order on both Spot and Futures. Up to 15 orders per batch on Spot.

### Read the suggestion field

When a rate limit error is returned, the `suggestion` field contains the specific limit that was hit and concrete advice. Parse it and adapt:

```bash
RESULT=$(kraken balance -o json 2>/dev/null)
if [ $? -ne 0 ]; then
  CATEGORY=$(echo "$RESULT" | jq -r '.error // "unknown"')
  if [ "$CATEGORY" = "rate_limit" ]; then
    SUGGESTION=$(echo "$RESULT" | jq -r '.suggestion // "Wait and retry"')
    DOCS=$(echo "$RESULT" | jq -r '.docs_url // ""')
    echo "Rate limited. $SUGGESTION"
    echo "Read more: $DOCS"
  fi
fi
```

### Multi-command budget planning

Before executing a sequence, estimate total cost:

```bash
# This sequence costs ~5 points on the Spot REST counter:
kraken ticker BTCUSD -o json 2>/dev/null       # 1 point
kraken balance -o json 2>/dev/null             # 1 point
kraken open-orders -o json 2>/dev/null         # 1 point
kraken trades-history -o json 2>/dev/null      # 2 points (heavy call)
```

Leave headroom for retries and unexpected calls.
