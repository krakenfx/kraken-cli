---
name: kraken-risk-operations
version: 1.0.0
description: "Operational risk controls for live agent trading sessions."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-risk-operations

Use this skill to reduce live-trading risk during automation.

## Pre-Flight Checklist

1. Confirm intended environment (paper vs live).
2. Confirm pair, side, volume, and order type.
3. Validate payload with `--validate`.
4. Enable a dead-man switch for unattended sessions.

## Live Session Safety Commands

Dead-man switch:

```bash
kraken order cancel-after 60 -o json 2>/dev/null
```

Mass cancel:

```bash
kraken order cancel-all -o json 2>/dev/null
kraken futures cancel-all -o json 2>/dev/null
```

Position visibility:

```bash
kraken open-orders -o json 2>/dev/null
kraken positions -o json 2>/dev/null
kraken futures positions -o json 2>/dev/null
```

## Failure Handling

- `network`: exponential backoff retry
- `rate_limit`: read `suggestion` and `docs_url` fields, reduce call frequency or switch to WebSocket
- `auth`: stop and refresh credentials
- `validation` or `api`: stop, fix request, do not blind retry

## Hard Rules

- All cancel and cancel-all operations are dangerous. Require explicit human approval before executing.
- Dead-man switch (`cancel-after`) is also dangerous; confirm the timeout value with the user.
- Never execute mass-cancel without verifying which orders and positions are open first.
