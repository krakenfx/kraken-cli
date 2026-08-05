# Agent Integration Guide: kraken-cli

> **This is experimental software. Commands execute real financial transactions on the Kraken exchange. Test with `kraken paper` before using real funds. See [DISCLAIMER.md](DISCLAIMER.md) for full terms.**

Self-contained guide for integrating `kraken-cli` into AI agents, MCP clients, and automated pipelines.

Fast entry points:
- Runtime agent context: `CONTEXT.md`
- Full command contract: `agents/tool-catalog.json`
- Error routing contract: `agents/error-catalog.json`
- Workflow skills: `skills/`

## Installation

Single binary, no runtime dependencies.

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/krakenfx/kraken-cli/releases/latest/download/kraken-cli-installer.sh | sh
```

Pre-built binaries for macOS (Apple Silicon, Intel) and Linux (x86_64, ARM64) are also available on the [GitHub Releases](https://github.com/krakenfx/kraken-cli/releases) page.

Verify: `kraken --version`

## Authentication

### Environment variables (recommended for agents)

```bash
export KRAKEN_API_KEY="your-key"
export KRAKEN_API_SECRET="your-secret"

# Futures (optional, separate credentials)
export KRAKEN_FUTURES_API_KEY="your-futures-key"
export KRAKEN_FUTURES_API_SECRET="your-futures-secret"
```

No config file needed. No interactive prompts. The CLI reads credentials from the environment.

### Credential resolution order

1. Command-line flags (`--api-key`, `--api-secret`)
2. Environment variables (`KRAKEN_API_KEY`, `KRAKEN_API_SECRET`)
3. Config file (`~/.config/kraken/config.toml`)

### Required permissions by command group

| Group | Kraken API permissions |
|-------|----------------------|
| market | None (public) |
| account | Query Funds, Query Open Orders & Trades, Query Closed Orders & Trades, Query Ledger Entries |
| trade | Create & Modify Orders, Cancel/Close Orders |
| funding | Deposit Funds, Withdraw Funds, Query Funds |
| earn | Query Funds, Create & Modify Orders |
| subaccount | Access & Modify Account Settings |
| websocket (public) | None |
| websocket (private) | Same as account + trade |
| futures | Access Futures Markets, Create & Modify Orders, Cancel/Close Orders |
| paper | None (local simulation) |
| futures paper | None (local simulation) |

## Invocation Pattern

Every command follows the same pattern:

```bash
kraken <command> [args...] -o json 2>/dev/null
```

Rules:
- Always pass `-o json` for structured output.
- Redirect stderr: `2>/dev/null` or `2>debug.log`. Diagnostics there are structured JSON (JSONL), off by default bar warnings/errors; enable detail with `-v` or `RUST_LOG`.
- stdout contains only valid JSON (success) or a JSON error envelope (runtime failure).
- Exit code 0 = success, 1 = runtime failure with a JSON envelope on stdout, 2 = argument/usage error from the parser. With `-o json`, exit 2 puts a `{"error":"validation"}` envelope on stdout (stderr stays empty); without it, clap's usage text goes to stderr. Validate flag values against the `values` arrays in `agents/tool-catalog.json` before invoking.

### Basic examples

```bash
# Public data (no auth)
kraken ticker BTCUSD -o json
kraken orderbook BTCUSD --count 10 -o json
kraken ohlc BTCUSD --interval 60 -o json

# Private data
kraken balance -o json
kraken open-orders -o json

# Trading
kraken order buy BTCUSD 0.001 --type limit --price 50000 -o json
kraken order cancel TXID123 -o json

# Spot paper trading (no auth)
kraken workspace create sandbox --capital 10000 --mode paper -o json
kraken paper buy BTCUSD 0.01 -o json
kraken workspace status sandbox -o json

