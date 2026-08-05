# CLAUDE.md

> **This is experimental software. Commands execute real financial transactions. Test with `kraken paper` (spot) or `kraken futures paper` (futures) before real funds. See `DISCLAIMER.md`.**

Integration guidance for Claude Code, Claude Desktop, and other Anthropic-powered agents interacting with `kraken-cli`.

Fast entry points:
- Runtime context: `CONTEXT.md`
- Full command contract: `agents/tool-catalog.json`

## What is kraken-cli?

A command-line interface for trading crypto, stocks, forex, and derivatives on Kraken. Every command returns structured JSON. Designed for AI agents and automated pipelines.

## Invocation

Call `kraken` as a subprocess. Always use `-o json` and redirect stderr:

```bash
kraken <command> [args...] -o json 2>/dev/null
```

## Authentication

Set environment variables before invoking:

```bash
export KRAKEN_API_KEY="your-key"
export KRAKEN_API_SECRET="your-secret"
```

Public market data commands (ticker, orderbook, ohlc, trades, spreads, status) require no credentials.

## Key Conventions

- stdout is valid JSON on success (exit 0), or a JSON error envelope on runtime failure (exit 1).
- The `error` field in error envelopes is a stable category code: `api`, `auth`, `network`, `rate_limit`, `validation`, `config`, `websocket`, `io`, `timeout`, `parse`.
- WebSocket commands emit JSONL (one JSON object per line).
- Paper trading commands (`kraken paper ...` for spot, `kraken futures paper ...` for futures) use live prices but no real money. No auth needed.
- Exit code 0 = success, 1 = runtime failure (JSON envelope on stdout), 2 = argument/usage error from the parser. With `-o json`, exit 2 puts a `{"error":"validation"}` envelope on stdout (stderr stays empty); without it, clap's usage text goes to stderr. Validate argument values against the `values` arrays in `agents/tool-catalog.json` before invoking.

## Safety Rules

1. Never execute any command marked `dangerous` without explicit user confirmation. The `dangerous` field in `agents/tool-catalog.json` is the authoritative list (41 commands). Four of them (`order buy/sell/cancel/cancel-all`) also carry `paper_safe: true`: inside a paper workspace they execute simulated fills and need no confirmation.
2. Use `--validate` flag to dry-run order commands before submitting.
3. Use `kraken paper` (spot) or `kraken futures paper` (futures) commands for testing strategies safely.
4. Gate all order placement, cancellation, withdrawal, transfer, and staking operations behind user approval.
5. Never log or display API secrets.

## Common Operations

### Get market data (no auth)

```bash
kraken ticker BTCUSD -o json
kraken orderbook BTCUSD --count 10 -o json
kraken ohlc BTCUSD --interval 60 -o json
kraken trades BTCUSD --count 20 -o json
```

### Record market data (no auth)

```bash
kraken record --symbols BTC/USD --channels trades,book --tape my-tape -o json  # runs until stopped; SIGTERM/Ctrl-C finalizes
kraken tape list -o json                                                       # library: refs, state, in_use per backend
kraken session start --from tape:my-tape --speed 10 -o json                    # replay a finalized tape into a paper session
```

Run `record` as a background process; lifecycle and `--monitor` health lines are stderr JSONL. One writer per tape: a concurrent recorder gets a `validation` envelope.

### Check account (auth required)

```bash
kraken balance -o json
kraken open-orders -o json
kraken trades-history -o json
```

### Place orders (auth required, dangerous)

```bash
# Validate first
kraken order buy BTCUSD 0.001 --type limit --price 50000 --validate -o json

# Then execute (requires user confirmation)
kraken order buy BTCUSD 0.001 --type limit --price 50000 -o json
```

### xStocks and forex

Stock symbols use the `x` suffix (AAPL becomes `AAPLx`). Pass `--asset-class tokenized_asset` on trade and market commands. Forex uses `--asset-class forex` on market data.

```bash
kraken ticker AAPLx/USD --asset-class tokenized_asset -o json
kraken order buy AAPLx/USD 0.1 --type limit --price 200 --asset-class tokenized_asset -o json
kraken ticker EURUSD --asset-class forex -o json
```

### Futures

Futures use a separate engine with separate credentials. Symbols: `PF_XBTUSD` (perp), `FI_XBTUSD_260327` (fixed-date), `PF_AAPLXUSD` (equity perp).

```bash
kraken futures order buy PF_XBTUSD 1 --type limit --price 50000 -o json
```

### Spot paper trading (no auth, safe)

```bash
kraken workspace create sandbox --capital 10000 --mode paper --slippage-rate 0.001 -o json
export KRAKEN_WORKSPACE=sandbox
kraken paper buy BTCUSD 0.01 -o json
kraken workspace status -o json
kraken workspace reset -o json --yes
```

Fee rate (default 0.26%) and slippage rate (default 0.0) are set at `workspace create` and fixed for the account's life; `workspace reset` returns to the created capital. The `paper init/reset/status/balance` verbs still exist as aliases of the workspace commands (one implementation).

### Futures paper trading (no auth, safe)

```bash
kraken futures paper init --balance 10000 -o json
kraken futures paper buy PF_XBTUSD 1 --leverage 10 --type market -o json
kraken futures paper sell PF_ETHUSD 5 --leverage 20 --type market -o json
kraken futures paper positions -o json
kraken futures paper status -o json
kraken futures paper reset -o json
```

### Error handling

```bash
RESULT=$(kraken balance -o json 2>/dev/null)
if [ $? -ne 0 ]; then
  CATEGORY=$(echo "$RESULT" | jq -r '.error // "unknown"')
  # Route on category: auth, rate_limit, network, api, validation, etc.
fi
```

## Tool Discovery

Load `agents/tool-catalog.json` for the full machine-readable command contract. Each entry includes parameters, types, auth requirements, and a `dangerous` flag.

## Full Documentation

- `AGENTS.md`: Complete agent integration guide
- `CONTEXT.md`: Runtime-optimized context for tool-using agents
- `agents/tool-catalog.json`: All 174 commands with parameters
- `agents/error-catalog.json`: Error categories with retry guidance
