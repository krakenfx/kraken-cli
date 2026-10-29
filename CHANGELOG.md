# Changelog

User-visible changes are recorded here. kraken-cli follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.1] - 2026-10-29

Patch release: documentation. Commands, flags and output are unchanged.

- The README walks from install to sign-in to a first real order: market data,
  paper practice, `kraken auth login`, an account check, a `--validate` preview,
  then the order. It shows how to size a market buy in quote currency with
  `--oflags viqc`, and how to confirm the fill with `query-orders`.
- A section for agents trading real money: validate first, confirm with the
  human, use a fresh `--cl-ord-id`, and look a write up before sending it again
  after a `network` error. The `network` entry in `agents/error-catalog.json`
  names the lookup for each kind of write, Earn moves included, and the Earn
  skill records the allocation before each move so that a timed-out call can be
  checked.
- Market counts in the README and agent docs match the live listings: 170+
  xStocks, 12 forex pairs, and 200+ perpetuals with up to 100x on BTC, ETH and
  SOL. `PF_SPXUSD` is the SPX6900 token, not the S&P 500 index.
- The README, `CONTEXT.md` and the MCP skill show the Codex (TOML) and VS Code
  (`servers` key) MCP configs, and no longer list ChatGPT among the clients that
  start a local server.
- The MCP docs and the setup skills warn that the `feedback` tool can upload a
  local file it names, with no approval step. Drop `feedback` from `-s` for an
  agent that reads untrusted input.
- Smaller corrections across the auth, MCP and streaming docs, such as
  `auth reset` deleting every stored key and `record` capturing market-data
  channels only.

## [0.5.0] - 2026-10-27

Sign in from the browser, run the MCP server and the REPL as their own binaries,
take a paper strategy live, and package a finished session as a recipe. The
catalog lists 188 commands in 15 groups; `kraken-mcp` serves 152 of them as MCP
tools. The project is still alpha: test with paper trading first.

### Upgrading from 0.4.1

These changes need action from scripts, agents or MCP clients.

- **The MCP server and the REPL are separate binaries.** Use `kraken-mcp`
  instead of `kraken mcp` and `kraken-shell` instead of `kraken shell`. Run the
  installer again before you update an agent plugin: the plugins now start
  `kraken-mcp`. In a client config, `-s` and `--allow-dangerous` work as before;
  pass API keys as `KRAKEN_API_KEY` and `KRAKEN_API_SECRET` in the client's
  environment. After a source install, run
  `cargo uninstall kraken-mcp kraken-shell` before installing `kraken-cli`.
- **REST output carries the fields the CLI models.** Fields Kraken adds are no
  longer passed through. `query-orders` returns the same order shape as
  `open-orders`, `query-trades` the same fill shape as `trades-history`, and
  `auth test` reports `balances` like `balance`. JSON objects keep the server's
  order instead of sorting keys.
- **MCP tool calls.** A dangerous call goes to the operator through MCP
  elicitation; `acknowledged: true` applies only to clients that cannot show
  one. Results larger than 256 KiB are refused with the arguments that narrow
  them, results over 16 KiB come back as `structuredContent` only, and a result
  that is not a JSON object arrives as `{"result": ...}`.
- **Catalog schema 2.0.0.** `parameters` became `input_schema` (JSON Schema),
  `auth_required` became `credentials`, `global_flags` is gone, and positional
  arguments moved from `command` to `usage`. New fields: `effect`, `reach`,
  `tool`, `why` and `constraints`.
- **`kraken record` writes JSONL by default.** It wrote DuckDB before. Pass
  `--to duckdb` (or `--to duckdb,jsonl`) to keep appending to a 0.4.x DuckDB
  tape. Release binaries read and write DuckDB; a source build needs
  `--features record-duckdb` to do either.
- **Futures paper accounts.** Open orders saved by 0.4.1 use the old type
  spelling and are not migrated: delete `<config-dir>/paper/futures_state.json`
  and run `kraken futures paper init`.
- **The Claude Code plugin no longer asks for an API key.** Its tools need none,
  and it no longer passes a key saved under 0.4.x to `kraken-mcp`; `kraken-mcp`
  still reads the active key stored by `kraken auth set` or `kraken auth login`.
- **`kraken feedback` keeps no local copy.** Its output no longer has `id` and
  `path`; you can delete `<config-dir>/feedback.duckdb`.

### New commands

| Command                                 | What it does                                                      |
| --------------------------------------- | ----------------------------------------------------------------- |
| `kraken auth login`                     | Sign in in the browser; saves a restricted, expiring Spot API key |
| `kraken auth switch api-key\|sign-in`   | Choose which stored key signs requests                            |
| `kraken recipe create\|list\|show\|run` | Package a stopped paper session and run it again                  |
| `kraken plugin init\|list`              | Scaffold a dependency-free extension over the CLI's JSON output   |
| `kraken lab curve`                      | The full marked equity series behind `lab score`                  |
| `kraken-mcp`, `kraken-shell`            | The MCP server and the REPL, installed next to `kraken`           |

