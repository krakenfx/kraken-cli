# kraken-cli Runtime Context for AI Agents

**This is experimental software. Commands interact with the live Kraken exchange
and can result in real financial transactions. The user who deploys this tool is
responsible for all outcomes. Test with `kraken paper` (spot) or
`kraken futures paper` (futures) before using real funds. See `DISCLAIMER.md`
for full terms.**

This file is optimized for runtime agent use. It defines how to call `kraken`
safely and reliably.

## Core Invocation Contract

Always call:

```bash
kraken <command> [args...] -o json 2>/dev/null
```

Rules:

- Always pass `-o json`.
- Treat `stdout` as the only machine data channel.
- Treat `stderr` as diagnostics only (structured JSON logs plus a few plain-text
  notes; off by default bar warnings, `-v`/`RUST_LOG` for more).
- Exit code `0` means success.
- Exit code `1` means runtime failure and `stdout` contains a JSON error
  envelope.
- Exit code `2` means the parser rejected the arguments (unknown flag, value
  outside a documented set). With `-o json`, `stdout` carries a
  `{"error":"validation"}` envelope and `stderr` is empty; without it, usage
  text goes to `stderr`. Check values against each command's `input_schema` in
  `agents/tool-catalog.json` before invoking.

## Authentication

For private endpoints:

```bash
export KRAKEN_API_KEY="your-key"
export KRAKEN_API_SECRET="your-secret"
```

Optional futures credentials:

```bash
export KRAKEN_FUTURES_API_KEY="your-futures-key"
export KRAKEN_FUTURES_API_SECRET="your-futures-secret"
```

Public market data, spot paper trading (`kraken paper`), and futures paper
trading (`kraken futures paper`) do not require credentials.

## Asset Classes

The CLI trades six asset classes through the same command interface. Pass
`--asset-class` where required.

| Asset class                       | Flag                            | Pair format                | Example                                                                                 |
| --------------------------------- | ------------------------------- | -------------------------- | --------------------------------------------------------------------------------------- |
| Crypto spot                       | _(default, no flag)_            | `BTCUSD`, `BTC/USD`        | `kraken order buy BTCUSD 0.001 --type limit --price 50000`                              |
| Tokenized stocks & ETFs (xStocks) | `--asset-class tokenized_asset` | `AAPLx/USD`, `TSLAx/USD`   | `kraken order buy AAPLx/USD 0.1 --type limit --price 200 --asset-class tokenized_asset` |
| Forex                             | `--asset-class forex`           | `EURUSD`, `GBPUSD`         | `kraken ticker EURUSD --asset-class forex`                                              |
| Perpetual futures                 | _(futures engine)_              | `PF_XBTUSD`, `PF_AAPLXUSD` | `kraken futures order buy PF_XBTUSD 1 --type limit --price 50000`                       |
| Inverse & fixed-date futures      | _(futures engine)_              | `FI_XBTUSD_<YYMMDD>`       | `kraken futures order buy FI_XBTUSD_<YYMMDD> 1 --type limit --price 50000`              |
| Earn / staking                    | —                               | —                          | `kraken earn strategies --asset ETH`                                                    |

xStocks use the `x` suffix on the ticker symbol (AAPL becomes `AAPLx`, TSLA
becomes `TSLAx`). The `--asset-class tokenized_asset` flag is required on trade,
market data, and funding commands. xStocks are not available in the USA.

Futures include 200+ perpetuals: crypto, 3 forex perps (PF_EURUSD, PF_GBPUSD,
PF_CHFUSD), xStocks perps (PF_AAPLXUSD, PF_NVDAXUSD, PF_TSLAXUSD, PF_SPYXUSD,
PF_QQQXUSD, and others), equity and index perps (PF_AMDUSD, PF_PLTRUSD,
PF_US100USD, and others), commodities, and pre-IPO contracts. PF_SPXUSD is the
SPX6900 meme coin, not the S&P 500. Use `kraken futures instruments -o json` to
list all available contracts.

## Safety Rules

1. Never place live orders or withdrawals without explicit human approval.
2. Prefer `kraken paper ...` (spot) or `kraken futures paper ...` (futures) for
   strategy testing.
3. Validate orders before execution with `--validate` (`order buy`,
   `order sell`, `order batch`, `ws add-order`, `ws batch-add`). Futures orders,
   amends, edits and cancels have no dry run.
4. Use `cancel-after` for unattended sessions.
5. Never log or echo API secrets.

The authoritative list of dangerous commands is in `agents/tool-catalog.json`
(every entry with `"dangerous": true`). Common examples:

- `kraken order buy/sell` and all batch/amend/edit order variants
- `kraken futures order buy/sell` and futures cancel/batch/edit variants
- `kraken withdraw`, `kraken wallet-transfer`
- `kraken order cancel-all`, `kraken futures cancel-all`
- `kraken earn allocate/deallocate`
- `kraken subaccount transfer`, `kraken subaccount create`
- `kraken futures set-leverage`, `kraken futures set-pnl-preference` (margin and
  payout settings)
- All WebSocket order mutations (`ws add-order`, `ws amend-order`,
  `ws cancel-order`, `ws cancel-all`, `ws cancel-after`, `ws batch-add`,
  `ws batch-cancel`)

This list is non-exhaustive. Always check the `dangerous` field in
`tool-catalog.json`. Also gate the two `effect: local_or_live` commands when
their live argument is set — `workspace create --mode live` and
`recipe run --real-account` reach the real account, though their ordinary shape
is local.

## Error Handling Contract

