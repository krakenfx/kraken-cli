# Kraken CLI

[![version](https://img.shields.io/github/v/release/krakenfx/kraken-cli?color=blue)](https://github.com/krakenfx/kraken-cli/releases)
[![license](https://img.shields.io/badge/license-MIT-green)](LICENSE)
[![platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey)](#installation)

The first AI-native CLI for trading crypto, stocks, forex, and derivatives.

Full Kraken API access. Built-in MCP server. Live and paper trading. One
install.

Works with Cursor, Claude, Codex, Copilot, Gemini, Goose, and OpenClaw.

Paste these prompts into Claude Code, Codex, Cursor, or another AI coding agent.
No agent yet? Install [Claude Code](https://claude.com/claude-code) first.

> _"Install kraken-cli (<https://github.com/krakenfx/kraken-cli>) and build me a
> morning market brief."_

> _"Watch ETH, SOL, and BTC for 30 seconds. What are the pros and cons of buying
> each right now?"_

> _"Look up AAPLx, TSLAx, and SPYx on xStocks. Summarize how each is performing
> today."_

> _"You are a Wall Street veteran with 20 years of experience. You have 1
> minute. Paper trade BTC and show me your P&L."_

> _"Open a 10x leveraged long on BTC futures paper with $5,000 collateral. Set a
> trailing stop at 3%."_

---

> [!CAUTION] Experimental software. Interacts with the live Kraken exchange and
> can execute real financial transactions. Read [DISCLAIMER.md](DISCLAIMER.md)
> before using with real funds or AI agents.

## Contents

- [Installation](#installation)
- [What You Can Trade](#what-you-can-trade)
- [For AI Agents](#for-ai-agents)
- [Verifying Binaries](#verifying-binaries)
- [Quick Start](#quick-start)
- [API Keys & Configuration](#api-keys--configuration)
- [MCP Server](#mcp-server)
- [Workspaces, Sessions, and Research](#workspaces-sessions-and-research)
- [Commands](#commands)
- [Paper Trading](#paper-trading)
- [Examples](#examples)
- [Agent Skills](#agent-skills)
- [Development](#development)
- [Contributing](#contributing)
- [License & Disclaimer](#license--disclaimer)

## Installation

One install, no runtime dependencies. It puts three binaries on your PATH:
`kraken`, `kraken-mcp` (the MCP server) and `kraken-shell` (an interactive
shell).

### One-liner

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/krakenfx/kraken-cli/releases/latest/download/kraken-cli-installer.sh | sh
```

Detects your OS and architecture, downloads the right build, and installs it.
macOS (Apple Silicon and Intel) and Linux (x86_64 and ARM64) are supported.
Windows: use WSL.

Verify it works:

```bash
kraken status && kraken ticker BTCUSD
```

> [!NOTE] `command not found: kraken` right after install? The installer puts
> the binaries in `~/.cargo/bin` (or `$CARGO_HOME/bin` if you set it) and adds
> that folder to your PATH for new terminals only.
>
> - Terminal: open a new window, or run `source "$HOME/.cargo/env"`
>   (`$CARGO_HOME/env` if you set it, `env.fish` for fish).
> - Claude Code, Codex, or another agent you start in a terminal: quit it, open
>   a new terminal window, and start it there. The old window keeps the old
>   PATH.
> - Cursor, VS Code, or another desktop app: quit it and open it again.
> - Until then, use the full path `~/.cargo/bin/kraken` (or
>   `$CARGO_HOME/bin/kraken`).

Pre-built binaries are also available on the
[GitHub Releases](https://github.com/krakenfx/kraken-cli/releases) page.

<details>
<summary>Build from source</summary>

Requires [Rust](https://rustup.rs/) 1.97 or later. If cargo says a newer rustc
is required, run `rustup update`.

A source build is DuckDB-free by default, because the bundled DuckDB C++ build
dominates compile time and only `kraken record --to duckdb` needs it. Pass
`--features record-duckdb` to any command below to include it; the released
binaries already carry it.

Both commands install all three binaries: `kraken`, `kraken-mcp` and
`kraken-shell`.

```bash
cargo install --locked --git https://github.com/krakenfx/kraken-cli kraken-cli
```

Or clone and build:

```bash
git clone https://github.com/krakenfx/kraken-cli.git
cd kraken-cli
cargo install --locked --path crates/kraken-cli
```

If an earlier source install added `kraken-mcp` or `kraken-shell`, cargo stops
with "binary already exists". Run `cargo uninstall kraken-mcp kraken-shell`
first, or add `--force`.

</details>

## What You Can Trade

One CLI covers six asset classes. All trading commands work across asset classes
using the same interface; pass `--asset-class` where needed.

| Asset class                                                                  | Instruments                                                                                       | Margin                   | Flag                            | Example                                                                                 |
| ---------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- | ------------------------ | ------------------------------- | --------------------------------------------------------------------------------------- |
| **Crypto spot**                                                              | 1,400+ pairs (BTC, ETH, SOL, and hundreds more)                                                   | Up to 10x on major pairs | _(default)_                     | `kraken order buy BTCUSD 0.001 --type limit --price 50000`                              |
| **Tokenized U.S. stocks & ETFs ([xStocks](https://www.kraken.com/xstocks))** | 79 assets: AAPL, NVDA, TSLA, GOOGL, AMZN, MSFT, SPY, QQQ, and more                                | Up to 3x on top 10       | `--asset-class tokenized_asset` | `kraken order buy AAPLx/USD 0.1 --type limit --price 200 --asset-class tokenized_asset` |
| **Forex**                                                                    | 11 fiat pairs: EUR/USD, GBP/USD, USD/JPY, AUD/USD, and more                                       | —                        | `--asset-class forex`           | `kraken ticker EURUSD --asset-class forex`                                              |
| **Perpetual futures**                                                        | 317 contracts: crypto, 5 forex perps, 11 equity/index perps (AAPL, NVDA, TSLA, SPY, QQQ, S&P 500) | Up to 50x                | _(futures engine)_              | `kraken futures order buy PF_XBTUSD 1 --type limit --price 50000`                       |
| **Inverse & fixed-date futures**                                             | 20 contracts: BTC, ETH, SOL, LTC, XRP, ADA, DOGE, LINK                                            | Varies                   | _(futures engine)_              | `kraken futures order buy FI_XBTUSD_<YYMMDD> 1 --type limit --price 50000`              |
| **Earn / staking**                                                           | Flexible and bonded strategies across multiple assets                                             | —                        | —                               | `kraken earn strategies --asset ETH`                                                    |

Fixed-date symbols end in their expiry date (`YYMMDD`).
`kraken futures instruments` lists the contracts that are trading now.

_xStocks are not available in the USA. Availability for all products varies by
jurisdiction._

## For AI Agents

If you're an AI agent or building one, start here:

| Resource                                               | Description                                                           |
| ------------------------------------------------------ | --------------------------------------------------------------------- |
| [CONTEXT.md](CONTEXT.md)                               | Runtime context for tool-using agents                                 |
| [AGENTS.md](AGENTS.md)                                 | Full integration guide: auth, invocation, errors, rate limits         |
| [agents/tool-catalog.json](agents/tool-catalog.json)   | 188 commands with generated argument schemas, types, and safety flags |
| [agents/error-catalog.json](agents/error-catalog.json) | 10 error categories with retry guidance                               |
| [skills/](skills/)                                     | Goal-oriented SKILL.md workflow packages                              |
| [CLAUDE.md](CLAUDE.md)                                 | Claude-specific integration guidance                                  |

Core invocation pattern:

```bash
kraken <command> [args...] -o json 2>/dev/null
```

- stdout is always valid JSON on success, or a JSON error envelope on failure.
- Exit code 0 means success. Non-zero means failure.
- stderr carries diagnostics only — structured JSON logs (warnings/errors by
  default, more with `-v` or `RUST_LOG`). Never parse stderr for data.

<details>
<summary>Why Agent-First?</summary>

Most CLIs are built for humans at a terminal. This one is built for LLM-based
agents, MCP tool servers, and automated pipelines that need to call Kraken
reliably without custom API clients.

- **Structured output by default.** Every command supports `-o json`. No
  screen-scraping, no regex on table borders.
- **Consistent error envelopes.** Errors are JSON objects with a stable `error`
  field (`auth`, `rate_limit`, `validation`, `api`, `network`). Agents route on
  `error` without parsing human sentences.
- **Predictable exit codes.** Success is 0, failure is non-zero. Combined with
  JSON errors on stdout, agents detect and classify failures programmatically.
- **Paper trading for safe iteration.** Test strategies against live prices with
  `kraken paper` (spot) and `kraken futures paper` (perpetual futures) commands.
  No API keys, no real money, same interface as live trading.
- **Market memory.** `kraken record` captures live streams into a local
  DuckDB/JSONL tape library with integrity counters; finalized tapes replay into
  paper sessions and feed the autoresearch lab.
- **Full API surface.** The catalog lists all 188 commands, covering Spot,
  Futures, xStocks, Forex, Funding, Earn, Subaccounts, and paper trading.
  `kraken-mcp` serves 152 MCP tools from it. Streams (WebSocket, recording,
  replay, session start, the stream daemon), interactive commands, and the local
  credential commands `auth switch` and `auth reset` stay CLI-only; each catalog
  entry's `reach` field says which.
- **Built-in MCP server.** Native Model Context Protocol support over stdio. No
  subprocess wrappers needed.
- **Rate-limit aware.** No client-side throttling. When the Kraken API rejects a
  request due to rate limits, the CLI returns an enriched error with
  `suggestion`, `retryable`, and `docs_url` fields so agents can read the
  documentation and adapt their strategy.

Humans benefit from the same design: `--output table` renders clean tables,
`kraken-shell` provides a REPL, and `kraken setup` walks through configuration.

</details>

## Verifying Binaries

Release binaries are signed with
[minisign](https://jedisct1.github.io/minisign/). Every artifact on the
[Releases](https://github.com/krakenfx/kraken-cli/releases) page has a
corresponding `.minisig` signature file.

**Public key:**

```text
RWQJue8SwPzZBZO8Up5ppAUqm0wM/gK5yjnam+Dbf7KbY0utkiO+7XCd
```

**Verify a downloaded binary:**

```bash
minisign -Vm kraken-cli-aarch64-apple-darwin.tar.gz -P RWQJue8SwPzZBZO8Up5ppAUqm0wM/gK5yjnam+Dbf7KbY0utkiO+7XCd
```

Install minisign: `brew install minisign` (macOS) or `apt install minisign`
(Linux).

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
kraken auth login   # sign in in the browser; saves a restricted, expiring API key
kraken setup
kraken ticker BTCUSD
kraken balance
kraken-shell
```

`kraken auth login` needs a terminal. It opens Kraken in your browser and saves
a restricted Spot API key with no withdrawal rights and an expiry date (90 days
unless you pass `--valid-for`). `kraken setup` stores a key you created yourself
instead.

## API Keys & Configuration

Authenticated commands (trading, balances, order management) require a Kraken
API key pair. Public market data and paper trading work without credentials.

### Getting API keys

**Spot** (covers trading, balances, funding, earn, subaccounts): **Settings >
API** on kraken.com. Step-by-step guide:
[How to create a spot API key](https://support.kraken.com/articles/360000919966-how-to-create-an-api-key).

**Futures** (only for `kraken futures` commands):
[Settings > Create Key](https://www.kraken.com/login-futures) on the Futures
interface. Full guide:
[How to create a Futures API key](https://support.kraken.com/hc/en-us/articles/360022839451-How-to-create-an-API-key-for-Kraken-Futures).

Grant the minimum permissions your workflow needs. For read-only monitoring,
"Query Funds" and "Query Open Orders & Trades" are sufficient.

### Environment variables (recommended for agents)

```bash
export KRAKEN_API_KEY="your-key"
export KRAKEN_API_SECRET="your-secret"
export KRAKEN_FUTURES_API_KEY="your-futures-key"       # optional
export KRAKEN_FUTURES_API_SECRET="your-futures-secret"  # optional
```

### Config file (for humans)

Store credentials in `~/.config/kraken/config.toml` (Linux) or
`~/Library/Application Support/kraken/config.toml` (macOS):

```toml
[auth]
api_key = "your-api-key"
api_secret = "your-api-secret"
```

Or use the interactive wizard: `kraken setup`. Run it in a normal terminal,
because it reads your input. Inside an AI agent, use the environment variables
above. Never paste API keys into an AI chat.

### Credential resolution

Highest precedence first:

1. Command-line flags (`--api-key`, `--api-secret`)
2. Environment variables (`KRAKEN_API_KEY`, `KRAKEN_API_SECRET`)
3. The active stored key in `config.toml` (`~/.config/kraken/` on Linux,
   `~/Library/Application Support/kraken/` on macOS): the API key from
   `kraken auth set`, or the sign-in key from `kraken auth login`

Both stored keys can exist at the same time. Without a choice, the API key is
active. `kraken auth switch api-key` or `kraken auth switch sign-in` changes the
active key, and `kraken auth show` tells you which key is in use. A new key
never becomes active without your approval: in a terminal the CLI asks, and in a
script it keeps the active key. The exception is an active key that cannot sign
(expired, missing, or a choice this release cannot read): the next stored key
replaces it without a prompt.

### Security

- Config file is created with `0600` permissions (owner read/write only)
- Secrets are never logged, printed, or included in error messages
- Use `--api-secret-stdin` or `--api-secret-file` instead of `--api-secret` to
  avoid secrets in process listings
- For automation, prefer environment variables over command-line flags

<details>
<summary>Advanced: API endpoint overrides</summary>

| Variable                   | Default                                         | Description                                      |
| -------------------------- | ----------------------------------------------- | ------------------------------------------------ |
| `KRAKEN_SPOT_URL`          | `https://api.kraken.com`                        | Spot REST API base URL                           |
| `KRAKEN_FUTURES_URL`       | `https://futures.kraken.com/derivatives/api/v3` | Futures REST API base URL                        |
| `KRAKEN_WS_PUBLIC_URL`     | `wss://ws.kraken.com/v2`                        | Public WebSocket endpoint                        |
| `KRAKEN_WS_AUTH_URL`       | `wss://ws-auth.kraken.com/v2`                   | Authenticated Spot WebSocket endpoint            |
| `KRAKEN_WS_L3_URL`         | `wss://ws-l3.kraken.com/v2`                     | Level 3 order book WebSocket endpoint            |
| `KRAKEN_FUTURES_WS_URL`    | `wss://futures.kraken.com/ws/v1`                | Futures WebSocket endpoint                       |
| `KRAKEN_FEEDBACK_ENDPOINT` | `https://cli-feedback.kraken.com/v1/feedback`   | Product feedback ingest URL (trusted hosts only) |

Resolution: CLI flag (`--api-url`, `--futures-url`, `--ws-public-url`,
`--ws-auth-url`, `--ws-l3-url`, `--ws-futures-url`) > environment variable >
default. `KRAKEN_FEEDBACK_ENDPOINT` has no flag. Only `https://` and `wss://`
schemes are accepted.

</details>

## MCP Server

`kraken-cli` includes a built-in
[Model Context Protocol](https://modelcontextprotocol.io/) server over stdio. No
subprocess wrappers needed.

MCP tool calls run through the same command path as CLI commands and inherit the
same error handling and rate-limit behavior.

> [!WARNING] MCP is local-first and designed for your own machine. Any agent
> connected to this MCP server uses the same configured Kraken account and API
> key permissions. Do not expose, tunnel, or share this server outside systems
> you control. Always use `https://` and `wss://` endpoints. Treat this
> integration as alpha and use least-privilege API keys.

```bash
kraken-mcp                           # default: market, account, paper, workspace, feedback
kraken-mcp -s all                    # all services, guarded: dangerous calls need approval (see below)
kraken-mcp -s all --allow-dangerous  # all services, no per-call confirmation
kraken-mcp -s market,trade,paper     # specific services
```

### Claude Code

Install `kraken-cli` with the one-liner first: the plugin and the command below
start `kraken-mcp`, which it installs. Then install the plugin (MCP server and
skills):

```text
/plugin marketplace add krakenfx/kraken-cli
/plugin install kraken-cli@kraken
```

Or add only the MCP server. The full path works even before `kraken-mcp` is on
your PATH:

```bash
claude mcp add kraken -- "$HOME/.cargo/bin/kraken-mcp" -s market,paper,feedback
```

If you set `CARGO_HOME`, use `$CARGO_HOME/bin/kraken-mcp` instead.

If you installed `kraken-cli` while Claude Code was running, quit Claude Code
and start it again from a new terminal window.

### Other MCP clients

Configure your MCP client (Claude Desktop, ChatGPT, Codex, Gemini CLI, Cursor,
VS Code, Windsurf, etc.):

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

This starts market data, paper trading, and the feedback tool, which uploads
text to Kraken (see [Service groups](#service-groups)). To add live trading, use
`market,paper,trade` or `all`, with a least-privilege API key. If the client
shows "Failed to connect", set `command` to the full path that
`command -v kraken-mcp` prints.

Gemini CLI users can install directly:
`gemini extensions install https://github.com/krakenfx/kraken-cli`

### Service groups

| Service       | Auth  | Risk                                                                                           |
| ------------- | ----- | ---------------------------------------------------------------------------------------------- |
| market        | No    | None                                                                                           |
| account       | Yes   | Read-only, except `export-retrieve` and `export-delete` (dangerous)                            |
| trade         | Yes   | Orders (dangerous)                                                                             |
| funding       | Yes   | Withdrawals (dangerous)                                                                        |
| earn          | Yes   | Staking (dangerous)                                                                            |
| subaccount    | Yes   | Transfers (dangerous)                                                                          |
| futures       | Mixed | Orders (dangerous)                                                                             |
| futures-paper | No    | None (simulation)                                                                              |
| paper         | No    | Simulation; `reset` is dangerous; in a live workspace `status`/`balance` read the real account |
| workspace     | No    | `reset` and `promote` are dangerous; live workspaces gate like the real account                |
| feedback      | No    | Uploads text to Kraken (see below)                                                             |
| auth          | Mixed | Read-only                                                                                      |

Default: `market,account,paper,workspace,feedback`. The `feedback` tool calls
the same upload path as `kraken feedback`. It sends the text you give it (at
most 8192 bytes after trimming), the feedback type, the optional `--llm-model`
value, the CLI version, the OS and architecture, the detected agent client, a
stable install ID, a random event ID and a timestamp to
`https://cli-feedback.kraken.com/v1/feedback` over HTTPS. Do not put secrets or
personal data in it. A failed upload fails the command and is not retried. The
streaming groups (`websocket`, `futures-ws`) and `utility` (the setup wizard)
are not served over MCP.

Dangerous tools carry the `destructive_hint` annotation and include
`[DANGEROUS: requires human confirmation]` in the description. The server runs
in guarded mode unless you pass `--allow-dangerous`, whichever services you
load. In guarded mode it puts each dangerous call to you through MCP elicitation
and runs it only if you approve. A client that cannot present an elicitation
gets no prompt from the server: the call runs when the model sets
`acknowledged=true` in its arguments, so on such a client rely on the client's
own tool-approval prompt. In autonomous mode (`--allow-dangerous`), neither is
asked for.

<details>
<summary>Output & Errors</summary>

### JSON (`-o json`)

The primary output format. Every command returns valid JSON on stdout.

```bash
kraken ticker BTCUSD -o json
# {"XXBTZUSD":{"ask_price":81401.4,"bid_price":81401.3,"last_price":81401.3,"volume_24h":1371.77,...}}
```

Results are keyed by Kraken's own pair name (`XXBTZUSD`), with named fields and
numeric values.

WebSocket commands emit JSONL (one JSON object per line) for streaming.

### Table (`-o table`)

Human-readable tables. Default when `-o` is not specified.

```text
┌──────────┬──────────┬──────────┬──────────┬────────────┐
│ Pair     │ Ask      │ Bid      │ Last     │ Volume ... │
├──────────┼──────────┼──────────┼──────────┼────────────┤
│ XXBTZUSD │ 67234.10 │ 67234.00 │ 67234.10 │ 1234.56    │
└──────────┴──────────┴──────────┴──────────┴────────────┘
```

### Error envelopes

On failure, stdout contains a JSON envelope with a stable `error` category:

```json
{ "error": "auth", "message": "Authentication failed: EAPI:Invalid key" }
```

Route on `error`, not on `message`. The `message` field is human-readable and
not stable.

| Category     | Retryable | Action                                                                                                                                                                                                               |
| ------------ | --------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `auth`       | No        | Check API key and secret                                                                                                                                                                                             |
| `rate_limit` | Yes       | Error includes `suggestion` with tier-specific limits, `docs_url` pointing to Kraken documentation, and `retryable` flag. Read `suggestion` and adapt strategy.                                                      |
| `network`    | Yes       | Exponential backoff, max 5 retries. If `verify_first` is `true`, check order or transfer state first                                                                                                                 |
| `validation` | No        | Fix input parameters                                                                                                                                                                                                 |
| `api`        | No        | Inspect request and parameters                                                                                                                                                                                       |
| `config`     | No        | Check config file or env vars                                                                                                                                                                                        |
| `websocket`  | Yes       | Spot streams reconnect with backoff until interrupted. `futures ws` stops after 12 failed reconnects in a row. `ws` order methods: if the envelope has `"verify_first": true`, check open orders before resubmitting |
| `io`         | No        | Check file paths and permissions                                                                                                                                                                                     |
| `timeout`    | Yes       | An operation ran past its time budget (`session stop`, a WebSocket token request, browser sign-in). Re-run; if `session stop` keeps timing out, check `kraken session show`                                          |
| `parse`      | No        | Log raw response, possible API maintenance                                                                                                                                                                           |

A `futures ws` reconnect counts as failed unless the new connection stays up for
30 seconds.

Full error catalog with envelopes and backoff strategies:
[agents/error-catalog.json](agents/error-catalog.json).

### Logging (`-v`, `--log-format`, `RUST_LOG`)

Diagnostics are emitted via `tracing` to **stderr only** — stdout stays the pure
result/JSON channel. Logging is **off by default** except warnings and errors,
so the `2>/dev/null` pattern is unaffected. Secrets, credentials, and financial
data are never logged.

**Format** — structured JSON (JSONL, one object per line) by default, so logs
are machine-parseable out of the box. A human-readable format is opt-in:

```bash
kraken balance -o json -v 2>/dev/null | jq .   # result on stdout; logs (JSON) on stderr
kraken balance -v --log-format pretty           # human-readable logs instead
export KRAKEN_LOG_FORMAT=pretty                 # ...or via env (e.g. a dev shell profile)
```

`--log-format` accepts `json` (default), `pretty`, or `compact`, and is
independent of `-o`/`--output` (which formats the command result on stdout).

**Level** — controlled by `RUST_LOG` (the standard `tracing` filter);
`-v`/`--verbose` is a shortcut for debug-level kraken-cli logs:

```bash
kraken balance -v                                  # kraken-cli at debug, dependencies at warn
RUST_LOG=warn,kraken=debug kraken balance          # same, explicit
RUST_LOG=kraken_rest=trace kraken ticker BTCUSD   # HTTP request/response detail
```

Note: a bare `RUST_LOG=debug` enables debug for _every_ crate (including
dependencies like `tungstenite`); scope it to `kraken`, which matches every
`kraken_*` crate, or just use `-v` for our diagnostics alone.

</details>

## Workspaces, Sessions, and Research

The CLI has a durable paper-research loop:

1. A **workspace** isolates the account.
2. A **session** records an evaluation window.
3. A **tape** preserves replayable market data.
4. A **lab experiment** seals the hypothesis and pass criteria.

```bash
# Create an isolated account.
kraken workspace create momentum --capital 10000 --mode paper -o json
export KRAKEN_WORKSPACE=momentum

# Record in one terminal.
kraken session start --symbols BTC/USD --channels ticker,trade \
  --label baseline --to jsonl -o json

# Trade in another terminal and preserve the rationale.
export KRAKEN_WORKSPACE=momentum
kraken paper buy BTC/USD 0.01 -o json
kraken session note --kind buy --symbol BTC/USD \
  --reason "breakout held above the prior high" -o json
kraken session stop -o json

# Review the closed window.
kraken session show --session baseline -o json
kraken explain pnl --session baseline -o json
kraken workspace report momentum -o json
```

For repeatable experiments, freeze the plan before the first session. A replay
leg names a finalized tape: `replay:session:s1` is the baseline session above,
the first one in this workspace (`kraken session list` shows each ordinal).

```bash
kraken lab new breakout-v1 \
  --hypothesis "a confirmed breakout produces positive net return" \
  --min-return-pct 0 --max-drawdown-pct 2 --min-fills 1 \
  --session live:5m --session replay:session:s1 -o json

kraken lab next breakout-v1 -o json
kraken lab compare breakout-v1 -o json
```

The command tables below cover each surface. Machine-readable contracts live in
[agents/tool-catalog.json](agents/tool-catalog.json).

## Commands

The published catalog contains 188 commands across 15 groups; 152 of them are
callable as MCP tools, and the rest say why they are not. Parameter schemas live
in [agents/tool-catalog.json](agents/tool-catalog.json). Long-running streams
are in the catalog too, with `reach: stream`, and are documented below. They
stay CLI-only.

| Group         | Commands | Auth  | Description                                                                                                                                     |
| ------------- | -------- | ----- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| market        | 11       | No    | Ticker, orderbook, OHLC, trades, spreads, asset info, tape library                                                                              |
| account       | 18       | Yes   | Balances, orders, trades, ledgers, positions, exports                                                                                           |
| trade         | 9        | Yes   | Order placement, amendment, cancellation (spot, xStocks, forex)                                                                                 |
| funding       | 10       | Yes   | Deposits, withdrawals, wallet transfers                                                                                                         |
| earn          | 6        | Yes   | Staking strategies and allocations                                                                                                              |
| subaccount    | 2        | Yes   | Create subaccounts, transfer between accounts                                                                                                   |
| futures       | 39       | Mixed | Futures market data and trading                                                                                                                 |
| futures-paper | 17       | No    | Futures paper trading simulation with live prices                                                                                               |
| futures-ws    | 9        | Mixed | Futures WebSocket streaming                                                                                                                     |
| websocket     | 17       | Mixed | Spot WebSocket v2 streaming and request/response                                                                                                |
| paper         | 17       | No    | Spot paper trading simulation with live prices, P&L explanation, and the Autoresearch Lab                                                       |
| workspace     | 25       | No    | Strategy workspaces, recorded sessions, recipes, and extension scaffolds: isolated accounts, session windows, decision logs, reports, promotion |
| feedback      | 1        | No    | Product feedback upload to Kraken                                                                                                               |
| auth          | 6        | Mixed | Browser sign-in and credential management                                                                                                       |
| utility       | 1        | No    | Interactive setup (the REPL is the `kraken-shell` binary)                                                                                       |

43 commands are marked `dangerous` (orders, withdrawals, transfers, cancel-all,
staking, account and margin settings, browser sign-in, switching the active
key). The authoritative list is the `dangerous` field in
[agents/tool-catalog.json](agents/tool-catalog.json).

<details>
<summary>Full command reference</summary>

### Market Data (Public)

| Command                                                                     | Description                                                     |
| --------------------------------------------------------------------------- | --------------------------------------------------------------- |
| `kraken status`                                                             | System status                                                   |
| `kraken server-time`                                                        | Server time                                                     |
| `kraken assets [--asset BTC,ETH] [--asset-class CLASS]`                     | Asset info                                                      |
| `kraken pairs [--pair BTCUSD] [--asset-class CLASS]`                        | Tradable pairs                                                  |
| `kraken ticker <PAIR...> [--asset-class tokenized_asset\|forex]`            | Ticker data                                                     |
| `kraken ohlc <PAIR> [--interval 60] [--asset-class tokenized_asset\|forex]` | OHLC candles                                                    |
| `kraken orderbook <PAIR> [--count 25] [--asset-class tokenized_asset]`      | L2 order book                                                   |
| `kraken orderbook-grouped <PAIR> [--depth 10] [--grouping 1]`               | Grouped order book                                              |
| `kraken trades <PAIR> [--count 1000] [--asset-class tokenized_asset]`       | Recent trades                                                   |
| `kraken spreads <PAIR> [--since TS] [--asset-class tokenized_asset]`        | Recent spreads                                                  |
| `kraken tape list`                                                          | Every recorded tape in the library: ref, backend, state, in use |

### Account Data (Private)

| Command                                                                                                                                                                                                  | Description                               |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------- |
| `kraken orderbook-l3 <PAIR> [--depth 100]`                                                                                                                                                               | L3 order book                             |
| `kraken balance [--rebase-multiplier rebased\|base]`                                                                                                                                                     | All balances                              |
| `kraken extended-balance`                                                                                                                                                                                | Extended balances (balance, credit, held) |
| `kraken credit-lines [--rebase-multiplier rebased\|base]`                                                                                                                                                | Credit line details (VIP)                 |
| `kraken trade-balance [--asset USD] [--rebase-multiplier rebased\|base]`                                                                                                                                 | Margin/equity summary                     |
| `kraken open-orders [--trades] [--cl-ord-id ID] [--rebase-multiplier rebased\|base]`                                                                                                                     | Open orders                               |
| `kraken closed-orders [--start TS] [--end TS] [--offset N] [--userref REF] [--cl-ord-id ID] [--closetime open\|close\|both] [--consolidate-taker] [--without-count] [--rebase-multiplier rebased\|base]` | Closed orders                             |
| `kraken query-orders <TXID...> [--trades] [--userref REF] [--consolidate-taker] [--rebase-multiplier rebased\|base]`                                                                                     | Query specific orders                     |
| `kraken trades-history [--type TYPE] [--trades] [--offset N] [--consolidate-taker] [--without-count] [--ledgers] [--rebase-multiplier rebased\|base]`                                                    | Trade history                             |
| `kraken query-trades <TXID...> [--trades] [--rebase-multiplier rebased\|base]`                                                                                                                           | Query specific trades                     |
| `kraken positions [--txid ID] [--show-pnl] [--consolidation market] [--rebase-multiplier rebased\|base]`                                                                                                 | Open positions                            |
| `kraken ledgers [--asset BTC --type trade] [--asset-class CLASS] [--start TS] [--end TS] [--offset N] [--without-count] [--rebase-multiplier rebased\|base]`                                             | Ledger entries                            |
| `kraken query-ledgers <ID...> [--trades] [--rebase-multiplier rebased\|base]`                                                                                                                            | Query specific ledger entries             |
| `kraken volume [--pair BTCUSD] [--rebase-multiplier rebased\|base]`                                                                                                                                      | Trade volume & fees                       |
| `kraken export-report --report trades --description "desc" [--format CSV\|TSV] [--fields F] [--starttm TS] [--endtm TS]`                                                                                 | Request export                            |
| `kraken export-status --report trades`                                                                                                                                                                   | Export status                             |
| `kraken export-retrieve <ID>`                                                                                                                                                                            | Download export                           |
| `kraken export-delete <ID>`                                                                                                                                                                              | Delete export                             |

### Trading

| Command                                                                                                    | Description                 |
| ---------------------------------------------------------------------------------------------------------- | --------------------------- |
| `kraken order buy <PAIR> <VOL> [--type limit --price P] [--asset-class tokenized_asset] [--validate] ...`  | Buy order                   |
| `kraken order sell <PAIR> <VOL> [--type limit --price P] [--asset-class tokenized_asset] [--validate] ...` | Sell order                  |
| `kraken order batch <JSON_FILE> [--pair P] [--asset-class tokenized_asset] [--deadline DL] [--validate]`   | Batch orders (2-15)         |
| `kraken order amend --txid <TXID> [--order-qty Q --limit-price P ...]`                                     | Amend order in-place        |
| `kraken order amend --cl-ord-id <ID> [--limit-price P ...]`                                                | Amend by client order ID    |
| `kraken order edit <TXID> [--price P]`                                                                     | Edit order (cancel+replace) |
| `kraken order cancel <TXID...>`                                                                            | Cancel by txid(s)           |
| `kraken order cancel --cl-ord-id <ID>`                                                                     | Cancel by client order ID   |
| `kraken order cancel-batch <TXID...> [--cl-ord-ids ID...]`                                                 | Cancel batch (max 50 total) |
| `kraken order cancel-all [--yes]`                                                                          | Cancel all                  |
| `kraken order cancel-after <SECS>`                                                                         | Dead man's switch           |

### Funding

| Command                                                                                                                                                        | Description               |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------- |
| `kraken deposit methods <ASSET> [--asset-class currency\|tokenized_asset] [--rebase-multiplier rebased\|base]`                                                 | Deposit methods           |
| `kraken deposit addresses <ASSET> <METHOD> [--new] [--asset-class CLASS] [--amount AMT]`                                                                       | Deposit addresses         |
| `kraken deposit status [--asset A] [--asset-class CLASS] [--method M] [--start TS] [--end TS] [--cursor C] [--limit N] [--rebase-multiplier rebased\|base]`    | Deposit status            |
| `kraken withdraw <ASSET> <KEY> <AMOUNT> [--asset-class CLASS] [--address ADDR] [--max-fee F] [--rebase-multiplier rebased\|base]`                              | Withdraw funds            |
| `kraken withdrawal methods [--asset A] [--asset-class CLASS] [--network N] [--rebase-multiplier rebased\|base]`                                                | Withdrawal methods        |
| `kraken withdrawal addresses [--asset A] [--asset-class CLASS] [--method M] [--key K] [--verified true\|false]`                                                | Withdrawal addresses      |
| `kraken withdrawal info <ASSET> <KEY> <AMOUNT>`                                                                                                                | Withdrawal fee info       |
| `kraken withdrawal status [--asset A] [--asset-class CLASS] [--method M] [--start TS] [--end TS] [--cursor C] [--limit N] [--rebase-multiplier rebased\|base]` | Withdrawal status         |
| `kraken withdrawal cancel <ASSET> <REFID>`                                                                                                                     | Cancel pending withdrawal |
| `kraken wallet-transfer <ASSET> <AMT> --from IIBAN --to IIBAN`                                                                                                 | Wallet transfer           |

### Earn

| Command                                                                                                    | Description         |
| ---------------------------------------------------------------------------------------------------------- | ------------------- |
| `kraken earn strategies [--asset ETH] [--lock-type flex bonded...] [--ascending] [--cursor C] [--limit N]` | List strategies     |
| `kraken earn allocate <ID> <AMT>`                                                                          | Allocate funds      |
| `kraken earn deallocate <ID> <AMT>`                                                                        | Deallocate funds    |
| `kraken earn allocate-status <ID>`                                                                         | Allocation status   |
| `kraken earn deallocate-status <ID>`                                                                       | Deallocation status |
| `kraken earn allocations [--ascending] [--converted-asset USD] [--hide-zero-allocations]`                  | Current allocations |

### Subaccounts

| Command                                                                                     | Description               |
| ------------------------------------------------------------------------------------------- | ------------------------- |
| `kraken subaccount create <USERNAME> <EMAIL>`                                               | Create sub account        |
| `kraken subaccount transfer <ASSET> <AMOUNT> --from IIBAN --to IIBAN [--asset-class CLASS]` | Transfer between accounts |

### Futures: Public Market Data

| Command                                                      | Description       |
| ------------------------------------------------------------ | ----------------- |
| `kraken futures instruments`                                 | All contracts     |
| `kraken futures tickers`                                     | All tickers       |
| `kraken futures ticker <SYMBOL>`                             | Single ticker     |
| `kraken futures orderbook <SYMBOL>`                          | Order book        |
| `kraken futures history <SYMBOL> [--since TS] [--before TS]` | Trade history     |
| `kraken futures feeschedules`                                | Fee schedules     |
| `kraken futures instrument-status [--symbol SYM]`            | Instrument status |

### Futures: Trading (Private)

| Command                                                                                                                                                                                                                                                     | Description               |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------- |
| `kraken futures order buy <SYM> <SIZE> [--type limit] [--price P] [--stop-price SP] [--trigger-signal mark\|index\|last] [--client-order-id ID] [--reduce-only] [--trailing-stop-max-deviation V] [--trailing-stop-deviation-unit percent\|quote_currency]` | Buy futures               |
| `kraken futures order sell <SYM> <SIZE> [--type limit] [--price P] [--stop-price SP] ...`                                                                                                                                                                   | Sell futures              |
| `kraken futures edit-order --order-id ID [--size S] [--price P] [--stop-price SP]`                                                                                                                                                                          | Edit existing order       |
| `kraken futures cancel --order-id ID`                                                                                                                                                                                                                       | Cancel by order ID        |
| `kraken futures cancel --cli-ord-id ID`                                                                                                                                                                                                                     | Cancel by client order ID |
| `kraken futures cancel-all [--symbol SYM] [--yes]`                                                                                                                                                                                                          | Cancel all orders         |
| `kraken futures cancel-after <SECS>`                                                                                                                                                                                                                        | Dead man's switch         |
| `kraken futures batch-order '<JSON>'` or `kraken futures batch-order @file.json`                                                                                                                                                                            | Batch order placement     |

### Futures: Account Data (Private)

| Command                                                                           | Description                          |
| --------------------------------------------------------------------------------- | ------------------------------------ |
| `kraken futures accounts`                                                         | Wallet/account info                  |
| `kraken futures open-orders`                                                      | Open orders                          |
| `kraken futures positions`                                                        | Open positions                       |
| `kraken futures fills [--since TS]`                                               | Recent fills                         |
| `kraken futures leverage [--symbol SYM]`                                          | Leverage preferences                 |
| `kraken futures set-leverage <SYMBOL> [LEVERAGE]`                                 | Set leverage (omit for cross margin) |
| `kraken futures notifications`                                                    | Account notifications                |
| `kraken futures transfers`                                                        | Transfer history                     |
| `kraken futures transfer <AMOUNT> <CURRENCY>`                                     | Transfer between wallets             |
| `kraken futures history-executions [--since TS] [--before TS] [--sort asc\|desc]` | Execution history                    |
| `kraken futures history-orders [--since TS] [--before TS] [--sort asc\|desc]`     | Order history                        |
| `kraken futures history-triggers [--since TS] [--before TS] [--sort asc\|desc]`   | Trigger history                      |
| `kraken futures history-account-log-csv [--since TS] [--before TS]`               | Account log (CSV)                    |
| `kraken futures trading-instruments [--contract-type ...]`                        | Trading instruments (auth)           |
| `kraken futures historical-funding-rates <SYMBOL>`                                | Historical funding rates             |
| `kraken futures order-status <ORDER_ID>...`                                       | Query order status (auth)            |
| `kraken futures pnl-preferences`                                                  | Get PnL preferences (auth)           |
| `kraken futures set-pnl-preference <SYMBOL> <PREFERENCE>`                         | Set PnL preference (auth)            |
| `kraken futures unwind-queue`                                                     | Get unwind queue (auth)              |
| `kraken futures assignment-programs`                                              | Assignment programs (auth)           |
| `kraken futures fee-schedule-volumes`                                             | Fee schedule volumes (auth)          |
| `kraken futures subaccounts`                                                      | List subaccounts (auth)              |
| `kraken futures subaccount-status <UID>`                                          | Subaccount trading status (auth)     |
| `kraken futures set-subaccount-status <UID> <ENABLED>`                            | Set subaccount trading status (auth) |
| `kraken futures wallet-transfer <FROM> <TO> <UNIT> <AMOUNT>`                      | Transfer between wallets (auth)      |

### Futures: Paper Trading (No Auth)

| Command                                                                                                                                                                                                                                                                   | Description                                                                     |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| `kraken futures paper init [--balance 10000] [--currency USD] [--fee-rate 0.0005]`                                                                                                                                                                                        | Initialize futures paper account                                                |
| `kraken futures paper reset [--balance B] [--currency C] [--fee-rate R]`                                                                                                                                                                                                  | Reset account (clear positions, orders, fills)                                  |
| `kraken futures paper balance`                                                                                                                                                                                                                                            | Collateral, used margin, available margin, unrealized P&L                       |
| `kraken futures paper status`                                                                                                                                                                                                                                             | Account summary: equity, P&L since start, unrealized P&L, position/order counts |
| `kraken futures paper buy <SYM> <SIZE> [--leverage L] [--type TYPE] [--price P] [--stop-price SP] [--trigger-signal mark\|index\|last] [--client-order-id ID] [--reduce-only] [--trailing-stop-max-deviation V] [--trailing-stop-deviation-unit percent\|quote_currency]` | Paper long (all 8 order types)                                                  |
| `kraken futures paper sell <SYM> <SIZE> [--leverage L] [--type TYPE] [--price P] ...`                                                                                                                                                                                     | Paper short (all 8 order types)                                                 |
| `kraken futures paper orders`                                                                                                                                                                                                                                             | Open resting orders                                                             |
| `kraken futures paper order-status <ID>`                                                                                                                                                                                                                                  | Query specific order                                                            |
| `kraken futures paper edit-order --order-id <ID> [--size S] [--price P] [--stop-price SP]`                                                                                                                                                                                | Edit resting order                                                              |
| `kraken futures paper cancel --order-id <ID>`                                                                                                                                                                                                                             | Cancel by order ID or `--cli-ord-id`                                            |
| `kraken futures paper cancel-all [--symbol SYM]`                                                                                                                                                                                                                          | Cancel all (optional symbol filter)                                             |
| `kraken futures paper batch-order '<JSON>'`                                                                                                                                                                                                                               | Batch order placement                                                           |
| `kraken futures paper positions`                                                                                                                                                                                                                                          | Open positions with mark price, liquidation price, unrealized P&L               |
| `kraken futures paper fills`                                                                                                                                                                                                                                              | Fill history                                                                    |
| `kraken futures paper history`                                                                                                                                                                                                                                            | Closed positions and cancelled orders with realized P&L                         |
| `kraken futures paper leverage [--symbol SYM]`                                                                                                                                                                                                                            | Current leverage preferences                                                    |
| `kraken futures paper set-leverage <SYM> <LEV>`                                                                                                                                                                                                                           | Set default leverage for a symbol                                               |

### Futures WebSocket Streaming

| Command                                | Description                            |
| -------------------------------------- | -------------------------------------- |
| `kraken futures ws ticker <MARKET...>` | Stream futures tickers (public)        |
| `kraken futures ws trades <MARKET...>` | Stream futures trades (public)         |
| `kraken futures ws book <MARKET...>`   | Stream futures order book (public)     |
| `kraken futures ws fills`              | Stream fills (private, challenge auth) |
| `kraken futures ws open-orders`        | Stream open orders (private)           |
| `kraken futures ws open-positions`     | Stream open positions (private)        |
| `kraken futures ws balances`           | Stream balances and margins (private)  |
| `kraken futures ws notifications`      | Stream notifications (private)         |
| `kraken futures ws account-log`        | Stream account log (private)           |

### WebSocket (Streaming and Request/Response)

| Command                                            | Description                              |
| -------------------------------------------------- | ---------------------------------------- |
| `kraken ws ticker <PAIR...> [--event-trigger bbo]` | Stream tickers                           |
| `kraken ws trades <PAIR...>`                       | Stream trades                            |
| `kraken ws book <PAIR...> [--depth 10]`            | Stream L2 order book                     |
| `kraken ws ohlc <PAIR...> [--interval 1]`          | Stream OHLC candles                      |
| `kraken ws instrument [PAIR...]`                   | Stream instrument metadata               |
| `kraken ws executions`                             | Stream trade executions (auth)           |
| `kraken ws balances`                               | Stream balance updates (auth)            |
| `kraken ws level3 <PAIR...>`                       | Stream L3 order book (auth)              |
| `kraken ws add-order`                              | Place order via WebSocket (auth)         |
| `kraken ws amend-order`                            | Amend order via WebSocket (auth)         |
| `kraken ws cancel-order`                           | Cancel order(s) via WebSocket (auth)     |
| `kraken ws cancel-all`                             | Cancel all orders via WebSocket (auth)   |
| `kraken ws cancel-after <TIMEOUT>`                 | Dead man's switch via WebSocket (auth)   |
| `kraken ws batch-add`                              | Batch add orders via WebSocket (auth)    |
| `kraken ws batch-cancel`                           | Batch cancel orders via WebSocket (auth) |
| `kraken ws ping`                                   | Check the connection with a ping/pong    |

### Market Memory (CLI-only, no auth)

| Command                                                                                  | Description                                                          |
| ---------------------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| `kraken record --symbols <PAIR...> --channels <CH...> [--tape NAME] [--to duckdb,jsonl]` | Capture live market data into a local tape; Ctrl-C/SIGTERM finalizes |
| `kraken streamd start --symbols <PAIR...> --channels <CH...>`                            | Multi-symbol, multi-channel JSONL stream to stdout                   |

Add `--monitor` to `ws`, `streamd`, or `record` for stderr JSONL health lines
(latency, trade gaps, staleness, heartbeats). See the
[market memory walkthrough](examples/03-market-memory.md).

`--to` defaults to `jsonl`, which every build has. `--to duckdb` needs a binary
built with the `record-duckdb` feature — the released binaries have it, a source
build opts in (see [Installation](#installation)).

### Strategy Workspaces

A workspace is **an allocation with a ledger**. You give it capital, it records
everything that happens to that capital, and it reports P&L against it.

There are three scopes, and one command vocabulary across all of them:

| Scope          | Capital is                       | Fills come from               | `kraken order buy`        |
| -------------- | -------------------------------- | ----------------------------- | ------------------------- |
| No workspace   | your whole account               | the venue                     | real order, whole account |
| `--mode paper` | simulated, minted                | the simulator, at live prices | simulated                 |
| `--mode live`  | **a slice of your real account** | the venue, reconciled by txid | **real order**            |

The point of the middle and bottom rows sharing a vocabulary is that moving a
strategy from paper to live teaches you nothing new. You test with `order buy`,
`open-orders`, `balance`, `workspace status`, `trades-history`, and
`workspace report` — and after promotion you keep using exactly those. No flag
changes, no different verb, no separate live command set.

Because the commands are identical, the _output_ is what has to tell you where
you are. Order and status tables lead with a `Mode` row, order and trade lists
put `[PAPER]` or `[LIVE]` in their first column header, and the JSON carries a
`mode` field. A paper balance table carries no marker. The same `order buy`, in
the two modes:

```text
$ kraken order buy BTC/USD 0.01 --type limit --price 60000     # paper workspace
┌───────────┬───────────────────────────┐
│ Mode      ┆ [PAPER] Simulated Trading │   ← simulated, no money moved
│ Workspace ┆ sandbox                   │
│ Action    ┆ Limit buy order placed    │
│ Order ID  ┆ PAPER-00003               │   ← a minted id
│ …         ┆ …                         │
└───────────┴───────────────────────────┘

$ kraken order buy BTC/USD 0.01 --type limit --price 60000     # live workspace
live: this goes to the real Kraken account                        ← stderr, every command
workspace 'prod' is an allocation ledger over that account, not a separate balance
┌───────────┬──────────────────────────────────────────────┐
│ Mode      ┆ [LIVE] Real Money                            │   ← a REAL order at Kraken
│ Workspace ┆ prod                                         │
│ Scope     ┆ REAL orders — allocation of the real account │
│ Action    ┆ Limit buy order placed                       │
│ Order ID  ┆ OQABCD-1EFGH-2IJKLM                          │   ← a venue txid
│ …         ┆ …                                            │
└───────────┴──────────────────────────────────────────────┘

allocation 1000 USD of 5000.00 USD held; 4000.00 USD unallocated   ← what backs it
```

Same shape, same row names, same order — so a script or an agent reads one
format, and a human sees the difference in the two rows they read first. `Mode`
answers "is this real money?" and `Scope` answers "whose money?", including with
no workspace at all:

| Scope          | `Mode`                      | `Scope`                                        |
| -------------- | --------------------------- | ---------------------------------------------- |
| No workspace   | `[LIVE] Real Money`         | `the WHOLE real account, no workspace limit`   |
| `--mode paper` | `[PAPER] Simulated Trading` | — (a `Workspace` row names it)                 |
| `--mode live`  | `[LIVE] Real Money`         | `REAL orders — allocation of the real account` |

Three further signals mark a live command, and all three are always present:

1. stderr carries the `live:` banner, on every command.
2. A footer sets the allocation against your real balance.
3. Order IDs are the venue's own transaction IDs, never `PAPER-…`.

In `-o json` the same facts are `"mode": "live"`, `"scope": "allocation"`,
`"reconciled"`, and the `backing` object. Order IDs are the other tell: paper
mints `PAPER-00001`, live carries the venue's own transaction ID.

`workspace create --mode live` and `workspace promote` print the full
explanation once, at the moment you opt in.

Two contract fields describe the simulator's fill model and therefore apply to
paper only: `--fee-rate` and `--slippage-rate`. A live workspace's fills carry
Kraken's own fee, so its tables print `Fees: Kraken's own, charged per fill`
instead of the rates, and its capital row is labelled `Allocation` rather than
`Capital` — it is a claim on money you already have, not capital the CLI minted.

#### What a live allocation is, and is not

Say your account holds 1000 BTC and you allocate 100 BTC to a workspace.

**It is** a real carve-out for accounting. Orders are real, fills are the
venue's own (reconciled by transaction ID, so the P&L is real and not notional),
and this CLI checks the allocation before every order — a spend the workspace's
own ledger cannot fund is refused before the venue sees it, the same way a paper
workspace refuses an order its capital cannot fund. The live check skips the
simulator's fee padding, so its numbers always match `workspace balance` rather
than paper's fee-padded refusals. Creating the workspace verifies your account
actually holds the allocation, net of what other live workspaces already claim,
and refuses if it does not.

**It is not** a subaccount, and Kraken enforces none of it. Funds, permissions
and positions stay shared with the whole account, so anything else using the
same API key — another workspace, another tool, the web UI — can spend allocated
money. That residual is reported rather than hidden: every live command carries
a `backing` object setting the allocation against your real balance. Nor is it a
security boundary: the workspace is plain local state — a manifest and a journal
on disk, not tamper-evident — so use it to scope and attribute an agent's
trading, never to contain an agent you do not trust.

```bash
kraken workspace balance -o json | jq '.backing'
# { "status": "backed", "asset": "BTC", "allocation": "100",
#   "account": "1000.0000", "allocated_elsewhere": "150",
#   "unallocated": "750.0000", "spendable": "97.5" }
```

`spendable` is the ledger's own figure — the allocation-asset balance free of
resting orders — so an agent can size an order without a refusal round-trip. For
orders that spend the allocation asset it is exactly what the pre-order check
compares a spend against; an order on a pair carrying the allocation on neither
leg is gated in its own funding leg instead. A refusal itself is structured the
same way in both output modes: the JSON envelope carries `refusal`
(`allocation_budget`, `over_allocated`, or `unverified`) with
`available`/`required`/`backing` as typed fields, and the table render prints
the same facts as rows.

`status` is `backed`, `over_allocated`, or `unverified`. An over-allocated
workspace stops spending the allocated asset until you move funds back into the
account, and an unverified one — the real balance could not be read — stops
spending until it can be checked again, exactly as `workspace create` refuses an
allocation it cannot verify. The order that pays that asset back in — closing a
position — is always allowed, so neither a shortfall nor an outage can ever hold
you in a trade.

#### What carries over, and what cannot

Everything you use to run and review a strategy carries over. Recorded sessions
work in a live workspace: `session start` opens a window at your real equity,
records market data alongside your decisions, and `session stop` reconciles the
window by transaction ID. `explain pnl` decomposes it into price movement, fees
and spread exactly as it does on paper, and `workspace report`, `open-orders`,
`balance` and `trades-history` all read the same ledger.

Four things are refused in a live workspace, and each says so when you hit it:

| Refused                                                     | Why                                       | Instead                           |
| ----------------------------------------------------------- | ----------------------------------------- | --------------------------------- |
| `paper buy`/`sell`/`orders`/`cancel`/`cancel-all`/`history` | simulates fills                           | `order buy`, or a paper workspace |
| `session start --from tape:…`                               | replay settles fills from recorded prices | record live (omit `--from`)       |
| `lab new --session replay:…`                                | sealing a leg this scope cannot run       | seal replay plans on paper        |
| `workspace reset`                                           | the journal holds real venue fills        | reset a paper workspace           |

Some verbs have no paper equivalent, so they are refused inside any paper or
live workspace: `order amend`, `edit`, `batch`, `cancel-batch` and
`cancel-after`; `withdraw`, `withdrawal cancel` and `wallet-transfer`;
`earn allocate` and `deallocate`; `subaccount transfer`; `futures order`,
`edit-order`, `cancel`, `cancel-all`, `cancel-after`, `batch-order`, `transfer`,
`wallet-transfer` and `set-subaccount-status`; and the `ws` order methods. Run
them with no workspace selected. Futures paper is refused too, apart from six
reads, because its state is global (see
[Futures Paper Trading](#futures-paper-trading)).

Scoring is **not** refused. `lab show`, `lab score`, `lab compare` and
`lab next` keep reading the experiment that justified the promotion — a
scorecard folds the session's own journal window and marks it against the tape
recorded beside it, so a paper-era session re-scores to the same numbers after
the switch. That matters because `workspace promote` grades its own checklist
through the same read. The scorecard is stamped with the scope it came from, so
a live workspace's sessions can never be mistaken for paper evidence.

The paper workspace is still worth keeping after promotion: it stays the place
you seal and run new hypotheses, replay legs included, while the live workspace
runs the one that already passed.

| Command                                                                                                                                            | Description                                                                                                                                                                                                                                                            |
| -------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `kraken workspace create <NAME> --capital <AMOUNT> --mode paper\|live [--currency USD] [--fee-rate R] [--slippage-rate R] [--allow-pairs PAIR...]` | Create a paper ledger, or allocate part of the real account to a live one. `--fee-rate`/`--slippage-rate` configure the simulator and apply to `--mode paper` only; `--mode live` needs working credentials so the allocation can be checked against your real balance |
| `kraken workspace list`                                                                                                                            | List named workspaces plus the real-account `default` workspace                                                                                                                                                                                                        |
| `kraken workspace show [NAME]`                                                                                                                     | Show the durable workspace contract                                                                                                                                                                                                                                    |
| `kraken workspace status [NAME]`                                                                                                                   | Mark account equity and P&L                                                                                                                                                                                                                                            |
| `kraken workspace balance [NAME]`                                                                                                                  | Fold balances from the local journal; `default` delegates to Kraken                                                                                                                                                                                                    |
| `kraken workspace reset [NAME] [--yes]`                                                                                                            | Start a new capital epoch while retaining history                                                                                                                                                                                                                      |
| `kraken workspace report [NAME]`                                                                                                                   | Report session, manual-trade, reset, drift, and total account results                                                                                                                                                                                                  |
| `kraken workspace promote [NAME]`                                                                                                                  | Promote qualified paper evidence to live execution                                                                                                                                                                                                                     |

Select a workspace with `--workspace <NAME>` or `KRAKEN_WORKSPACE=<NAME>`.

A workspace is an allocation with a ledger. Both modes keep the same
event-sourced journal, so `workspace status`, `balance`, `report`, and
`explain pnl` read identically; the only thing that differs is where fills come
from. A paper workspace mints its capital and simulates fills at live prices. A
live workspace allocates part of your real account — say 100 BTC out of the 1000
BTC you hold — sends real Spot orders through the normal credentials, and
reconciles the venue's own fills back into that workspace's ledger by
transaction ID, so its P&L is real rather than notional.

The allocation is verified against your actual balance when you create it (net
of what other live workspaces already claim) and enforced locally before each
order. Kraken does not enforce it: another tool on the same API key can spend
allocated funds at any time. That is why every live read carries a `backing`
object setting the allocation against the real balance, and why a workspace
whose account no longer covers its claim reports `over_allocated` and refuses to
open new positions.

Without a workspace, order verbs address the whole account exactly as before.

### Recorded Sessions

| Command                                                                                                     | Description                                                              |
| ----------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| `kraken session start --symbols <PAIR...> [--label LABEL] [--experiment NAME]`                              | Start a live recorded evaluation window                                  |
| `kraken session start --from tape:<NAME> [--speed N] [--label LABEL] [--experiment NAME]`                   | Start a replay-backed paper session                                      |
| `kraken session stop`                                                                                       | Finalize the active session and its account window                       |
| `kraken session list`                                                                                       | List sessions and their derived state                                    |
| `kraken session show [--session latest\|s<N>\|LABEL]`                                                       | Inspect the window, provenance, and valuation                            |
| `kraken session note --kind buy\|sell\|cancel\|skip\|alert --reason <TEXT> [--symbol PAIR] [--order-id ID]` | Append the reasoning behind a decision                                   |
| `kraken session decisions [--session REF]`                                                                  | Read the stable decision-log contract                                    |
| `kraken session state get [--session REF]`                                                                  | Read an agent's typed resumption cursor                                  |
| `kraken session state set [FIELDS...]`                                                                      | Replace the active session's resumption cursor                           |
| `kraken replay [--session REF] [--speed N]`                                                                 | Play a recorded session's timeline back to stdout                        |
| `kraken playground [--symbols PAIR...] [--template SKILL] [--for 1h]`                                       | One-command demo: a recorded session in the `playground` paper workspace |

`session start`, `record`, `replay`, `playground`, `recipe run`, and `streamd`
are CLI-only streams. Request/response operations around them remain available
through MCP.

### Recipes

| Command                                                                                                | Description                                                                                              |
| ------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------- |
| `kraken recipe create <NAME> [--from-session REF] [--driver SKILL] [--description TEXT] [--overwrite]` | Derive a reusable recipe from a stopped paper session: an editable `recipe.json` plus a draft `SKILL.md` |
| `kraken recipe list`                                                                                   | List the local recipe library                                                                            |
| `kraken recipe show <NAME>`                                                                            | Show a recipe's contract and where its artifacts live                                                    |
| `kraken recipe run <NAME> [--for 15m] [--from REF] [--speed N] [--experiment NAME] [--real-account]`   | Provision (or reuse) its paper workspace and start the session it records; `--real-account` asks first   |

A recipe records what to run again — driver skill, frozen strategy params,
market feeds, and provenance — as one portable directory under the global config
dir. It is an editable input, not sealed evidence; the lab's experiments keep
the sealing role.

### Extensions

| Command                                                                                                  | Description                                                                              |
| -------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| `kraken plugin init <NAME> --lang python\|bash [--kind indicator\|alert\|exporter\|driver] [--path DIR]` | Scaffold a runnable extension: entrypoint, `extension.json`, loadable `SKILL.md`, README |
| `kraken plugin list`                                                                                     | List scaffolded extensions: name, kind, language, entrypoint                             |

There is no plugin ABI: the CLI's versioned JSON output is the extension API.
The scaffolded entrypoint shells out to `kraken ... -o json` and prints;
kraken-cli never executes extension code.

### Autoresearch Lab and P&L

| Command                                                | Description                                               |
| ------------------------------------------------------ | --------------------------------------------------------- |
| `kraken explain pnl [--session REF]`                   | Decompose a stopped session's P&L and attribute decisions |
| `kraken lab new <NAME> [EXPERIMENT OPTIONS...]`        | Freeze a hypothesis, criteria, and session plan           |
| `kraken lab show <NAME>`                               | Verify and show a sealed experiment                       |
| `kraken lab next <NAME>`                               | Return the next planned session command                   |
| `kraken lab score [--session REF] [--experiment NAME]` | Score a stopped session mechanically                      |
| `kraken lab curve [--session REF]`                     | Emit the marked equity curve point by point               |
| `kraken lab compare <NAME>`                            | Compare every planned session, including named failures   |

`session state` is operational; decisions, fills, marks, and sealed criteria
remain the evidence used for replay and scoring.

### Feedback

| Command                                                                                | Description                                                                                                                                                                           |
| -------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `kraken feedback <TYPE> (--input-string STR \| --input-file PATH) [--llm-model MODEL]` | Submit product feedback (`feature-request`, `friction`, or `bug`). Uploads to Kraken over HTTPS. Do not include secrets or PII. [Service groups](#service-groups) lists what is sent. |

### Utility

| Command                                        | Description                               |
| ---------------------------------------------- | ----------------------------------------- |
| `kraken setup`                                 | Interactive setup wizard                  |
| `kraken-shell`                                 | Interactive REPL                          |
| `kraken auth login/set/show/switch/test/reset` | Browser sign-in and credential management |

</details>

## Paper Trading

Paper trading provides a safe sandbox for testing trading logic against live
Kraken prices. No API keys, no account, no real money. Two independent paper
engines cover spot and futures.

### Spot Paper Trading

The command interface has near-parity with live spot trading. Switch between
paper and live by changing the prefix: `kraken paper buy` vs `kraken order buy`.
`paper buy` defaults to a market order and `order buy` to a limit order, so pass
`--type` when you switch. Paper supports `market` and `limit` order types, a
configurable flat slippage model, and no partial fills.

**Agent pattern:**

```bash
kraken workspace create sandbox --capital 10000 --mode paper -o json
export KRAKEN_WORKSPACE=sandbox
kraken paper buy BTCUSD 0.01 -o json
kraken paper sell BTCUSD 0.005 --type limit --price 70000 -o json
kraken workspace status -o json
kraken workspace reset --yes -o json
```

**Human pattern:**

```bash
kraken workspace create sandbox --capital 5000 --currency EUR --mode paper
export KRAKEN_WORKSPACE=sandbox
kraken paper buy BTCEUR 0.01
kraken paper balance
kraken paper status
kraken paper orders
kraken paper history
kraken workspace reset
```

| Command                                                                                               | Description                                                     |
| ----------------------------------------------------------------------------------------------------- | --------------------------------------------------------------- |
| `kraken paper init [NAME] [--capital 10000] [--currency USD] [--fee-rate 0.0026] [--slippage-rate 0]` | Compatibility alias that creates the selected paper workspace   |
| `kraken paper reset [NAME] [--capital N] [--currency C] [--fee-rate R] [--slippage-rate S] [--yes]`   | Compatibility alias that reconciles pending orders, then resets |
| `kraken paper balance`                                                                                | All balances (total, reserved, available)                       |
| `kraken paper buy <PAIR> <VOL> [--type limit --price P]`                                              | Paper buy (market or limit)                                     |
| `kraken paper sell <PAIR> <VOL> [--type limit --price P]`                                             | Paper sell (market or limit)                                    |
| `kraken paper orders`                                                                                 | Open limit orders                                               |
| `kraken paper cancel <ORDER_ID>`                                                                      | Cancel a limit order                                            |
| `kraken paper cancel-all`                                                                             | Cancel all limit orders                                         |
| `kraken paper history`                                                                                | Filled trade history                                            |
| `kraken paper status`                                                                                 | Portfolio value, unrealized P&L, trade count                    |

Prices come from the public Kraken Ticker API (no auth needed). A 0.26% taker
fee (Kraken Starter tier default) is applied to all fills. Optional slippage
simulation applies a flat rate to market orders: buys fill at
`ask * (1 + slippage_rate)`, sells at `bid * (1 - slippage_rate)`. Default
slippage is 0.0 (no simulation). A limit order always rests when placed, even
when the market is already through its price. The next `paper buy`, `sell`,
`orders`, `cancel`, `cancel-all`, `history` or `status` checks it against the
live price, and once the market has crossed the limit it fills at the limit
price, not at the better market price.

In table mode, `paper buy`, `sell`, `cancel`, `cancel-all` and `status` lead
with a `[PAPER] Simulated Trading` row, and `paper orders` and `history` put
`[PAPER]` in their first column header. Their JSON carries `"mode": "paper"`, as
does the JSON of `paper balance` and `paper init`. `paper init` shows a plain
`Mode` row; the `paper balance` table and all `paper reset` output carry no
marker.

### Futures Paper Trading

Near-parity with live `kraken futures order` commands. Supports all 8 order
types (market, limit, post, stop, take-profit, ioc, trailing-stop, fok),
leverage and margin tracking, position aggregation/netting, liquidation
simulation, and funding rate accrual. Known differences: `order-status` accepts
a single ID (live accepts multiple), post-only orders are cancelled rather than
simulating maker queue priority, and fills use the bid/ask snapshot with no
depth-based slippage. Only IOC orders fill partially: they take the book depth
available at the limit price and cancel the rest.

The live commands differ in more than the prefix. `futures paper buy`/`sell`
become `futures order buy`/`sell`, which take no `--leverage` (set it with
`futures set-leverage`), and `futures paper orders` becomes
`futures open-orders`. `edit-order`, `cancel`, `cancel-all`, `batch-order`,
`order-status`, `positions`, `fills`, `leverage` and `set-leverage` keep their
names under `futures`. `init`, `reset`, `balance` and `status` are paper-only;
`futures accounts` shows the live wallet. Live `futures history` is public trade
history; your own past orders and fills are in `futures history-orders` and
`futures history-executions`.

Futures paper state is global, not per workspace. Run it with no workspace
selected (`unset KRAKEN_WORKSPACE` if an earlier example exported it): inside a
workspace only `balance`, `status`, `orders`, `order-status`, `positions` and
`fills` run; `history`, `leverage` and every mutation are refused.

Successful JSON responses include `"mode": "futures_paper"`. Most table views
are labeled `[FUTURES PAPER]` or `[FP]`.

**Agent pattern:**

```bash
kraken futures paper init --balance 10000 -o json
kraken futures paper buy PF_XBTUSD 1 --leverage 10 --type market -o json
kraken futures paper sell PF_ETHUSD 5 --leverage 20 --type market -o json
kraken futures paper positions -o json
kraken futures paper status -o json
kraken futures paper reset -o json
```

**Human pattern:**

```bash
kraken futures paper init --balance 10000 --currency USD
kraken futures paper buy PF_XBTUSD 1 --leverage 10 --type market
kraken futures paper sell PF_XBTUSD 1 --leverage 10 --type limit --price 70000
kraken futures paper positions
kraken futures paper balance
kraken futures paper fills
kraken futures paper history
kraken futures paper reset
```

| Command                                                                                    | Description                                                    |
| ------------------------------------------------------------------------------------------ | -------------------------------------------------------------- |
| `kraken futures paper init [--balance B] [--currency C] [--fee-rate R]`                    | Initialize futures paper account                               |
| `kraken futures paper reset [--balance B] [--currency C] [--fee-rate R]`                   | Reset account (clear positions, orders, fills)                 |
| `kraken futures paper balance`                                                             | Collateral, used margin, available margin, unrealized P&L      |
| `kraken futures paper status`                                                              | Equity, P&L since start, unrealized P&L, position/order counts |
| `kraken futures paper buy <SYM> <SIZE> [--leverage L] [--type TYPE] ...`                   | Paper long (all 8 order types)                                 |
| `kraken futures paper sell <SYM> <SIZE> [--leverage L] [--type TYPE] ...`                  | Paper short (all 8 order types)                                |
| `kraken futures paper orders`                                                              | Open resting orders                                            |
| `kraken futures paper order-status <ID>`                                                   | Query specific order                                           |
| `kraken futures paper edit-order --order-id <ID> [--size S] [--price P] [--stop-price SP]` | Edit a resting order                                           |
| `kraken futures paper cancel --order-id <ID>`                                              | Cancel order (also supports `--cli-ord-id`)                    |
| `kraken futures paper cancel-all [--symbol SYM]`                                           | Cancel all (optional symbol filter)                            |
| `kraken futures paper batch-order '<JSON>'`                                                | Batch order placement                                          |
| `kraken futures paper positions`                                                           | Open positions with mark price, liquidation, P&L               |
| `kraken futures paper fills`                                                               | Fill history                                                   |
| `kraken futures paper history`                                                             | Closed positions and cancelled orders                          |
| `kraken futures paper leverage`                                                            | Current leverage preferences                                   |
| `kraken futures paper set-leverage <SYM> <LEV>`                                            | Set default leverage per symbol                                |

Prices come from the public Kraken Futures Ticker API (no auth needed). Taker
fees are simulated. Positions track leverage, margin, and liquidation prices.
Funding rates are accrued based on the public funding rate API. Spot paper and
futures paper are fully independent: resetting one does not affect the other.

## Examples

Quick one-liners below. For guided walkthroughs of the bigger features — the
playground, paper workspaces, market recording, session replay with P&L explain,
the autoresearch lab, and driving all of it with agent skills — see
[examples/](examples/README.md). None of them need a Kraken account.

### Conditional order based on live price

```bash
PRICE=$(kraken ticker BTCUSD -o json | jq -r '.[].last_price')
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

Ships with 59 agent skills covering trading strategies, market analysis, and
portfolio management. See the full [Skills Index](skills/INDEX.md).

<details>
<summary>Troubleshooting</summary>

**`command not found: kraken` after install**

The installer puts the binaries in `~/.cargo/bin` (or `$CARGO_HOME/bin`). Open a
new terminal, or restart your agent. To run it right now, use the full path:
`~/.cargo/bin/kraken --version` (or `$CARGO_HOME/bin/kraken --version`).

**macOS blocks the binary ("Apple could not verify")**

This only affects binaries downloaded with a web browser. The one-liner install
is not affected. In the folder where you unpacked the download, remove the
quarantine flag:

```bash
xattr -d com.apple.quarantine kraken kraken-mcp kraken-shell
```

**"EAPI:Invalid key" or authentication errors**

Check that `KRAKEN_API_KEY` and `KRAKEN_API_SECRET` are set correctly. Keys are
case-sensitive. Verify the API key has the required permissions for the command
(Query Funds for `balance`, Modify Orders for `order buy`, etc.).

**Rate limit errors**

The CLI does not pre-throttle or retry rate-limited requests. The Kraken API
server enforces rate limits, and the CLI returns the error immediately with a
`suggestion` field containing tier-specific limits and a `docs_url` pointing to
the relevant Kraken documentation. Read the suggestion and adjust request
frequency. For sustained high-frequency use, prefer WebSocket streaming over
REST polling.

**"Failed to connect" or "No tools found" in MCP client**

The client cannot find `kraken-mcp` on its PATH. Quit the client and start it
again from a new terminal window, or set `command` to the absolute path that
`command -v kraken-mcp` prints in a new terminal. Most clients start the server
without a shell, so `~` and `$CARGO_HOME` stay unexpanded. If `kraken-mcp` is
missing, run the one-liner again: older releases installed `kraken` only. Run
`~/.cargo/bin/kraken-mcp -s market` (or the `$CARGO_HOME` path) in a terminal to
confirm the server starts; it prints one `server_start` line, then waits for a
client. Stop it with Ctrl-C. Check that your client configuration matches the
JSON format in the [MCP Server](#mcp-server) section.

**Spot paper trading state errors**

Run `kraken paper reset` to clear and reinitialize.

**Futures paper trading state errors**

Run `kraken futures paper reset` to clear and reinitialize. Spot and futures
paper states are independent.

</details>

<details>
<summary>Architecture</summary>

The repository is a Cargo workspace. The three binaries ship from the
`kraken-cli` package; every front-end is a thin adapter over `kraken-app`.

| Crate              | Role                                                                      |
| ------------------ | ------------------------------------------------------------------------- |
| `kraken-cli`       | The `kraken` binary, and the package that ships all three                 |
| `kraken-mcp`       | The MCP server: tool discovery, gating, and audit                         |
| `kraken-shell`     | The `kraken-shell` REPL                                                   |
| `kraken-app`       | The engine behind every command                                           |
| `kraken-contract`  | The command tree, shared by every front-end                               |
| `kraken-auth`      | Stored keys, the active key, and browser sign-in                          |
| `kraken-rest`      | Every Spot and Futures REST endpoint as a type, over one signed transport |
| `kraken-core`      | Typed wire model and error taxonomy for Spot WebSocket v2                 |
| `kraken-ws`        | Spot WebSocket v2 client: connection lifecycle and reconnects             |
| `kraken-paper`     | Event-sourced spot paper-trading engine                                   |
| `kraken-workspace` | Isolated account scopes for paper and live                                |
| `kraken-session`   | Session contract and timeline read model                                  |
| `kraken-recording` | Market data recording, JSONL and DuckDB backends                          |
| `kraken-replay`    | P&L decomposition and paced playback                                      |
| `kraken-lab`       | Autoresearch Lab read models: equity curve, scorecards                    |
| `kraken-recipe`    | Session-derived recipes                                                   |

</details>

## Development

```bash
cargo build                                            # dev build
cargo test                                             # run tests
cargo clippy --workspace --all-targets -- -D warnings  # lint
```

## Contributing

Bug reports, feature requests, and pull requests are welcome. See
[CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License & Disclaimer

MIT. See [LICENSE](LICENSE).

This is experimental software. Commands interact with the live Kraken exchange
and can result in real financial transactions. Orders, withdrawals, and
transfers are irreversible once processed. The authors accept no liability for
financial losses. See [DISCLAIMER.md](DISCLAIMER.md) for full terms.