# Futures paper trading (no auth)
kraken futures paper init --balance 10000 -o json
kraken futures paper buy PF_XBTUSD 1 --leverage 10 --type market -o json
kraken futures paper positions -o json
```

## Asset Classes

The CLI supports six asset classes through the same command surface. Most commands default to crypto. For other asset classes, pass the `--asset-class` flag.

### Crypto spot (default)

No flag needed. Covers 1,400+ pairs with margin up to 10x on major pairs.

```bash
kraken ticker BTCUSD -o json
kraken order buy BTCUSD 0.001 --type limit --price 50000 -o json
kraken order buy BTCUSD 0.001 --type limit --price 50000 --leverage 5 -o json
```

### Tokenized U.S. stocks & ETFs (xStocks)

Requires `--asset-class tokenized_asset`. Stock symbols use the `x` suffix: AAPL becomes `AAPLx`, TSLA becomes `TSLAx`. 79 assets available with margin up to 3x on the top 10. Not available in the USA.

```bash
kraken ticker AAPLx/USD --asset-class tokenized_asset -o json
kraken order buy AAPLx/USD 0.1 --type limit --price 200 --asset-class tokenized_asset -o json
kraken order buy TSLAx/USD 0.5 --type limit --price 250 --asset-class tokenized_asset --leverage 3 -o json
```

To list all xStocks assets: `kraken assets --asset-class tokenized_asset -o json`

### Forex

Requires `--asset-class forex` on market data commands. 11 fiat pairs including EUR/USD, GBP/USD, USD/JPY, AUD/USD.

```bash
kraken ticker EURUSD --asset-class forex -o json
```

### Futures

Separate engine with its own credentials (`KRAKEN_FUTURES_API_KEY`, `KRAKEN_FUTURES_API_SECRET`). Covers crypto perpetuals, 5 forex perps (PF_EURUSD, PF_GBPUSD, PF_AUDUSD, PF_CHFUSD, PF_JPYUSD), 11 equity/index perps (PF_AAPLXUSD, PF_NVDAXUSD, PF_TSLAXUSD, PF_SPYXUSD, PF_QQQXUSD, PF_SPXUSD, and others), and fixed-date contracts. Leverage up to 50x.

```bash
kraken futures instruments -o json                                            # list all contracts
kraken futures order buy PF_XBTUSD 1 --type limit --price 50000 -o json      # crypto perp
kraken futures order buy PF_AAPLXUSD 10 --type limit --price 200 -o json     # equity perp
kraken futures order buy FI_XBTUSD_260327 1 --type limit --price 50000 -o json  # fixed-date
```

### Earn / staking

```bash
kraken earn strategies --asset ETH -o json
kraken earn allocate <STRATEGY_ID> 1.0 -o json
```

## Output Parsing

### Success response

stdout contains the API response as JSON. Structure varies by command.

```json
{"BTCUSD":{"a":["67234.10","1","1.000"],"b":["67234.00","1","1.000"]}}
```

### Error response

On exit 1, stdout contains a JSON error envelope:

```json
{"error": "auth", "message": "Authentication failed: EAPI:Invalid key"}
```

The `error` field is a stable category code. The `message` field is human-readable and not stable.

Argument and usage errors exit 2. When `-o json` is requested they render the same `{"error":"validation"}` envelope on stdout; otherwise the parser prints usage text on stderr. Catch them up front by checking values against the `values` arrays in `agents/tool-catalog.json`.

### WebSocket streaming

WebSocket commands emit JSONL (one JSON object per line):

```bash
kraken ws ticker BTC/USD -o json | while read -r line; do
  echo "$line" | jq -r '.data[0].last'
done
```

## Error Handling

Route on the `error` field, not on message strings.

| Category | Retryable | Agent action |
|----------|-----------|-------------|
| `api` | No | Fix parameters. Do not retry same request. |
| `auth` | No | Re-authenticate. Check KRAKEN_API_KEY and KRAKEN_API_SECRET. |
| `network` | Yes | Exponential backoff, max 5 retries, starting at 1s. |
| `rate_limit` | Yes | Error returned immediately (no CLI retry). Includes `suggestion` (tier-specific guidance), `docs_url` (Kraken documentation link), and `retryable` flag. Read `suggestion` and `docs_url` to understand the limit and adapt your strategy. |
| `validation` | No | Fix input. Check types, required fields, allowed values. |
| `config` | No | Check config file or environment variables. |
| `websocket` | Yes | Streams reconnect with paced exponential backoff (capped at 30s, plus jitter) until interrupted; a subscription rejected again on a fresh token is dropped from the replay so the rest keeps streaming. One-shot methods do not retry past their own timeout. |
| `io` | No | Check file paths and permissions. |
| `parse` | No | Log raw response. Possible API maintenance. |

### Error handling pattern

```bash
RESULT=$(kraken balance -o json 2>/dev/null)
EXIT=$?

