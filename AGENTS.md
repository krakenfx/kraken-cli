# Agent Integration Guide: kraken-cli

> **This is experimental software. Commands execute real financial transactions
> on the Kraken exchange. Test with `kraken paper` before using real funds. See
> [DISCLAIMER.md](DISCLAIMER.md) for full terms.**

Self-contained guide for integrating `kraken-cli` into AI agents, MCP clients,
and automated pipelines.

Fast entry points:

- Runtime agent context: `CONTEXT.md`
- Full command contract: `agents/tool-catalog.json`
- Error routing contract: `agents/error-catalog.json`
- Workflow skills: `skills/`

## Installation

One install, no runtime dependencies. It puts `kraken`, `kraken-mcp` (the MCP
server) and `kraken-shell` (an interactive shell) on your PATH.

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/krakenfx/kraken-cli/releases/latest/download/kraken-cli-installer.sh | sh
```

Pre-built binaries for macOS (Apple Silicon, Intel) and Linux (x86_64, ARM64)
are also available on the
[GitHub Releases](https://github.com/krakenfx/kraken-cli/releases) page.

Verify: `kraken --version && kraken-mcp --version`

The installer adds `~/.cargo/bin` (or `$CARGO_HOME/bin`, if `CARGO_HOME` is set)
to PATH for new terminals only. If `kraken` is not found right after install,
your session's PATH predates it: call `~/.cargo/bin/kraken` (or
`$CARGO_HOME/bin/kraken`) by its full path until the user restarts the session.

## Authentication

### Environment variables (recommended for agents)

```bash
export KRAKEN_API_KEY="your-key"
export KRAKEN_API_SECRET="your-secret"

# Futures (optional, separate credentials)
export KRAKEN_FUTURES_API_KEY="your-futures-key"
export KRAKEN_FUTURES_API_SECRET="your-futures-secret"
```

No config file needed. No interactive prompts. The CLI reads credentials from
the environment.

### Credential resolution order

1. Command-line flags (`--api-key`, `--api-secret`)
2. Environment variables (`KRAKEN_API_KEY`, `KRAKEN_API_SECRET`)
3. The active stored key in `config.toml` (`~/.config/kraken/` on Linux,
   `~/Library/Application Support/kraken/` on macOS): the key from
   `kraken auth set`, or the sign-in key from `kraken auth login`. Without a
   choice, the key from `kraken auth set` is active.
   `kraken auth switch api-key` or `kraken auth switch sign-in` changes it.

### Required permissions by command group

| Group               | Kraken API permissions                                                                      |
| ------------------- | ------------------------------------------------------------------------------------------- |
| market              | None (public)                                                                               |
| account             | Query Funds, Query Open Orders & Trades, Query Closed Orders & Trades, Query Ledger Entries |
| trade               | Create & Modify Orders, Cancel/Close Orders                                                 |
| funding             | Deposit Funds, Withdraw Funds, Query Funds                                                  |
| earn                | Query Funds, Create & Modify Orders                                                         |
| subaccount          | Access & Modify Account Settings                                                            |
| websocket (public)  | None                                                                                        |
| websocket (private) | Same as account + trade                                                                     |
| futures             | Access Futures Markets, Create & Modify Orders, Cancel/Close Orders                         |
| paper               | None (local simulation)                                                                     |
| futures paper       | None (local simulation)                                                                     |

## Invocation Pattern

Every command follows the same pattern:

```bash
kraken <command> [args...] -o json 2>/dev/null
```

Rules:

- Always pass `-o json` for structured output.
- Redirect stderr: `2>/dev/null` or `2>debug.log`. Diagnostics there are
  structured JSON (JSONL), off by default bar warnings/errors; enable detail
  with `-v` or `RUST_LOG`. A few notes, such as the live-order banner, are plain
  text, so do not parse stderr as strict JSONL.
- stdout contains only valid JSON (success) or a JSON error envelope (runtime
  failure).
- Exit code 0 = success, 1 = runtime failure with a JSON envelope on stdout, 2 =
  argument/usage error from the parser. With `-o json`, exit 2 puts a
  `{"error":"validation"}` envelope on stdout (stderr stays empty); without it,
  clap's usage text goes to stderr. Validate values against each command's
  `input_schema` in `agents/tool-catalog.json` before invoking.

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
export KRAKEN_WORKSPACE=sandbox
kraken paper buy BTCUSD 0.01 -o json
kraken workspace status sandbox -o json

# Futures paper trading (no auth; runs outside a workspace)
unset KRAKEN_WORKSPACE
kraken futures paper init --balance 10000 -o json
kraken futures paper buy PF_XBTUSD 1 --leverage 10 --type market -o json
kraken futures paper positions -o json
```