`auth login` needs a terminal, so a script or an agent cannot create a key. The
key has no withdrawal rights and expires after 90 days unless `--valid-for` says
less; `--read-only` creates a key that cannot trade, and `--no-browser` only
prints the sign-in URL. `auth reset` deletes the local copy only: delete the key
in the Kraken UI to revoke it at once. An older release ignores the sign-in key
and can drop it from `config.toml`, so delete it in the Kraken UI before you
downgrade.

### Changed commands

- `workspace create --mode live` and `workspace promote` now work. A live
  workspace is a budget-fenced allocation of the real account: creation checks
  the allocation against the real balance, orders print a live banner on stderr,
  and venue fills are reconciled by transaction ID.
- `ticker --all` returns every tradable pair, `ticker --watch <SECONDS>` streams
  snapshots as NDJSON, and one call can mix asset classes.
- Orders are checked before they reach Kraken: a Spot `*-limit` order needs
  `price2`, a futures order takes only the prices its type uses, and sizes and
  prices must be decimals.
- `auth show` (alias `auth status`) names the key in use and says when a flag or
  an environment variable overrides it.
- `kraken setup` without a terminal fails with a `validation` error that names
  `KRAKEN_API_KEY` and `KRAKEN_API_SECRET`. `setup` and `auth set` refuse an API
  secret that is not valid base64.
- `session show` includes the strategy label and parameters. Streams print a
  `reconnected` event on stdout after a reconnect.
- With `-o json`, commands that write their own output no longer print a
  trailing `{}`.

### Fixed