if [ $EXIT -eq 0 ]; then
  echo "$RESULT" | jq .
else
  CATEGORY=$(echo "$RESULT" | jq -r '.error // "unknown"')
  case "$CATEGORY" in
    auth)       echo "Re-authenticate" ;;
    rate_limit)
      SUGGESTION=$(echo "$RESULT" | jq -r '.suggestion // "Wait and retry"')
      DOCS=$(echo "$RESULT" | jq -r '.docs_url // ""')
      echo "Rate limited. $SUGGESTION"
      echo "Docs: $DOCS"
      ;;
    network)    sleep 5; echo "Retry" ;;
    *)          echo "Failed: $CATEGORY" ;;
  esac
fi
```

For the full error catalog with backoff strategies and example envelopes, see `agents/error-catalog.json`.

## Feedback

When the CLI's interface or responses do not match what you are trying to do — including a mismatch between a skill and this CLI version's contract — submit feedback. Submitting uploads the text (and metadata such as CLI version, instance id, and optional `--llm-model`) to Kraken over HTTPS. A local DuckDB copy under the Kraken config directory is written only after a successful upload. A failed upload fails the command and writes nothing locally. Do not include API keys, secrets, passwords, or personal data in the body. Architecture, limits, and retention: `docs/feedback-ingest.md`.

```bash
kraken feedback bug --input-string "description; steps; versions" --llm-model <your-model> -o json
kraken feedback friction --input-file ./notes.txt -o json
kraken feedback feature-request --input-string "I need X because Y" -o json
```

Types: `feature-request`, `friction`, `bug`. Soft recommendations for body content (and the 8192-byte limit) are documented in `kraken feedback --help`.

## Rate Limiting

The CLI does not pre-throttle or retry rate-limited requests. The Kraken API server enforces rate limits, and the CLI returns the error immediately with structured fields that agents should read:

- `suggestion`: specific guidance including tier limits, per-endpoint costs, and recommended alternatives
- `docs_url`: link to the relevant Kraken rate limit documentation
- `retryable`: `true` for all rate limit errors (the agent decides whether and when to retry)

### Kraken rate limit systems

Kraken enforces multiple independent rate limit systems. The `suggestion` field in rate limit errors describes which system was triggered and its parameters. For full documentation:

- **Spot REST counter**: [docs.kraken.com/api/docs/guides/spot-rest-ratelimits](https://docs.kraken.com/api/docs/guides/spot-rest-ratelimits/)
- **Spot trading engine (per-pair)**: [docs.kraken.com/api/docs/guides/spot-ratelimits](https://docs.kraken.com/api/docs/guides/spot-ratelimits)
- **Futures cost-based budget**: [docs.kraken.com/api/docs/guides/futures-rate-limits](https://docs.kraken.com/api/docs/guides/futures-rate-limits/)

### Agent recommendations

- Use WebSocket streaming instead of REST polling for real-time data (no rate limit cost).
- Batch order operations where possible (up to 15 per batch; lower per-order cost on both Spot and Futures).
- Let orders rest before cancelling; cancelling within 5 seconds costs +8 on the trading engine limiter.
- When a `rate_limit` error is returned, read the `suggestion` field and adapt your strategy accordingly.

## Paper Trading

Two independent paper engines cover spot and futures. No API keys, no account, no real money. Both use live Kraken prices.

### Spot Paper

```bash
kraken workspace create sandbox --capital 10000 --mode paper --slippage-rate 0.001 -o json   # Create AND fund the account
export KRAKEN_WORKSPACE=sandbox
kraken paper buy BTCUSD 0.01 -o json        # Market buy
kraken paper sell BTCUSD 0.005 --type limit --price 70000 -o json  # Limit sell
kraken workspace status -o json             # Check portfolio
kraken paper orders -o json                 # Check open orders
kraken paper history -o json                # Trade history
kraken workspace reset -o json --yes        # Return to starting capital
```

Switch between paper and live by changing the prefix: `kraken paper buy` vs `kraken order buy`.

A 0.26% taker fee is applied to all fills (Kraken Starter tier default, configurable with `--fee-rate`). Optional slippage simulation is available via `--slippage-rate` (default 0.0). Market buys fill at `ask * (1 + slippage_rate)`, sells at `bid * (1 - slippage_rate)`. Limit orders fill at the limit price when the live market crosses the order price. Use `kraken workspace status -o json` to check current fee and slippage rates; `paper init/reset/status/balance` remain as aliases of the workspace commands.

### Futures Paper

Near-parity with live `kraken futures order` commands. Supports all 8 order types, leverage/margin tracking, position aggregation/netting, liquidation simulation, and funding rate accrual. Known differences: single-ID `order-status`, post-only orders are cancelled rather than queued, fills use the bid/ask snapshot with no depth-based slippage, and partial fills are not modeled.

```bash
kraken futures paper init --balance 10000 -o json                              # Create account
kraken futures paper buy PF_XBTUSD 1 --leverage 10 --type market -o json      # Market long
kraken futures paper sell PF_ETHUSD 5 --leverage 20 --type market -o json     # Market short
kraken futures paper buy PF_XBTUSD 1 --leverage 10 --type limit --price 50000 -o json  # Limit long
kraken futures paper sell PF_XBTUSD 1 --leverage 10 --type stop --stop-price 48000 --trigger-signal mark -o json  # Stop order
kraken futures paper positions -o json                                         # Open positions
kraken futures paper balance -o json                                           # Margin state
kraken futures paper fills -o json                                             # Fill history
kraken futures paper status -o json                                            # Account summary
kraken futures paper reset -o json                                             # Reset account
```

Switch between paper and live by replacing `futures paper` with `futures order`: `kraken futures paper buy` vs `kraken futures order buy`.

Spot paper and futures paper are fully independent. Resetting one does not affect the other.

## Market Memory & Stream Reliability

Recording and streaming are CLI-only surfaces — a request/response MCP tool cannot host a live stream (see [Integration](#integration)). Agents run them as background processes and read the lifecycle from stderr; `kraken tape list` is a regular MCP tool in the market service.

```bash
kraken record --symbols BTC/USD --channels trades,book --tape my-tape --to duckdb,jsonl -o json &
RECORDER=$!
# ... let the window run ...
kill -TERM $RECORDER   # graceful stop: every backend finalizes
```

The lifecycle contract, as stderr JSONL (stdout stays empty unless `--to` includes `stdout`):

- `{"event":"record","type":"started","tape":"<path>","format":"duckdb",...}` — one line per backend, emitted after locks and schema are ensured, before any socket opens.
- `{"event":"record","type":"summary","tapes":[...]}` — only for a capture that actually finalized. SIGINT and SIGTERM both finalize: the tape metadata gains `window_end`, `events_dropped`, `frames_unparsed`, and `reconnect_count`.
- One writer per tape: a second `record` on the same tape exits 1 with a `validation` envelope (`tape '<path>' is in use by another process`). Readers coexist; `kraken tape list -o json` reports `in_use` and `state` per backend.
- Re-recording an existing tape appends to it and extends its window.

Add `--monitor` to `ws`, `streamd`, or `record` for reliability diagnostics as stderr JSONL, routable by `type`: periodic `health` (connection status, per-channel latency percentiles, trade-gap counters, staleness ages), `alert` (stale data, heartbeat timeout), and a final `summary`. Tune with `--monitor-health-interval`, `--monitor-stale-after`, and `--monitor-heartbeat-timeout` (seconds).

A finalized tape is a replay source: `kraken session start --from tape:my-tape --speed 10` replays it into a paper session. The [market memory walkthrough](examples/03-market-memory.md) covers the interactive loop.

## Command Groups Overview

| Group | Auth | Commands | Description |
|-------|------|----------|-------------|
| market | No | 11 | Public market data plus the local tape-library catalog |
| account | Yes | 18 | Balances, orders, trades, ledgers, positions, export, L3 orderbook |
| trade | Yes | 9 | Order placement, amendment, cancellation (spot, xStocks, forex) |
| funding | Yes | 10 | Deposits, withdrawals, wallet transfers |
| earn | Yes | 6 | Staking strategies, allocations |
| subaccount | Yes | 2 | Create subaccounts, transfer between accounts |
| futures | Mixed | 39 | Futures market data (public) and trading (private) |
| futures-paper | No | 17 | Futures paper trading: all 8 order types, leverage, margin, liquidation |
| futures-ws | Mixed | 9 | Futures WebSocket streaming |
| websocket | Mixed | 15 | Spot WebSocket v2 streaming and request/response |
| paper | No | 16 | Spot paper trading simulation, P&L explanation, and the Autoresearch Lab |
| workspace | No | 15 | Strategy workspaces and recorded sessions: isolated accounts, session windows, decision logs, reports, promotion |
| auth | No | 4 | Credential management (set, show, test, reset) |
| utility | No | 2 | Interactive setup and REPL shell |

Catalog: 174 request/response commands. See `agents/tool-catalog.json`; CLI-only streams are listed under [Integration](#integration).

`kraken mcp` is a runtime mode that starts an MCP server, not a tool-callable command. It is not included in the catalog.

## Dangerous Commands

The catalog marks 41 commands as `dangerous: true`. Four of them (`order buy/sell/cancel/cancel-all`) also carry `paper_safe: true` — inside a paper workspace (KRAKEN_WORKSPACE set to a paper account) they execute simulated fills and need no confirmation; against the real account they stay gated. These move real money, cancel real orders, or mutate account state. Gate every one with confirmation logic.

The authoritative source is the `dangerous` field in `agents/tool-catalog.json`.

Common dangerous command groups:

| Group | Commands |
|-------|----------|
| Spot orders | `order buy`, `order sell`, `order batch`, `order amend`, `order edit`, `order cancel`, `order cancel-batch`, `order cancel-all`, `order cancel-after` |
| Funding | `withdraw`, `withdrawal cancel`, `wallet-transfer` |
| Earn | `earn allocate`, `earn deallocate` |
| Subaccounts | `subaccount transfer`, `subaccount create` |
| Futures | `futures order buy/sell`, `futures edit-order`, `futures cancel`, `futures cancel-all`, `futures cancel-after`, `futures batch-order`, `futures transfer`, `futures wallet-transfer`, `futures set-subaccount-status`, `futures set-leverage`, `futures set-pnl-preference` |
| Account data | `export-retrieve`, `export-delete` |
| WebSocket | `ws add-order`, `ws amend-order`, `ws cancel-order`, `ws cancel-all`, `ws cancel-after`, `ws batch-add`, `ws batch-cancel` |

### Agent Autonomy Levels

Traders can give agents increasing levels of control:

| Level | Agent capability | API key scope |
|-------|-----------------|---------------|
| Read-only | Market data and account queries | Query permissions only |
| Paper trading | Strategy testing with live prices | No key needed |
| Supervised | Trades with human confirmation on each order | Trade permissions |
| Autonomous | Trades independently, no human in the loop | Trade permissions |
| Full control | Trades and moves funds | All permissions |

The CLI supports all levels today. The trader controls the boundary by choosing which API key permissions to grant and whether the agent passes `--yes` to skip prompts. See `skills/kraken-autonomy-levels/SKILL.md` for the full progression guide with safeguards at each level.

### Dry-run support

Use `--validate` on order commands to validate without submitting:

```bash
kraken order buy BTCUSD 0.001 --type limit --price 50000 --validate -o json
```

Use paper trading for full lifecycle testing:

```bash
kraken paper buy BTCUSD 0.001 -o json                                 # spot
kraken futures paper buy PF_XBTUSD 1 --leverage 10 --type market -o json  # futures
```

## Integration

### Calling kraken-cli from agent code

```python
import json, subprocess

