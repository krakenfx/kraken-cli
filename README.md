# Kraken CLI

![version](https://img.shields.io/github/v/release/krakenfx/kraken-cli?color=blue)
![license](https://img.shields.io/badge/license-MIT-green)
![platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey)

The first AI-native CLI for crypto trading.

Full Kraken API access. Built-in MCP server. Live and paper trading. Single binary.

Works with Cursor, Claude, Codex, Copilot, Gemini, Goose, and OpenClaw.

Try these with your AI agent:

> *"Install kraken-cli (https://github.com/krakenfx/kraken-cli) and build me a morning market brief."*

> *"Watch ETH, SOL, and BTC for 30 seconds. Tell me which one you'd buy and why."*

> *"You are a Wall Street veteran with 20 years of experience. You have 1 minute. Paper trade BTC and show me your P&L."*

---

**Warning:** Experimental software. Interacts with the live Kraken exchange and can execute real financial transactions. Read [DISCLAIMER.md](DISCLAIMER.md) before using with real funds or AI agents.

## Contents

- [Installation](#installation)
- [Quick Start](#quick-start)
- [MCP Server](#mcp-server)
- [Paper Trading](#paper-trading)
- [Commands](#commands)
- [API Keys & Configuration](#api-keys--configuration)
- [Verifying Binaries](#verifying-binaries)
- [License & Disclaimer](#license--disclaimer)

## Installation

Single binary, no runtime dependencies.

### One-liner

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/krakenfx/kraken-cli/releases/latest/download/kraken-cli-installer.sh | sh
```

Detects your OS and architecture, downloads the right binary, and installs it. macOS (Apple Silicon and Intel) and Linux (x86_64 and ARM64) are supported. Windows: use WSL.

Verify it works:

```bash
kraken status && kraken ticker BTCUSD
```

Pre-built binaries are also available on the [GitHub Releases](https://github.com/krakenfx/kraken-cli/releases) page.

<details>
<summary>Build from source</summary>

Requires [Rust](https://rustup.rs/).

```bash
cargo install --git https://github.com/krakenfx/kraken-cli
```

Or clone and build:

```bash
git clone https://github.com/krakenfx/kraken-cli.git
cd kraken-cli
cargo install --path .
```
</details>

## For AI Agents

If you're an AI agent or building one, start here:

| Resource | Description |
|----------|-------------|
| [CONTEXT.md](CONTEXT.md) | Runtime context for tool-using agents |
| [AGENTS.md](AGENTS.md) | Full integration guide: auth, invocation, errors, rate limits |
| [agents/tool-catalog.json](agents/tool-catalog.json) | 134 commands with parameter schemas, types, and safety flags |
| [agents/error-catalog.json](agents/error-catalog.json) | 9 error categories with retry guidance |
| [skills/](skills/) | 50 goal-oriented SKILL.md workflow packages |
| [CLAUDE.md](CLAUDE.md) | Claude-specific integration guidance |

Core invocation pattern:

```bash
kraken <command> [args...] -o json 2>/dev/null
```

- stdout is always valid JSON on success, or a JSON error envelope on failure.
- Exit code 0 means success. Non-zero means failure.
- stderr carries diagnostics only (enabled with `-v`). Never parse stderr for data.

<details>
<summary>Why Agent-First?</summary>

Most CLIs are built for humans at a terminal. This one is built for LLM-based agents, MCP tool servers, and automated pipelines that need to call Kraken reliably without custom API clients.

- **Structured output by default.** Every command supports `-o json`. No screen-scraping, no regex on table borders.
- **Consistent error envelopes.** Errors are JSON objects with a stable `error` field (`auth`, `rate_limit`, `validation`, `api`, `network`). Agents route on `error` without parsing human sentences.
- **Predictable exit codes.** Success is 0, failure is non-zero. Combined with JSON errors on stdout, agents detect and classify failures programmatically.
- **Paper trading for safe iteration.** Test strategies against live prices with `kraken paper` commands. No API keys, no real money, same interface.
- **Full API surface.** 134 commands covering Spot, Futures, Funding, Earn, Subaccounts, and WebSocket streaming.
- **Built-in MCP server.** Native Model Context Protocol support over stdio. No subprocess wrappers needed.
- **Rate-limit aware.** Built-in Spot counter/decay and Futures token-bucket rate limiting.

Humans benefit from the same design: `--output table` renders clean tables, `kraken shell` provides a REPL, and `kraken setup` walks through configuration.

</details>

## Verifying Binaries

Release binaries are signed with [minisign](https://jedisct1.github.io/minisign/). Every artifact on the [Releases](https://github.com/krakenfx/kraken-cli/releases) page has a corresponding `.minisig` signature file.

**Public key:**

```
RWQJue8SwPzZBZO8Up5ppAUqm0wM/gK5yjnam+Dbf7KbY0utkiO+7XCd
```

**Verify a downloaded binary:**

```bash
minisign -Vm kraken-cli-aarch64-apple-darwin.tar.gz -P RWQJue8SwPzZBZO8Up5ppAUqm0wM/gK5yjnam+Dbf7KbY0utkiO+7XCd
```

Install minisign: `brew install minisign` (macOS) or `apt install minisign` (Linux).

## Quick Start

Public market data requires no credentials:

```bash
kraken ticker BTCUSD -o json
kraken orderbook BTCUSD --count 10 -o json
kraken trades BTCUSD --count 20 -o json
kraken ohlc BTCUSD --interval 60 -o json
```

With authentication:

```bash
export KRAKEN_API_KEY="your-key"
export KRAKEN_API_SECRET="your-secret"

kraken balance -o json
kraken open-orders -o json
kraken order buy BTCUSD 0.001 --type limit --price 50000 -o json
```

For humans (table output, interactive setup):

```bash
kraken setup
kraken ticker BTCUSD
kraken balance
kraken shell
```

## API Keys & Configuration

Authenticated commands (trading, balances, order management) require a Kraken API key pair. Public market data and paper trading work without credentials.

### Getting API keys

**Spot** (covers trading, balances, funding, earn, subaccounts): [Settings > API](https://www.kraken.com) on kraken.com. Full guide: [How to create a spot API key](https://support.kraken.com/articles/360000919966-how-to-create-an-api-key).

**Futures** (only for `kraken futures` commands): [Settings > Create Key](https://www.kraken.com/login-futures) on the Futures interface. Full guide: [How to create a Futures API key](https://support.kraken.com/hc/en-us/articles/360022839451-How-to-create-an-API-key-for-Kraken-Futures).

Grant the minimum permissions your workflow needs. For read-only monitoring, "Query Funds" and "Query Open Orders & Trades" are sufficient.

### Environment variables (recommended for agents)

```bash
export KRAKEN_API_KEY="your-key"
export KRAKEN_API_SECRET="your-secret"
export KRAKEN_FUTURES_API_KEY="your-futures-key"       # optional
export KRAKEN_FUTURES_API_SECRET="your-futures-secret"  # optional
```

### Config file (for humans)

Store credentials in `~/.config/kraken/config.toml`:

```toml
[auth]
api_key = "your-api-key"
api_secret = "your-api-secret"

[settings]
default_pair = "BTCUSD"
output = "table"
rate_tier = "starter"  # starter, intermediate, or pro
```

Or use the interactive wizard: `kraken setup`.

### Credential resolution

Highest precedence first:

1. Command-line flags (`--api-key`, `--api-secret`)
2. Environment variables (`KRAKEN_API_KEY`, `KRAKEN_API_SECRET`)
3. Config file (`~/.config/kraken/config.toml`)

### Security

- Config file is created with `0600` permissions (owner read/write only)
- Secrets are never logged, printed, or included in error messages
- Use `--api-secret-stdin` or `--api-secret-file` instead of `--api-secret` to avoid secrets in process listings
- For automation, prefer environment variables over command-line flags

<details>
<summary>Advanced: API endpoint overrides</summary>

| Variable | Default | Description |
|----------|---------|-------------|
| `KRAKEN_SPOT_URL` | `https://api.kraken.com` | Spot REST API base URL |
| `KRAKEN_FUTURES_URL` | `https://futures.kraken.com/derivatives/api/v3` | Futures REST API base URL |
| `KRAKEN_WS_PUBLIC_URL` | `wss://ws.kraken.com/v2` | Public WebSocket endpoint |
| `KRAKEN_WS_AUTH_URL` | `wss://ws-auth.kraken.com/v2` | Authenticated Spot WebSocket endpoint |
| `KRAKEN_WS_L3_URL` | `wss://ws-l3.kraken.com/v2` | Level 3 order book WebSocket endpoint |
| `KRAKEN_FUTURES_WS_URL` | `wss://futures.kraken.com/ws/v1` | Futures WebSocket endpoint |

Resolution: CLI flag > environment variable > default. Only `https://` and `wss://` schemes are accepted.

</details>

## MCP Server

`kraken-cli` includes a built-in [Model Context Protocol](https://modelcontextprotocol.io/) server over stdio. No subprocess wrappers needed.

Security note:
- MCP is local-first and designed for your own machine.
- Any agent connected to this MCP server uses the same configured Kraken account and API key permissions.
- Do not expose, tunnel, or share this MCP server outside systems you control.
- Always use `https://` and `wss://` endpoints.
- Treat this integration as alpha and use least-privilege API keys.

```bash
kraken mcp                           # read-only (market, account, paper)
kraken mcp -s all                    # all services, dangerous calls require acknowledged=true
kraken mcp -s all --allow-dangerous  # all services, no per-call confirmation
kraken mcp -s market,trade,paper     # specific services
```

Configure your MCP client (Claude Desktop, ChatGPT, Codex, Gemini CLI, Cursor, VS Code, Windsurf, etc.):

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

Gemini CLI users can install directly: `gemini extensions install https://github.com/krakenfx/kraken-cli`

### Service groups

| Service | Auth | Risk |
|---------|------|------|
| market | No | None |
| account | Yes | Read-only |
| trade | Yes | Orders (dangerous) |
| funding | Yes | Withdrawals (dangerous) |
| earn | Yes | Staking (dangerous) |
| subaccount | Yes | Transfers (dangerous) |
| futures | Mixed | Orders (dangerous) |
| paper | No | None (simulation) |
| auth | No | Read-only |

Default: `market,account,paper`. Streaming groups (`websocket`, `futures-ws`) are excluded in MCP v1.

Dangerous tools carry the `destructive_hint` annotation and include `[DANGEROUS: requires human confirmation]` in the description. In guarded mode (default), dangerous calls must include `acknowledged=true`. In autonomous mode (`--allow-dangerous`), this requirement is disabled.

<details>
<summary>Output & Errors</summary>

### JSON (`-o json`)

The primary output format. Every command returns valid JSON on stdout.

```bash
kraken ticker BTCUSD -o json
# {"BTCUSD":{"a":["67234.10","1","1.000"],"b":["67234.00","1","1.000"],...}}
```

WebSocket commands emit NDJSON (one JSON object per line) for streaming.

### Table (`-o table`)

Human-readable tables. Default when `-o` is not specified.

```
┌──────────┬──────────┬──────────┬──────────┬────────────┐
│ Pair     │ Ask      │ Bid      │ Last     │ Volume ... │
├──────────┼──────────┼──────────┼──────────┼────────────┤
│ BTCUSD   │ 67234.10 │ 67234.00 │ 67234.10 │ 1234.56    │
└──────────┴──────────┴──────────┴──────────┴────────────┘
```

### Error envelopes

On failure, stdout contains a JSON envelope with a stable `error` category:

```json
{"error": "auth", "message": "Authentication failed: EAPI:Invalid key"}
```

Route on `error`, not on `message`. The `message` field is human-readable and not stable.

| Category | Retryable | Action |
|----------|-----------|--------|
| `auth` | No | Check API key and secret |
| `rate_limit` | Yes | Wait 5-15s, reduce frequency |
| `network` | Yes | Exponential backoff, max 5 retries |
| `validation` | No | Fix input parameters |
| `api` | No | Inspect request and parameters |
| `config` | No | Check config file or env vars |
| `websocket` | Yes | Reconnect with backoff (CLI retries 3x) |
| `io` | No | Check file paths and permissions |
| `parse` | No | Log raw response, possible API maintenance |

Full error catalog with envelopes and backoff strategies: [agents/error-catalog.json](agents/error-catalog.json).

### Verbose (`-v`)

Request/response diagnostics go to stderr, keeping stdout clean:

```bash
kraken balance -o json -v 2>/dev/null | jq .
```

</details>

## Commands

134 commands across 12 groups. For machine-readable parameter schemas, load [agents/tool-catalog.json](agents/tool-catalog.json).

| Group | Commands | Auth | Description |
|-------|----------|------|-------------|
| market | 10 | No | Ticker, orderbook, OHLC, trades, spreads, asset info |
| account | 18 | Yes | Balances, orders, trades, ledgers, positions, exports |
| trade | 9 | Yes | Order placement, amendment, cancellation |
| funding | 10 | Yes | Deposits, withdrawals, wallet transfers |
| earn | 6 | Yes | Staking strategies and allocations |
| subaccount | 2 | Yes | Create subaccounts, transfer between accounts |
| futures | 39 | Mixed | Futures market data and trading |
| futures-ws | 9 | Mixed | Futures WebSocket streaming |
| websocket | 15 | Mixed | Spot WebSocket v2 streaming and request/response |
| paper | 10 | No | Paper trading simulation with live prices |
| auth | 4 | No | Credential management |
| utility | 2 | No | Interactive setup and REPL shell |

32 commands are marked `dangerous` (orders, withdrawals, transfers, cancel-all, staking). The authoritative list is the `dangerous` field in [agents/tool-catalog.json](agents/tool-catalog.json).

<details>
<summary>Full command reference</summary>

### Market Data (Public)

| Command | Description |
|---------|-------------|
| `kraken status` | System status |
| `kraken server-time` | Server time |
| `kraken assets [--asset BTC,ETH] [--aclass CLASS]` | Asset info |
| `kraken pairs [--pair BTCUSD] [--aclass CLASS]` | Tradable pairs |
| `kraken ticker <PAIR...> [--asset-class tokenized_asset\|forex]` | Ticker data |
| `kraken ohlc <PAIR> [--interval 60] [--asset-class tokenized_asset\|forex]` | OHLC candles |
| `kraken orderbook <PAIR> [--count 25] [--asset-class tokenized_asset]` | L2 order book |
| `kraken orderbook-grouped <PAIR> [--depth 10] [--grouping 1]` | Grouped order book |
| `kraken trades <PAIR> [--count 1000] [--asset-class tokenized_asset]` | Recent trades |
| `kraken spreads <PAIR> [--since TS] [--asset-class tokenized_asset]` | Recent spreads |

### Account Data (Private)

| Command | Description |
|---------|-------------|
| `kraken orderbook-l3 <PAIR> [--depth 100]` | L3 order book |
| `kraken balance [--rebase-multiplier rebased\|base]` | All balances |
| `kraken extended-balance` | Extended balances (balance, credit, held) |
| `kraken credit-lines [--rebase-multiplier rebased\|base]` | Credit line details (VIP) |
| `kraken trade-balance [--asset USD] [--rebase-multiplier rebased\|base]` | Margin/equity summary |
| `kraken open-orders [--trades] [--cl-ord-id ID] [--rebase-multiplier rebased\|base]` | Open orders |
| `kraken closed-orders [--start TS] [--end TS] [--offset N] [--userref REF] [--cl-ord-id ID] [--closetime open\|close\|both] [--consolidate-taker] [--without-count] [--rebase-multiplier rebased\|base]` | Closed orders |
| `kraken query-orders <TXID...> [--trades] [--userref REF] [--consolidate-taker] [--rebase-multiplier rebased\|base]` | Query specific orders |
| `kraken trades-history [--type TYPE] [--trades] [--offset N] [--consolidate-taker] [--without-count] [--ledgers] [--rebase-multiplier rebased\|base]` | Trade history |
| `kraken query-trades <TXID...> [--trades] [--rebase-multiplier rebased\|base]` | Query specific trades |
| `kraken positions [--txid ID] [--show-pnl] [--consolidation market] [--rebase-multiplier rebased\|base]` | Open positions |
| `kraken ledgers [--asset BTC --type trade] [--aclass CLASS] [--start TS] [--end TS] [--offset N] [--without-count] [--rebase-multiplier rebased\|base]` | Ledger entries |
| `kraken query-ledgers <ID...> [--trades] [--rebase-multiplier rebased\|base]` | Query specific ledger entries |
| `kraken volume [--pair BTCUSD] [--rebase-multiplier rebased\|base]` | Trade volume & fees |
| `kraken export-report --report trades --description "desc" [--format CSV\|TSV] [--fields F] [--starttm TS] [--endtm TS]` | Request export |
| `kraken export-status --report trades` | Export status |
| `kraken export-retrieve <ID>` | Download export |
| `kraken export-delete <ID>` | Delete export |

### Trading

| Command | Description |
|---------|-------------|
| `kraken order buy <PAIR> <VOL> [--type limit --price P] [--asset-class tokenized_asset] [--validate] ...` | Buy order |
| `kraken order sell <PAIR> <VOL> [--type limit --price P] [--asset-class tokenized_asset] [--validate] ...` | Sell order |
| `kraken order batch <JSON_FILE> [--pair P] [--asset-class tokenized_asset] [--deadline DL] [--validate]` | Batch orders (2-15) |
| `kraken order amend --txid <TXID> [--order-qty Q --limit-price P ...]` | Amend order in-place |
| `kraken order amend --cl-ord-id <ID> [--limit-price P ...]` | Amend by client order ID |
| `kraken order edit <TXID> [--price P]` | Edit order (cancel+replace) |
| `kraken order cancel <TXID...>` | Cancel by txid(s) |
| `kraken order cancel --cl-ord-id <ID>` | Cancel by client order ID |
| `kraken order cancel-batch <TXID...> [--cl-ord-ids ID...]` | Cancel batch (max 50 total) |
| `kraken order cancel-all [--yes]` | Cancel all |
| `kraken order cancel-after <SECS>` | Dead man's switch |

### Funding

| Command | Description |
|---------|-------------|
| `kraken deposit methods <ASSET> [--aclass currency\|tokenized_asset] [--rebase-multiplier rebased\|base]` | Deposit methods |
| `kraken deposit addresses <ASSET> <METHOD> [--new] [--aclass CLASS] [--amount AMT]` | Deposit addresses |
| `kraken deposit status [--asset A] [--aclass CLASS] [--method M] [--start TS] [--end TS] [--cursor C] [--limit N] [--rebase-multiplier rebased\|base]` | Deposit status |
| `kraken withdraw <ASSET> <KEY> <AMOUNT> [--aclass CLASS] [--address ADDR] [--max-fee F] [--rebase-multiplier rebased\|base]` | Withdraw funds |
| `kraken withdrawal methods [--asset A] [--aclass CLASS] [--network N] [--rebase-multiplier rebased\|base]` | Withdrawal methods |
| `kraken withdrawal addresses [--asset A] [--aclass CLASS] [--method M] [--key K] [--verified true\|false]` | Withdrawal addresses |
| `kraken withdrawal info <ASSET> <KEY> <AMOUNT>` | Withdrawal fee info |
| `kraken withdrawal status [--asset A] [--aclass CLASS] [--method M] [--start TS] [--end TS] [--cursor C] [--limit N] [--rebase-multiplier rebased\|base]` | Withdrawal status |
| `kraken withdrawal cancel <ASSET> <REFID>` | Cancel pending withdrawal |
| `kraken wallet-transfer <ASSET> <AMT> --from IIBAN --to IIBAN` | Wallet transfer |

### Earn

| Command | Description |
|---------|-------------|
| `kraken earn strategies [--asset ETH] [--lock-type flex bonded...] [--ascending] [--cursor C] [--limit N]` | List strategies |
| `kraken earn allocate <ID> <AMT>` | Allocate funds |
| `kraken earn deallocate <ID> <AMT>` | Deallocate funds |
| `kraken earn allocate-status <ID>` | Allocation status |
| `kraken earn deallocate-status <ID>` | Deallocation status |
| `kraken earn allocations [--ascending] [--converted-asset USD] [--hide-zero-allocations]` | Current allocations |

### Subaccounts

| Command | Description |
|---------|-------------|
| `kraken subaccount create <USERNAME> <EMAIL>` | Create sub account |
| `kraken subaccount transfer <ASSET> <AMOUNT> --from IIBAN --to IIBAN [--asset-class CLASS]` | Transfer between accounts |

### Futures: Public Market Data

| Command | Description |
|---------|-------------|
| `kraken futures instruments` | All contracts |
| `kraken futures tickers` | All tickers |
| `kraken futures ticker <SYMBOL>` | Single ticker |
| `kraken futures orderbook <SYMBOL>` | Order book |
| `kraken futures history <SYMBOL> [--since TS] [--before TS]` | Trade history |
| `kraken futures feeschedules` | Fee schedules |
| `kraken futures instrument-status [--symbol SYM]` | Instrument status |

### Futures: Trading (Private)

| Command | Description |
|---------|-------------|
| `kraken futures order buy <SYM> <SIZE> [--type limit] [--price P] [--stop-price SP] [--trigger-signal mark\|index\|last] [--client-order-id ID] [--reduce-only] [--trailing-stop-max-deviation V] [--trailing-stop-deviation-unit percent\|quote_currency]` | Buy futures |
| `kraken futures order sell <SYM> <SIZE> [--type limit] [--price P] [--stop-price SP] ...` | Sell futures |
| `kraken futures edit-order --order-id ID [--size S] [--price P] [--stop-price SP]` | Edit existing order |
| `kraken futures cancel --order-id ID` | Cancel by order ID |
| `kraken futures cancel --cli-ord-id ID` | Cancel by client order ID |
| `kraken futures cancel-all [--symbol SYM] [--yes]` | Cancel all orders |
| `kraken futures cancel-after <SECS>` | Dead man's switch |
| `kraken futures batch-order '<JSON>'` or `kraken futures batch-order @file.json` | Batch order placement |

### Futures: Account Data (Private)

| Command | Description |
|---------|-------------|
| `kraken futures accounts` | Wallet/account info |
| `kraken futures open-orders` | Open orders |
| `kraken futures positions` | Open positions |
| `kraken futures fills [--since TS]` | Recent fills |
| `kraken futures leverage [--symbol SYM]` | Leverage preferences |
| `kraken futures set-leverage <SYMBOL> <MAX_LEVERAGE>` | Set leverage |
| `kraken futures notifications` | Account notifications |
| `kraken futures transfers` | Transfer history |
| `kraken futures transfer <AMOUNT> <CURRENCY>` | Transfer between wallets |
| `kraken futures history-executions [--since TS] [--before TS] [--sort asc\|desc]` | Execution history |
| `kraken futures history-orders [--since TS] [--before TS] [--sort asc\|desc]` | Order history |
| `kraken futures history-triggers [--since TS] [--before TS] [--sort asc\|desc]` | Trigger history |
| `kraken futures history-account-log-csv [--since TS] [--before TS]` | Account log (CSV) |
| `kraken futures trading-instruments [--contract-type ...]` | Trading instruments (auth) |
| `kraken futures historical-funding-rates <SYMBOL>` | Historical funding rates |
| `kraken futures order-status <ORDER_ID>...` | Query order status (auth) |
| `kraken futures pnl-preferences` | Get PnL preferences (auth) |
| `kraken futures set-pnl-preference <SYMBOL> <PREFERENCE>` | Set PnL preference (auth) |
| `kraken futures unwind-queue` | Get unwind queue (auth) |
| `kraken futures assignment-programs` | Assignment programs (auth) |
| `kraken futures fee-schedule-volumes` | Fee schedule volumes (auth) |
| `kraken futures subaccounts` | List subaccounts (auth) |
| `kraken futures subaccount-status <UID>` | Subaccount trading status (auth) |
| `kraken futures set-subaccount-status <UID> <ENABLED>` | Set subaccount trading status (auth) |
| `kraken futures wallet-transfer <FROM> <TO> <UNIT> <AMOUNT>` | Transfer between wallets (auth) |

### Futures WebSocket Streaming

| Command | Description |
|---------|-------------|
| `kraken futures ws ticker <PRODUCT_ID...>` | Stream futures tickers (public) |
| `kraken futures ws trade <PRODUCT_ID...>` | Stream futures trades (public) |
| `kraken futures ws book <PRODUCT_ID...>` | Stream futures order book (public) |
| `kraken futures ws fills` | Stream fills (private, challenge auth) |
| `kraken futures ws open-orders` | Stream open orders (private) |
| `kraken futures ws open-positions` | Stream open positions (private) |
| `kraken futures ws balances` | Stream balances and margins (private) |
| `kraken futures ws notifications` | Stream notifications (private) |
| `kraken futures ws account-log` | Stream account log (private) |

### WebSocket (Streaming and Request/Response)

| Command | Description |
|---------|-------------|
| `kraken ws ticker <PAIR...> [--event-trigger bbo]` | Stream tickers |
| `kraken ws trades <PAIR...>` | Stream trades |
| `kraken ws book <PAIR...> [--depth 10]` | Stream L2 order book |
| `kraken ws ohlc <PAIR...> [--interval 1]` | Stream OHLC candles |
| `kraken ws instrument [PAIR...]` | Stream instrument metadata |
| `kraken ws executions` | Stream trade executions (auth) |
| `kraken ws balances` | Stream balance updates (auth) |
| `kraken ws level3 <PAIR...>` | Stream L3 order book (auth) |
| `kraken ws add-order` | Place order via WebSocket (auth) |
| `kraken ws amend-order` | Amend order via WebSocket (auth) |
| `kraken ws cancel-order` | Cancel order(s) via WebSocket (auth) |
| `kraken ws cancel-all` | Cancel all orders via WebSocket (auth) |
| `kraken ws cancel-after <TIMEOUT>` | Dead man's switch via WebSocket (auth) |
| `kraken ws batch-add` | Batch add orders via WebSocket (auth) |
| `kraken ws batch-cancel` | Batch cancel orders via WebSocket (auth) |

### Utility

| Command | Description |
|---------|-------------|
| `kraken setup` | Interactive setup wizard |
| `kraken shell` | Interactive REPL |
| `kraken auth set/show/test/reset` | Credential management |

</details>

## Paper Trading

Paper trading provides a safe sandbox for testing trading logic against live Kraken prices. No API keys, no account, no real money. The command interface mirrors live trading exactly; agents can switch between paper and live by changing the command prefix (`kraken paper buy` vs `kraken order buy`).

**Agent pattern:**

```bash
kraken paper init --balance 10000 -o json
kraken paper buy BTCUSD 0.01 -o json
kraken paper sell BTCUSD 0.005 --type limit --price 70000 -o json
kraken paper status -o json
kraken paper reset -o json
```

**Human pattern:**

```bash
kraken paper init --balance 5000 --currency EUR
kraken paper buy BTCUSD 0.01
kraken paper balance
kraken paper status
kraken paper orders
kraken paper history
kraken paper reset
```

| Command | Description |
|---------|-------------|
| `kraken paper init [--balance 10000] [--currency USD]` | Initialize paper account |
| `kraken paper reset` | Reconcile pending orders, then reset |
| `kraken paper balance` | All balances (total, reserved, available) |
| `kraken paper buy <PAIR> <VOL> [--type limit --price P]` | Paper buy (market or limit) |
| `kraken paper sell <PAIR> <VOL> [--type limit --price P]` | Paper sell (market or limit) |
| `kraken paper orders` | Open limit orders |
| `kraken paper cancel <ORDER_ID>` | Cancel a limit order |
| `kraken paper cancel-all` | Cancel all limit orders |
| `kraken paper history` | Filled trade history |
| `kraken paper status` | Portfolio value, unrealized P&L, trade count |

Prices come from the public Kraken Ticker API (no auth needed). A 0.26% taker fee (Kraken Starter tier default) is applied to all fills. Limit orders fill at the limit price when the live market crosses the order price.

All output is labeled `[PAPER]` in table mode and includes `"mode": "paper"` in JSON mode.

## Examples

### Conditional order based on live price

```bash
PRICE=$(kraken ticker BTCUSD -o json | jq -r '.[].c[0]')
kraken order buy BTCUSD 0.001 --type limit --price "$PRICE" -o json
```

### Portfolio rebalance check

```bash
BTC=$(kraken balance -o json | jq -r '.BTC // "0"')

if [ "$(echo "$BTC > 0.001" | bc)" -eq 1 ]; then
  kraken order sell BTCUSD 0.001 --type market -o json
fi
```

### Real-time price stream

```bash
kraken ws ticker BTC/USD -o json | while read -r line; do
  LAST=$(echo "$line" | jq -r '.data[0].last // empty')
  [ -n "$LAST" ] && echo "BTC: $LAST"
done
```

### Stream multiple tickers

```bash
kraken ws ticker BTC/USD ETH/USD SOL/USD -o json
```

### Dead man's switch

```bash
kraken order cancel-after 60
```

Cancels all open orders if the CLI does not reset the timer within 60 seconds.

## Agent Skills

Ships with 50 agent skills covering trading strategies, market analysis, and portfolio management. See the full [Skills Index](skills/INDEX.md).

<details>
<summary>Troubleshooting</summary>

**macOS blocks the binary ("Apple could not verify")**

```bash
xattr -d com.apple.quarantine /usr/local/bin/kraken
```

**"EAPI:Invalid key" or authentication errors**

Check that `KRAKEN_API_KEY` and `KRAKEN_API_SECRET` are set correctly. Keys are case-sensitive. Verify the API key has the required permissions for the command (Query Funds for `balance`, Modify Orders for `order buy`, etc.).

**Rate limit errors**

Kraken uses counter-based rate limiting for Spot (15-20 counter with 0.33-1.0/s decay depending on tier) and token-bucket for Futures. Wait 5-15 seconds and retry. For real-time data, use WebSocket streaming instead of polling.

**"No tools found" in MCP client**

Verify `kraken` is on your PATH. Run `kraken mcp -s market` in a terminal to confirm the server starts. Check that your client configuration matches the JSON format in the [MCP Server](#mcp-server) section.

**Paper trading state errors**

Run `kraken paper reset` to clear and reinitialize.

</details>

<details>
<summary>Architecture</summary>

```
src/
  main.rs          -- CLI entry point, clap parsing, exit codes
  lib.rs           -- AppContext, shared dispatcher, module re-exports
  auth.rs          -- HMAC-SHA512 signing (Spot + Futures), nonce management
  config.rs        -- Config file I/O, credential resolution, secret wrappers
  client.rs        -- HTTP clients with rate limiting and retry
  errors.rs        -- Unified error types with category-based JSON envelopes
  paper.rs         -- Paper trading state engine (local simulation)
  shell.rs         -- Interactive REPL with rustyline
  telemetry.rs     -- Agent client identification, instance tracking, user-agent
  commands/        -- One module per command group
    mod.rs         -- Module re-exports
    helpers.rs     -- Shared utilities (JSON extraction, output formatting)
    market.rs      -- Public market data
    account.rs     -- Private account data
    trade.rs       -- Spot trading
    funding.rs     -- Deposits, withdrawals, wallet transfers
    earn.rs        -- Staking/earn
    subaccount.rs  -- Subaccount management
    futures.rs     -- Futures trading and market data
    futures_ws.rs  -- Futures WebSocket v1 streaming
    websocket.rs   -- Spot WebSocket v2 streaming
    paper.rs       -- Paper trading commands
    auth.rs        -- Credential management commands
    utility.rs     -- Setup wizard
  mcp/             -- Built-in MCP server
    mod.rs         -- Service group parsing, streaming exclusion filters
    server.rs      -- ServerHandler impl, tool dispatch, env-based URL resolution
    registry.rs    -- Tool registry built from clap tree + tool-catalog.json
    schema.rs      -- JSON Schema generation from clap argument metadata
  output/          -- Rendering layer
    mod.rs         -- Format dispatch, CommandOutput type
    table.rs       -- Human-readable table output
    json.rs        -- JSON output and NDJSON streaming
```

</details>

## Development

```bash
cargo build                    # dev build
cargo test                     # run tests
cargo clippy -- -D warnings    # lint
```

## License & Disclaimer

MIT. See [LICENSE](LICENSE).

This is experimental software. Commands interact with the live Kraken exchange and can result in real financial transactions. Orders, withdrawals, and transfers are irreversible once processed. The authors accept no liability for financial losses. See [DISCLAIMER.md](DISCLAIMER.md) for full terms.