## Asset Classes

The CLI supports six asset classes through the same command surface; futures
covers two of them, perpetual and inverse/fixed-date. Most commands default to
crypto. For other asset classes, pass the `--asset-class` flag.

### Crypto spot (default)

No flag needed. Covers 1,400+ pairs with margin up to 10x on major pairs.

```bash
kraken ticker BTCUSD -o json
kraken order buy BTCUSD 0.001 --type limit --price 50000 -o json
kraken order buy BTCUSD 0.001 --type limit --price 50000 --leverage 5 -o json
```

### Tokenized U.S. stocks & ETFs (xStocks)

Requires `--asset-class tokenized_asset`. Stock symbols use the `x` suffix: AAPL
becomes `AAPLx`, TSLA becomes `TSLAx`. 79 assets available with margin up to 3x
on the top 10. Not available in the USA.

```bash
kraken ticker AAPLx/USD --asset-class tokenized_asset -o json
kraken order buy AAPLx/USD 0.1 --type limit --price 200 --asset-class tokenized_asset -o json
kraken order buy TSLAx/USD 0.5 --type limit --price 250 --asset-class tokenized_asset --leverage 3 -o json
```

To list all xStocks assets:
`kraken assets --asset-class tokenized_asset -o json`

### Forex

Requires `--asset-class forex` on market data commands. 11 fiat pairs including
EUR/USD, GBP/USD, USD/JPY, AUD/USD.

```bash
kraken ticker EURUSD --asset-class forex -o json
```

### Futures

Separate engine with its own credentials (`KRAKEN_FUTURES_API_KEY`,
`KRAKEN_FUTURES_API_SECRET`). Covers crypto perpetuals, 5 forex perps
(PF_EURUSD, PF_GBPUSD, PF_AUDUSD, PF_CHFUSD, PF_JPYUSD), 11 equity/index perps
(PF_AAPLXUSD, PF_NVDAXUSD, PF_TSLAXUSD, PF_SPYXUSD, PF_QQQXUSD, PF_SPXUSD, and
others), and fixed-date contracts. Leverage up to 50x.

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

stdout contains the command's typed output as JSON, not the raw Kraken wire
shape. Structure varies by command. `kraken ticker BTCUSD -o json` is keyed by
the venue pair name and carries named numeric fields (abridged here):

```json
{
  "XXBTZUSD": {
    "ask_price": 81401.4,
    "bid_price": 81401.3,
    "last_price": 81401.3,
    "volume_24h": 1371.76541087
  }
}
```

### Error response

On exit 1, stdout contains a JSON error envelope:

```json
{ "error": "auth", "message": "Authentication failed: EAPI:Invalid key" }
```

The `error` field is a stable category code. The `message` field is
human-readable and not stable.

Argument and usage errors exit 2. When `-o json` is requested they render the
same `{"error":"validation"}` envelope on stdout; otherwise the parser prints
usage text on stderr. Catch them up front by checking values against each
command's `input_schema` in `agents/tool-catalog.json`.

### WebSocket streaming

WebSocket commands emit JSONL (one JSON object per line):

```bash
kraken ws ticker BTC/USD -o json | while read -r line; do
  echo "$line" | jq -r '.data[0].last'
done
```

When the stream reconnects mid-run (see the `websocket` row below), stdout
carries one marker line before the fresh data:

```json
{ "endpoint": "public", "snapshot_follows": true, "type": "reconnected" }
```

`snapshot_follows: true` means the next frames on that endpoint are full
snapshots — discard any book or ticker state you accumulated for it. The marker
appears on every streaming stdout (`ws`, `streamd`, `session start`,
`playground`, `recipe run`, `record --to stdout`) but never inside recorded
tapes, which hold frames only. `futures ws` emits no marker.