- A timed-out order, cancel, withdrawal or transfer is never sent again. When it
  may have executed (a timeout, a broken connection, a 5xx, or an accepted reply
  that does not decode), the error envelope carries `"verify_first": true`,
  whatever its category: check open orders or history before retrying.
  ([#51](https://github.com/krakenfx/kraken-cli/pull/51), thanks @SashaMIT)
- MCP tools whose result is a JSON array, such as `kraken_deposit_status`, work
  in strict clients. ([#52](https://github.com/krakenfx/kraken-cli/issues/52))
- Futures paper: editing a trailing stop's `--stop-price` moves its trigger,
  `orders` and `order-status` show stop price and trigger signal, and stop,
  take-profit and trailing-stop orders net their margin against an opposite
  position. ([#54](https://github.com/krakenfx/kraken-cli/pull/54),
  [#55](https://github.com/krakenfx/kraken-cli/pull/55),
  [#56](https://github.com/krakenfx/kraken-cli/pull/56), thanks @theghostmac)
- Private futures requests with query parameters are signed correctly, the
  futures history commands reach the history API, and a trailing stop sends its
  deviation unit in the documented spelling.
- `order cancel-batch` and `export-delete` send the request shape the API takes.
- Futures paper computes money in `Decimal`; its state file still stores JSON
  numbers, as 0.4.1 did.
- History responses (`ledgers`, `trades-history`, `closed-orders` and the
  others) keep the server's newest-first order.
- Replay sessions are refused on the real account and on live workspaces.
- README badges link to the release, the license and the install section.
  ([#50](https://github.com/krakenfx/kraken-cli/pull/50), thanks @sashaphmn)

### Security

- Dependency updates fix RUSTSEC-2026-0258 (h2 0.4.16), RUSTSEC-2026-0285,
  -0346, -0348 and -0349 (rustls 0.23.46), RUSTSEC-2026-0382 (rustls-webpki
  0.103.16), RUSTSEC-2026-0381 and -0383 (webpki-roots and webpki-root-certs
  1.0.10), RUSTSEC-2026-0367, -0368 and -0369 (zip 9.0.5), RUSTSEC-2026-0385
  (ctutils 0.4.6) and RUSTSEC-2026-0386 (tokio 1.53.3), and replace the yanked
  chacha20 0.10.1 and clipboard-win 5.4.1.
- Report vulnerabilities privately as described in [SECURITY.md](SECURITY.md).

## [0.4.1] - 2026-08-07

Patch release: financial detail in the MCP audit stream.

The per-invocation audit records the MCP server writes to stderr now identify
the financial context of every call. `tool_call` and `tool_result` lines carry
the trading mode — paper or live, re-resolved on every call so a mid-session
workspace promotion is visible — and `tool_call` lines add the values of
allowlisted financial arguments (amounts, assets, pairs, prices). Values outside
the allowlist, such as credentials and free-form text, never reach the log.

## [0.4.0] - 2026-07-27

Version 0.4.0 adds a repeatable research loop around the trading CLI. Users can
isolate a paper account, record a market window, preserve decisions, replay the
same observations, and grade the result against criteria fixed in advance.

The release exposes 173 request/response commands in 14 groups and ships 59
agent skills. Streaming and recording remain CLI-only. The project is still
alpha software: test with paper trading and use least-privilege credentials.

### Research workflow

**Workspaces** are durable account scopes. Creation also funds the paper
account, eliminating half-created states. Each workspace records its capital,
currency, fees, slippage, and optional pair restrictions. Resets open a new
journal epoch without deleting history. Reports separate session activity,
manual trades, resets, reconciliation drift, and current equity.

The familiar spot order verbs route to simulated execution inside a paper
workspace and retain live behavior outside one. Named live workspaces are
blocked until credentials can be scoped safely.

**Sessions** are recorded windows over a workspace journal. They preserve
opening and closing valuation, market data, provenance, fills, and decision
notes. A separate typed state cell lets an agent resume without mixing its
cursor into the evidence. Sessions distinguish recording, stopped, aborted, and
damaged states; repeated stops return the existing result.

**Tapes** store WebSocket data in DuckDB, JSONL, or both. They carry symbols,
channels, time bounds, integrity counters, and completion state. SIGINT and
SIGTERM finalize every backend. Concurrent writers are refused, readers can
coexist with a writer, and finalized tapes can be appended or replayed.
Incomplete captures cannot masquerade as replayable evidence.

`--monitor` adds JSONL diagnostics on stderr for latency, throughput, gaps,
staleness, heartbeats, reconnects, dropped events, and parse failures. Stdout
remains reserved for command data.

**Lab experiments** seal a hypothesis, thresholds, and optional live/replay plan
before results exist. The lab verifies the seal, identifies the next run, checks
tape hashes, scores stopped sessions, and compares every planned result.
Failures remain visible rather than disappearing from a comparison.

Scoring covers return, drawdown, fees and execution friction, turnover, hit
rate, profit factor, trade counts, and slippage sensitivity. `explain pnl`
reconciles the stop total across price movement, fees, spread, slippage,
residual differences, missed-fill counterfactuals, and recorded decisions.

### Automation contracts

The public catalog describes parameters, allowed values, authentication,
examples, and safety annotations for all request/response commands. MCP exposes
those commands through the same parser and dispatcher as the CLI. Unknown MCP
arguments now fail with the accepted names instead of being dropped.

JSON-formatted parser failures use the stable error envelope. Rate-limit errors
include retry guidance, a retryable flag, and the relevant Kraken documentation
URL. Workspace, session, tape, experiment, and P&L request/response operations
are available through MCP; long-running streams are not.

Financial values in workspace and session contracts use exact decimal strings.
Pair restrictions are enforced before price lookup or journal mutation. Damaged
manifests, journals, tapes, and seals fail closed. Destructive resets require
confirmation, and futures-paper mutations are refused inside a workspace because
that simulator still has one global account.

Release archives are built with cargo-dist for Apple Silicon and Intel macOS
plus ARM64 and x86-64 Linux. Archives include SHA-256 checksums, provenance
attestations, and Minisign signatures.

### Changes requiring migration

- `paper init`, `paper reset`, `paper status`, and `paper balance` remain as
  compatibility aliases, but lifecycle responses now use the workspace contract.
  Amounts are decimal strings.
- `paper reset` can change capital, currency, fee, and slippage for the new
  epoch. Unattended resets require the global `--yes` flag.
- Recording vocabulary now uses **tape**. `record --dataset` remains a CLI
  alias, but output and stored metadata use `tape`.
- MCP tool `kraken_dataset_list` is replaced by `kraken_tape_list`; its replay
  reference field is `tape`, not `dataset`.
- Playground lifecycle commands moved to `session start`, `stop`, `list`,
  `show`, and `note`. Bare `kraken playground` remains the guided demo.
- `--session` and session references now accept `latest`, `s<N>`, or a unique
  label. Reusable recordings use `tape:<NAME>`; session recordings use
  `session:s<N>`.
- MCP argument names follow the CLI. Existing aliases still work, but unknown
  names are rejected.

### Upgrade from 0.3.2

1. Re-list MCP tools and refresh cached schemas.
2. Replace `kraken_dataset_list` with `kraken_tape_list`.
3. Read paper lifecycle responses as workspace contracts with decimal strings.
4. Add `--yes` to unattended resets.
5. Select paper accounts with `--workspace <NAME>` or `KRAKEN_WORKSPACE=<NAME>`.
   Empty, omitted, and `default` select the flat real-account layout.
6. Replace playground lifecycle calls with session commands.
7. Update recording references to `tape:<NAME>` or `session:s<N>`.

### Release facts

- Version: 0.4.0
- Minimum Rust version: 1.97
- Catalog: 173 request/response commands in 14 groups
- Skills: 59
- Previous release: 0.3.2

## [0.3.2] - 2026-04-20

Version 0.3.2 is the previous public release.

[Unreleased]: https://github.com/krakenfx/kraken-cli/compare/v0.5.1...HEAD
[0.5.1]: https://github.com/krakenfx/kraken-cli/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/krakenfx/kraken-cli/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/krakenfx/kraken-cli/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/krakenfx/kraken-cli/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/krakenfx/kraken-cli/releases/tag/v0.3.2