def call_kraken(command_args: list[str]) -> dict:
    result = subprocess.run(
        ["kraken"] + command_args + ["-o", "json"],
        capture_output=True, text=True, timeout=30
    )
    output = json.loads(result.stdout) if result.stdout.strip() else {}
    if result.returncode != 0:
        return {"error": "subprocess", "exit_code": result.returncode, "response": output}
    return output
```

### Discovering commands

Load `agents/tool-catalog.json` for the full command contract. Each entry includes the command template, parameter schemas, auth requirements, and a `dangerous` flag for safety gating.

```python
import json

with open("agents/tool-catalog.json") as f:
    catalog = json.load(f)

# All 174 commands with full parameter schemas
commands = catalog["commands"]

# Filter to safe read-only commands
safe = [c for c in commands if not c.get("dangerous") and not c.get("auth_required")]
```

### MCP (built-in server)

`kraken-cli` ships a built-in MCP server over stdio. No subprocess wrappers needed.

Security note:
- MCP is intended for local use on systems you control.
- All connected agents operate with the same configured Kraken account privileges.
- Do not expose or share this MCP server on network-accessible hosts.
- Always use `https://` and `wss://` endpoints.
- Current release posture is alpha, run with least-privilege API keys.

```bash
# Default mode: read-only services (market, account, paper)
kraken mcp

# Guarded mode: all services, dangerous calls require acknowledged=true
kraken mcp -s all

# Autonomous mode: all services, no per-call dangerous confirmation
kraken mcp -s all --allow-dangerous

# Start with specific services
kraken mcp -s market,trade,paper
```