## Error Handling

Route on the `error` field, not on message strings.

| Category     | Retryable | Agent action                                                                                                                                                                                                                                                  |
| ------------ | --------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `api`        | No        | Fix parameters. Do not retry same request.                                                                                                                                                                                                                    |
| `auth`       | No        | Re-authenticate. Check KRAKEN_API_KEY and KRAKEN_API_SECRET.                                                                                                                                                                                                  |
| `network`    | Yes       | Exponential backoff, max 5 retries, starting at 1s. If `verify_first` is `true`, the order or transfer may have executed: check its state before any retry.                                                                                                   |
| `rate_limit` | Yes       | Error returned immediately (no CLI retry). Includes `suggestion` (tier-specific guidance), `docs_url` (Kraken documentation link), and `retryable` flag. Read `suggestion` and `docs_url` to understand the limit and adapt your strategy.                    |
| `validation` | No        | Fix input. Check types, required fields, allowed values.                                                                                                                                                                                                      |
| `config`     | No        | Check config file or environment variables.                                                                                                                                                                                                                   |
| `websocket`  | Yes       | Spot streams reconnect (backoff capped at 30s, jittered) until interrupted and drop a twice-rejected subscription; `futures ws` stops after 12 consecutive failed reconnects. One-shot calls never retry: if `verify_first` is true, check order state first. |
| `io`         | No        | Check file paths and permissions.                                                                                                                                                                                                                             |
| `timeout`    | Yes       | A wait ran out: `session stop` on a recorder's lock, a WebSocket token request, or an unfinished browser sign-in. Re-run; if `session stop` repeats it, check `kraken session show`.                                                                          |
| `parse`      | No        | Log raw response. Possible API maintenance.                                                                                                                                                                                                                   |

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

For the full error catalog with backoff strategies and example envelopes, see
`agents/error-catalog.json`.

## Feedback

When the CLI's interface or responses do not match what you are trying to do —
including a mismatch between a skill and this CLI version's contract — submit
feedback. Submitting uploads the text (and metadata such as CLI version,
instance id, and optional `--llm-model`) to Kraken over HTTPS. A failed upload
fails the command. Do not include API keys, secrets, passwords, or personal data
in the body. Kraken keeps feedback for 90 days.

```bash
kraken feedback bug --input-string "description; steps; versions" --llm-model <your-model> -o json
kraken feedback friction --input-file ./notes.txt -o json
kraken feedback feature-request --input-string "I need X because Y" -o json
```

Types: `feature-request`, `friction`, `bug`. Soft recommendations for body
content (and the 8192-byte limit) are documented in `kraken feedback --help`.

## Rate Limiting

The CLI does not pre-throttle or retry rate-limited requests. The Kraken API
server enforces rate limits, and the CLI returns the error immediately with
structured fields that agents should read:

- `suggestion`: specific guidance including tier limits, per-endpoint costs, and
  recommended alternatives
- `docs_url`: link to the relevant Kraken rate limit documentation
- `retryable`: `true` for all rate limit errors (the agent decides whether and
  when to retry)

### Kraken rate limit systems

Kraken enforces multiple independent rate limit systems. The `suggestion` field
in rate limit errors describes which system was triggered and its parameters.
For full documentation:

