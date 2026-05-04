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

- stdout is always valid JSON on success, or a JSON error envelope on failure.
- The `error` field in error envelopes is a stable category code: `api`, `auth`, `network`, `rate_limit`, `validation`, `config`, `websocket`, `io`, `parse`.
- WebSocket commands emit NDJSON (one JSON object per line).
- Paper trading commands (`kraken paper ...` for spot, `kraken futures paper ...` for futures) use live prices but no real money. No auth needed.
- Exit code 0 = success, non-zero = failure.

## Safety Rules

1. Never execute any command marked `dangerous` without explicit user confirmation. The `dangerous` field in `agents/tool-catalog.json` is the authoritative list (34 commands).
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
kraken paper init --balance 10000 -o json
kraken paper buy BTCUSD 0.01 -o json
kraken paper status -o json
kraken paper reset -o json
```

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
- `agents/tool-catalog.json`: All 152 commands with parameters
- `agents/error-catalog.json`: Error categories with retry guidance