Configure your MCP client:

```json
{
  "mcpServers": {
    "kraken": {
      "command": "kraken",
      "args": ["mcp", "-s", "all"]
    }
  }
}
```

**Service groups**: `market`, `account`, `trade`, `funding`, `earn`, `subaccount`, `futures`, `futures-paper`, `paper`, `auth`, `feedback`, or `all`. Default exposed groups are `market,account,paper,feedback`.

**Streaming commands are CLI-only.** `session start` (and bare `kraken playground`, its demo composition), `record`, `replay`, the `streamd` verbs, and `ws ping` never appear as MCP tools — a request/response tool cannot host a live stream. An MCP-only agent gets the whole loop *around* a session (order, note, state, stop, show, score, explain), but opening one takes a shell: background `kraken session start … -o json &` and parse the `session_started` line for the handle.

**Behavior:**
- Streaming groups (`websocket`, `futures-ws`) are excluded in MCP v1 (REST-only tools).
- MCP tool execution uses the same command dispatch path as CLI, so error handling and rate-limit behavior is identical.
- `utility` (interactive setup/shell) is not available as an MCP service.
- Dangerous tools are annotated with `destructive_hint: true` and include `[DANGEROUS: requires human confirmation]` in the description.
- In guarded mode (default), dangerous calls must include `acknowledged=true`.
- In autonomous mode (`--allow-dangerous`), the per-call confirmation is disabled.
- `auth set` and `auth reset` are excluded from MCP registration because they rely on secret input or local credential deletion, both unsuitable for MCP's stateless tool-call model. Configure credentials via environment variables or config file instead.
- All tool execution runs with `force=true` (no interactive prompts).
- Tool names follow the pattern `kraken_<command>` with spaces and hyphens converted to underscores (e.g., `kraken_server_time`, `kraken_order_buy`).
- Input schemas are derived from clap argument metadata. Tool descriptions come from clap help text.
- Errors from tool execution are returned as MCP tool-execution errors, not as panics.

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `KRAKEN_API_KEY` | For private endpoints | Spot API key |
| `KRAKEN_API_SECRET` | For private endpoints | Spot API secret |
| `KRAKEN_FUTURES_API_KEY` | For futures private endpoints | Futures API key |
| `KRAKEN_FUTURES_API_SECRET` | For futures private endpoints | Futures API secret |
| `KRAKEN_SPOT_URL` | No | Override Spot REST API URL |
| `KRAKEN_FUTURES_URL` | No | Override Futures REST API URL |
| `KRAKEN_WS_PUBLIC_URL` | No | Override public WebSocket URL |
| `KRAKEN_WS_AUTH_URL` | No | Override authenticated WebSocket URL |
| `KRAKEN_FUTURES_WS_URL` | No | Override Futures WebSocket URL |

## Machine-Readable Resources

| File | Format | Description |
|------|--------|-------------|
| `agents/tool-catalog.json` | JSON | All 174 commands with parameters, types, safety flags, and examples |
| `agents/error-catalog.json` | JSON | 10 error categories with retry guidance |
| `agents/examples/` | Shell | Runnable workflow examples |
| `skills/` | SKILL.md | Goal-oriented workflow skills for agents |