- **Spot REST counter**:
  [docs.kraken.com/api/docs/guides/spot-rest-ratelimits](https://docs.kraken.com/api/docs/guides/spot-rest-ratelimits/)
- **Spot trading engine (per-pair)**:
  [docs.kraken.com/api/docs/guides/spot-ratelimits](https://docs.kraken.com/api/docs/guides/spot-ratelimits)
- **Futures cost-based budget**:
  [docs.kraken.com/api/docs/guides/futures-rate-limits](https://docs.kraken.com/api/docs/guides/futures-rate-limits/)

### Agent recommendations

- Use WebSocket streaming instead of REST polling for real-time data (no rate
  limit cost).
- Batch order operations where possible (up to 15 per batch; lower per-order
  cost on both Spot and Futures).
- Let orders rest before cancelling; cancelling within 5 seconds costs +8 on the
  trading engine limiter.
- When a `rate_limit` error is returned, read the `suggestion` field and adapt
  your strategy accordingly.

## Paper Trading

Two independent paper engines cover spot and futures. No API keys, no account,
no real money. Both use live Kraken prices.

### Spot Paper

```bash
kraken workspace create sandbox --capital 10000 --mode paper --slippage-rate 0.001 -o json   # Create AND fund the account
export KRAKEN_WORKSPACE=sandbox
kraken paper buy BTCUSD 0.01 -o json        # Market buy
kraken paper sell BTCUSD 0.005 --type limit --price 70000 -o json  # Limit sell
kraken workspace status -o json             # Check portfolio
kraken paper orders -o json                 # Check open orders
kraken paper history -o json                # Trade history
kraken workspace reset -o json --yes        # Return to starting capital (dangerous: confirm with the user first)
```

Switch between paper and live by changing the prefix: `kraken paper buy` vs
`kraken order buy`. `paper buy` defaults to a market order and `order buy` to a
limit order, so pass `--type` explicitly.

A 0.26% taker fee is applied to all fills (Kraken Starter tier default,
configurable with `--fee-rate`). Optional slippage simulation is available via
`--slippage-rate` (default 0.0). Market buys fill at
`ask * (1 + slippage_rate)`, sells at `bid * (1 - slippage_rate)`. A limit order
rests even when it is marketable, and fills at its limit price once the live
market crosses it, checked when the next paper command syncs. Use
`kraken workspace status -o json` to check current fee and slippage rates;
`paper init/reset/status/balance` remain as aliases of the workspace commands.

### Futures Paper

Near-parity with live `kraken futures order` commands. Supports all 8 order
types, leverage/margin tracking, position aggregation/netting, liquidation
simulation, and funding rate accrual. Known differences: single-ID
`order-status`, post-only orders are cancelled rather than queued, and fills use
the bid/ask snapshot with no depth-based slippage. Only IOC orders fill
partially: the size the book covers at the limit price fills, and the rest is
cancelled.

Futures paper keeps one account outside every workspace. With `KRAKEN_WORKSPACE`
set, it refuses everything except its reads (`balance`, `status`, `orders`,
`order-status`, `positions`, `fills`), so unset it first.

```bash
unset KRAKEN_WORKSPACE                                                         # Futures paper runs outside a workspace
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

The order verbs map from `kraken futures paper buy` to
`kraken futures order buy`, but the flags and reads differ: a live order takes
no `--leverage` (set it with `kraken futures set-leverage`), and live positions
and fills are `kraken futures positions` and `kraken futures fills`.

Spot paper and futures paper are fully independent. Resetting one does not
affect the other.

## Market Memory & Stream Reliability

Recording and streaming are CLI-only surfaces — a request/response MCP tool
cannot host a live stream (see [Integration](#integration)). Agents run them as
background processes and read the lifecycle from stderr; `kraken tape list` is a
regular MCP tool in the market service.

```bash
kraken record --symbols BTC/USD --channels trades,book --tape my-tape --to duckdb,jsonl -o json &
RECORDER=$!
# ... let the window run ...
kill -TERM $RECORDER   # graceful stop: every backend finalizes
```

The lifecycle contract, as stderr JSONL (stdout stays empty unless `--to`
includes `stdout`):

- `{"event":"record","type":"started","tape":"<path>","format":"duckdb",...}` —
  one line per backend, emitted after locks and schema are ensured, before any
  socket opens.
- `{"event":"record","type":"summary","tapes":[...]}` — only for a capture that
  actually finalized. SIGINT and SIGTERM both finalize: the tape metadata gains
  `window_end`, `events_dropped`, `frames_unparsed`, and `reconnect_count`.
- One writer per tape: a second `record` on the same tape exits 1 with a
  `validation` envelope (`tape '<path>' is in use by another process`). Readers
  coexist; `kraken tape list -o json` reports `in_use` and `state` per backend.
- Re-recording an existing tape appends to it and extends its window.

Add `--monitor` to `ws`, `streamd`, or `record` for reliability diagnostics as
stderr JSONL, routable by `type`: periodic `health` (connection status,
per-channel latency percentiles, trade-gap counters, staleness ages), `alert`
(stale data, heartbeat timeout), and a final `summary`. Tune with
`--monitor-health-interval`, `--monitor-stale-after`, and
`--monitor-heartbeat-timeout` (seconds).

A finalized tape is a replay source:
`kraken session start --from tape:my-tape --speed 10` replays it into a paper
session. The [market memory walkthrough](examples/03-market-memory.md) covers
the interactive loop.

## Command Groups Overview

| Group         | Auth  | Commands | Description                                                                                                                                     |
| ------------- | ----- | -------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| market        | No    | 11       | Public market data plus the local tape-library catalog                                                                                          |
| account       | Yes   | 18       | Balances, orders, trades, ledgers, positions, export, L3 orderbook                                                                              |
| trade         | Yes   | 9        | Order placement, amendment, cancellation (spot, xStocks, forex)                                                                                 |
| funding       | Yes   | 10       | Deposits, withdrawals, wallet transfers                                                                                                         |
| earn          | Yes   | 6        | Staking strategies, allocations                                                                                                                 |
| subaccount    | Yes   | 2        | Create subaccounts, transfer between accounts                                                                                                   |
| futures       | Mixed | 39       | Futures market data (public) and trading (private)                                                                                              |
| futures-paper | No    | 17       | Futures paper trading: all 8 order types, leverage, margin, liquidation                                                                         |
| futures-ws    | Mixed | 9        | Futures WebSocket streaming                                                                                                                     |
| websocket     | Mixed | 17       | Spot WebSocket v2 streaming and request/response                                                                                                |
| paper         | No    | 17       | Spot paper trading simulation, P&L explanation, and the Autoresearch Lab                                                                        |
| workspace     | No    | 25       | Strategy workspaces, recorded sessions, recipes, and extension scaffolds: isolated accounts, session windows, decision logs, reports, promotion |
| feedback      | No    | 1        | Product feedback upload to Kraken                                                                                                               |
| auth          | No    | 6        | Browser sign-in and credential management (login, set, show, switch, test, reset)                                                               |
| utility       | No    | 1        | Interactive setup (the REPL is the `kraken-shell` binary)                                                                                       |

Catalog: every command the CLI can run. Entries with a `tool` name are callable
over MCP; the rest carry `why` they are not. See `agents/tool-catalog.json`;
CLI-only streams are listed under [Integration](#integration).

`kraken-mcp` is a separate binary that starts an MCP server; it is not a command
of `kraken` and is not in the catalog.

## Dangerous Commands

The catalog marks 43 commands as `dangerous: true`. Four of them
(`order buy/sell/cancel/cancel-all`) also carry `paper_safe: true` — inside a
paper workspace (KRAKEN_WORKSPACE set to a paper account) they execute simulated
fills and need no confirmation; against the real account they stay gated. These
move real money, cancel real orders, or mutate account state. Gate every one
with confirmation logic.

The authoritative source is the `dangerous` field in `agents/tool-catalog.json`.
Two commands are conditionally dangerous and carry `effect: local_or_live`
instead: `workspace create --mode live` and `recipe run --real-account` write
local state until that argument points them at the real account, so gate them
whenever it is set.

Common dangerous command groups:

| Group        | Commands                                                                                                                                                                                                                                                                    |
| ------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Spot orders  | `order buy`, `order sell`, `order batch`, `order amend`, `order edit`, `order cancel`, `order cancel-batch`, `order cancel-all`, `order cancel-after`                                                                                                                       |
| Funding      | `withdraw`, `withdrawal cancel`, `wallet-transfer`                                                                                                                                                                                                                          |
| Earn         | `earn allocate`, `earn deallocate`                                                                                                                                                                                                                                          |
| Subaccounts  | `subaccount transfer`, `subaccount create`                                                                                                                                                                                                                                  |
| Futures      | `futures order buy/sell`, `futures edit-order`, `futures cancel`, `futures cancel-all`, `futures cancel-after`, `futures batch-order`, `futures transfer`, `futures wallet-transfer`, `futures set-subaccount-status`, `futures set-leverage`, `futures set-pnl-preference` |
| Account data | `export-retrieve`, `export-delete`                                                                                                                                                                                                                                          |
| WebSocket    | `ws add-order`, `ws amend-order`, `ws cancel-order`, `ws cancel-all`, `ws cancel-after`, `ws batch-add`, `ws batch-cancel`                                                                                                                                                  |
| Workspaces   | `workspace promote`, `workspace reset`, `paper reset`                                                                                                                                                                                                                       |
| Credentials  | `auth login`, `auth switch`, `auth reset`                                                                                                                                                                                                                                   |

### Agent Autonomy Levels

Traders can give agents increasing levels of control:

| Level         | Agent capability                             | API key scope          |
| ------------- | -------------------------------------------- | ---------------------- |
| Read-only     | Market data and account queries              | Query permissions only |
| Paper trading | Strategy testing with live prices            | No key needed          |
| Supervised    | Trades with human confirmation on each order | Trade permissions      |
| Autonomous    | Trades independently, no human in the loop   | Trade permissions      |
| Full control  | Trades and moves funds                       | All permissions        |

The CLI supports all levels today. The trader controls the boundary by choosing
which API key permissions to grant. Placing an order never prompts: `--yes` only
skips the confirmations on cancel-all, withdrawals and transfers, earn allocate
and deallocate, workspace reset and promote, and the live forms of
`workspace create` and `recipe run`. Per-order approval is the agent's job, or
the MCP server's in guarded mode. See `skills/kraken-autonomy-levels/SKILL.md`
for the full progression guide with safeguards at each level.

### Dry-run support

`order buy`, `order sell`, `order batch`, `ws add-order` and `ws batch-add` take
`--validate`, which checks an order without submitting it:

```bash
kraken order buy BTCUSD 0.001 --type limit --price 50000 --validate -o json
```

Futures orders, amends, edits and cancels have no dry run: gate them on the
user's approval. Use paper trading for full lifecycle testing:

```bash
kraken paper buy BTCUSD 0.001 --workspace sandbox -o json             # spot, in a paper workspace
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

Load `agents/tool-catalog.json` for the full command contract. Each entry
carries the invocation (`command`), its positional order (`usage`), a generated
`input_schema`, which credentials it needs (`credentials`), a worked `example`,
any cross-field `constraints`, and a `dangerous` flag for safety gating.

```python
import json

with open("agents/tool-catalog.json") as f:
    catalog = json.load(f)

# All 188 commands, each callable one with a generated argument schema
commands = catalog["commands"]

# Filter to safe public commands: `credentials` is "none", "spot", or "futures".
# `local_or_live` commands reach the real account when one argument is set, so
# check their arguments on every call instead.
safe = [
    c for c in commands
    if not c.get("dangerous")
    and c["effect"] != "local_or_live"
    and c["credentials"] == "none"
]
```

### MCP (built-in server)

`kraken-cli` ships a built-in MCP server over stdio. No subprocess wrappers
needed.

Security note:

- MCP is intended for local use on systems you control.
- All connected agents operate with the same configured Kraken account
  privileges.
- Do not expose or share this MCP server on network-accessible hosts.
- Always use `https://` and `wss://` endpoints.
- Current release posture is alpha, run with least-privilege API keys.

```bash
# Default services (market, account, paper, workspace, feedback), guarded mode
kraken-mcp

# All services, guarded mode: dangerous calls need approval through MCP
# elicitation, or acknowledged=true from the model where the client has none
kraken-mcp -s all

# Autonomous mode: all services, no per-call dangerous confirmation
kraken-mcp -s all --allow-dangerous

# Start with specific services
kraken-mcp -s market,trade,paper
```

Configure your MCP client:

```json
{
  "mcpServers": {
    "kraken": {
      "command": "kraken-mcp",
      "args": ["-s", "market,paper,feedback"]
    }
  }
}
```

Add services such as `trade` only with a least-privilege API key. If the client
shows "Failed to connect", use the full path from `command -v kraken-mcp` as
`command`.

**Service groups**: `market`, `account`, `trade`, `paper`, `workspace`,
`futures`, `futures-paper`, `funding`, `earn`, `auth`, `subaccount`, `feedback`,
or `all`. Default exposed groups are `market,account,paper,workspace,feedback`.

**Tool results.** `structuredContent` is always a JSON object. When a command's
output is an array or a scalar (for example `deposit status`), it arrives as
`{"result": <output>}`; the text block carries the output unchanged, as
`-o json` prints it. A failed call returns the CLI's error envelope,
`verify_first` included.

**Streaming commands are CLI-only.** `session start` (and bare
`kraken playground`, its demo composition), `record`, `replay`, the `streamd`
verbs, and `ws ping` never appear as MCP tools — a request/response tool cannot
host a live stream. An MCP-only agent gets the whole loop _around_ a session
(order, note, state, stop, show, score, explain), but opening one takes a shell:
background `kraken session start … -o json &` and parse the `session_started`
line for the handle.

**Behavior:**

- Streaming groups (`websocket`, `futures-ws`) are excluded in MCP v1 (REST-only
  tools).
- MCP tool execution uses the same command dispatch path as CLI, so error
  handling and rate-limit behavior is identical.
- `utility` (interactive setup/shell) is not available as an MCP service.
- Dangerous tools are annotated with `destructive_hint: true` and include
  `[DANGEROUS: requires human confirmation]` in the description.
- In guarded mode (default), a dangerous call is put to the operator through MCP
  elicitation; it runs only on approval. A client that declared no elicitation
  capability must include `acknowledged=true` in the arguments instead — that
  argument is the model's own word, so it stands in only where there is nobody
  to ask.
- In autonomous mode (`--allow-dangerous`), the per-call confirmation is
  disabled.
- `auth login`, `auth set`, `auth switch` and `auth reset` are excluded from MCP
  registration because they rely on a browser sign-in, secret input or local
  credential changes, all unsuitable for MCP's stateless tool-call model.
  Configure credentials via environment variables or config file instead.
- All tool execution runs with `force=true` (no interactive prompts).
- Tool names follow the pattern `kraken_<command>` with spaces and hyphens
  converted to underscores (e.g., `kraken_server_time`, `kraken_order_buy`).
- Input schemas are generated from each command's own Rust type
  (`kraken-contract`), so a renamed field reshapes the tool and the two cannot
  drift. Tool descriptions are the command's doc comment — the same text
  `--help` prints. Types are real: an integer argument is `integer`, a decimal
  accepts a string or a number.
- Errors from tool execution are returned as MCP tool-execution errors, not as
  panics.

## Environment Variables

| Variable                    | Required                      | Description                          |
| --------------------------- | ----------------------------- | ------------------------------------ |
| `KRAKEN_API_KEY`            | For private endpoints         | Spot API key                         |
| `KRAKEN_API_SECRET`         | For private endpoints         | Spot API secret                      |
| `KRAKEN_FUTURES_API_KEY`    | For futures private endpoints | Futures API key                      |
| `KRAKEN_FUTURES_API_SECRET` | For futures private endpoints | Futures API secret                   |
| `KRAKEN_OTP`                | If the API key has 2FA        | One-time password (2FA code)         |
| `KRAKEN_WORKSPACE`          | No                            | Active workspace (paper or live)     |
| `KRAKEN_SPOT_URL`           | No                            | Override Spot REST API URL           |
| `KRAKEN_FUTURES_URL`        | No                            | Override Futures REST API URL        |
| `KRAKEN_WS_PUBLIC_URL`      | No                            | Override public WebSocket URL        |
| `KRAKEN_WS_AUTH_URL`        | No                            | Override authenticated WebSocket URL |
| `KRAKEN_WS_L3_URL`          | No                            | Override Level 3 WebSocket URL       |
| `KRAKEN_FUTURES_WS_URL`     | No                            | Override Futures WebSocket URL       |
| `KRAKEN_FEEDBACK_ENDPOINT`  | No                            | Override feedback upload URL         |
| `KRAKEN_LOG_FORMAT`         | No                            | stderr log format (default `json`)   |

## Machine-Readable Resources

| File                        | Format   | Description                                                               |
| --------------------------- | -------- | ------------------------------------------------------------------------- |
| `agents/tool-catalog.json`  | JSON     | All 188 commands with generated argument schemas, types, and safety flags |
| `agents/error-catalog.json` | JSON     | 10 error categories with retry guidance                                   |
| `agents/examples/`          | Shell    | Runnable workflow examples                                                |
| `skills/`                   | SKILL.md | Goal-oriented workflow skills for agents                                  |
