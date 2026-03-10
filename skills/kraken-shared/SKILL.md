---
name: kraken-shared
version: 1.0.0
description: "Shared runtime contract for kraken-cli: auth, invocation, parsing, and safety."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-shared

**This tool is experimental. Commands execute real financial transactions on the Kraken exchange. Test with `kraken paper` before using real funds. See `DISCLAIMER.md` for full terms.**

## Invocation Contract

Always call:

```bash
kraken <command> [args...] -o json 2>/dev/null
```

Rules:
- Parse `stdout` only.
- Treat `stderr` as diagnostics.
- Exit code `0` means success.
- Non-zero exit means failure with JSON envelope in `stdout`.

## Authentication

```bash
export KRAKEN_API_KEY="your-key"
export KRAKEN_API_SECRET="your-secret"
```

Optional futures credentials:

```bash
export KRAKEN_FUTURES_API_KEY="your-futures-key"
export KRAKEN_FUTURES_API_SECRET="your-futures-secret"
```

Public market data and paper trading require no credentials.

## Error Routing

Route on `.error`:
- `auth`: re-authenticate
- `rate_limit`: read `suggestion` and `docs_url` fields, adapt strategy
- `network`: retry with exponential backoff
- `validation`: fix inputs, do not retry unchanged request
- `api`: inspect request parameters

## Safety

The catalog marks 32 commands as `dangerous: true`. Always check the `dangerous` field in `agents/tool-catalog.json` before executing a command.

Require explicit human approval before:
- live buy or sell orders (spot and futures)
- order amendments, edits, and batch operations
- cancel and cancel-all operations
- withdrawals and wallet transfers
- earn allocate/deallocate
- subaccount transfers
- WebSocket order mutations

Use paper trading for dry runs:

```bash
kraken paper init --balance 10000 -o json
```
