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
- Streaming commands (`session start`, `record`, `ws`, `streamd`) emit JSONL: match lines by their `"type"` field (e.g. scan for `"type":"session_started"` to get the session handle) — never assume a line position, because live market frames interleave on `stdout`.
- Probe capability, not version, at the start of a loop: `kraken session --help >/dev/null 2>&1` exits `0` on a build that speaks this contract and `2` on an older one — `kraken --version` cannot tell them apart across a vocabulary change.
- Market-data field names are readable (`last_price`, not `c[0]`). Only `ticker` still keys on Kraken's INTERNAL pair name (`XXBTZUSD`, not `BTCUSD`) — reach its fields with `.[]` or `to_entries`, never a hardcoded key. `ohlc`/`trades`/`spreads` reshape to `{pair, …}`: fold the named row array (`.candles`/`.trades`/`.spreads`) directly, never the whole object — `ohlc` keeps a sibling `last` cursor that poisons naive iteration. `orderbook` drops the wrapper to top-level `.asks`/`.bids`.
- Validate every number a READ produces before it gates an order or lands in a reason. On an empty or failed read, record `kraken session note --kind alert --reason "READ failed: ..."` and end the step — never interpolate a failed read into a reason, or the decision log fills with plausible-looking false evidence.

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

The catalog marks 41 commands as `dangerous: true`. Always check the `dangerous` field in `agents/tool-catalog.json` before executing a command.

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
kraken workspace create sandbox --capital 10000 --mode paper -o json
export KRAKEN_WORKSPACE=sandbox
```

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