On failure, parse:

```json
{ "error": "<category>", "message": "<detail>" }
```

Route on `error`, not on `message`.

Common categories:

- `auth`: re-authenticate
- `rate_limit`: read `suggestion` and `docs_url` fields, adapt strategy
- `network`: retry with exponential backoff, unless `verify_first` is `true`:
  then the order or transfer may have executed, so check its state first
- `validation`: fix inputs, do not retry unchanged request
- `api`: inspect request and parameters

For full envelopes and retry guidance: `agents/error-catalog.json`.

## Feedback

If you hit a mismatch between what you are trying to do and the CLI's interface
or responses — including a skill/contract mismatch — submit feedback:

```bash
kraken feedback bug --input-string "..." --llm-model <your-model> -o json
```

Types: `feature-request`, `friction`, `bug`. Soft body recommendations are in
`kraken feedback --help`. Submitting uploads feedback to Kraken over HTTPS. Do
not include API keys, secrets, or personal data. Kraken keeps feedback for 90
days.

## Context Window Efficiency

Prefer narrower responses:

- Use limits like `--count`, `--depth`, `--offset`, `--limit` where available.
- Prefer pair-scoped calls over broad list calls.
- Prefer WebSocket JSONL for streaming instead of high-frequency polling.

## High-Value Patterns

Public price read:

```bash
kraken ticker BTCUSD -o json
```

Safe order flow:

```bash
kraken order buy BTCUSD 0.001 --type limit --price 50000 --validate -o json
# ask for human approval
kraken order buy BTCUSD 0.001 --type limit --price 50000 -o json
```

Spot paper trading loop:

```bash
kraken workspace create sandbox --capital 10000 --mode paper --slippage-rate 0.001 -o json
export KRAKEN_WORKSPACE=sandbox
kraken paper buy BTCUSD 0.01 -o json
kraken workspace status -o json
```

Capital, fee rate (default 0.26%) and slippage rate (default 0.0) are set at
`workspace create`; a confirmed `workspace reset` can change them with
`--capital`, `--currency`, `--fee-rate` and `--slippage-rate`.
`paper init/reset/status/balance` are aliases of the workspace commands.

Futures paper trading loop (it runs outside a workspace and refuses everything
but its reads while `KRAKEN_WORKSPACE` is set):

```bash
unset KRAKEN_WORKSPACE
kraken futures paper init --balance 10000 -o json
kraken futures paper buy PF_XBTUSD 1 --leverage 10 --type market -o json
kraken futures paper positions -o json
kraken futures paper status -o json
```

Market memory capture (no auth):

```bash
kraken record --symbols BTC/USD --channels trades,book --tape my-tape -o json
# Ctrl-C / SIGTERM finalizes the tape: window_end + integrity counters land in its metadata
kraken tape list -o json
```

Tapes live in the shared library (`tapes/` under the config dir); one writer per
tape — a concurrent recorder gets a `validation` envelope. Replay a finalized
tape into a paper session with
`kraken session start --from tape:my-tape --speed 10`. Add `--monitor` to
`ws`/`streamd`/`record` for stderr JSONL health lines (latency, trade gaps,
staleness), routable by `type`. On a mid-stream reconnect, streaming stdout
carries one `{"endpoint":"public","snapshot_follows":true,"type":"reconnected"}`
marker line — discard accumulated state for that endpoint; the next frames are
full snapshots. Markers never land in recorded tapes. Spot streams reconnect
until interrupted; `futures ws` stops after 12 consecutive failed reconnects and
emits no marker.

Recipe loop (no auth):

```bash
kraken recipe create btc-levels --from-session latest -o json  # package a stopped paper session
kraken recipe run btc-levels --for 15m                          # provision its workspace + rerun; streams like session start
kraken recipe list -o json                                      # global library; ships with a built-in 'playground'
```

A run is an ordinary session: `session show` reads back the strategy params,
`lab curve` emits the full marked equity series, `lab score`/`explain pnl` grade
it. `recipe create` requires a paper workspace scope;
`recipe run --real-account` is a confirmed opt-in and refuses `--from`.
`kraken plugin init <name> --lang python|bash` scaffolds a JSON-contract
extension (`extension.json` + entrypoint + SKILL.md) that the CLI never
executes.

## MCP Server

For MCP clients that start local stdio servers (Claude Desktop, Codex, Gemini
CLI, Cursor, VS Code, Windsurf), use the built-in MCP server:

```bash
kraken-mcp -s market,account,paper,workspace,feedback,futures-paper
```

This exposes CLI commands as structured MCP tools over stdio. No subprocess
wrappers needed. Most clients take it as an `mcpServers` JSON entry. VS Code
uses a top-level `"servers"` key in `.vscode/mcp.json`, and Codex reads a
`[mcp_servers.kraken]` table in `~/.codex/config.toml`.

Security note:

- MCP is local-only and should run on your own machine.
- Agents connected to the same MCP server share the same configured Kraken
  account permissions.
- Do not expose or share this server outside systems you control.
- Always use `https://` and `wss://` endpoints.

Streaming commands (`session start`, bare `kraken playground`, `record`,
`replay`, `streamd`, `ws ping`) are CLI-only and never appear as MCP tools.
Every other session verb (order, note, state, stop, show, score, explain) is a
tool; opening a session takes a shell.

## Tool Discovery

Use these machine-readable files:

- `agents/tool-catalog.json`: full command catalog (188 commands with generated
  argument schemas and `dangerous` flags)
- `agents/error-catalog.json`: error categories and retry policy
