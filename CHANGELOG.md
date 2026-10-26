# Changelog

User-visible changes are recorded here. kraken-cli follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

The MCP server no longer reports a real service group as unknown.

`-s futures-paper` returned `Unknown service: 'futures-paper'` even though
`futures paper` is a real CLI service group with 17 REST commands and no
credential requirement. The group now parses and is filtered like the streaming
groups, and the server names it and the reason instead of blaming the caller. A
mixed `-s` list that includes it no longer fails; the server warns on stderr
that the group contributes no tools.

`agents/mcp-coverage.json` records every catalog command MCP does not expose,
with a reason each. It is pinned by a test in `src/mcp/registry.rs`, so a rename
or a newly registered group fails the suite instead of silently changing what
agents can reach.

The default service list is now stated correctly across the docs: the CLI
defaults to `market,account,paper,workspace,feedback`, while marketplace
listings ship the narrower `market,paper,feedback`.

The paper-backtest skill now states that limit orders pay the configured
`fee_rate` too, so a strategy resting limit orders is charged more on paper
than it would pay live.

**Futures paper is exposed as MCP tools.** The 17 `futures paper` commands were a
real CLI service group with no credential requirement, yet absent from the MCP
registry — `-s futures-paper` returned `Unknown service`. They are now tools, and
their state is scoped per workspace like spot paper, so MCP clients no longer
share one global futures paper account. `futures paper reset` is now flagged
`dangerous`, matching `paper reset` and `workspace reset`.

**The config directory honors `XDG_CONFIG_HOME` on every platform.** On Windows
the `dirs` crate resolves `config_dir()` through `SHGetKnownFolderPath`, which
ignores `HOME` and `XDG_CONFIG_HOME` — so callers that set those to isolate state,
including the integration tests, were silently redirected into the real user
config directory. An interrupted run then left workspaces and a stale tape lock
behind, which broke every later run.

**The Windows process-liveness test is fixed.** It asserted that an impossible
PID was not alive, but the Windows `process_alive` deliberately reports alive for
any PID (there is no portable probe). The assertion is now unix-only, so the suite
can pass on Windows.

## [0.4.1] - 2026-08-07

Patch release: financial detail in the MCP audit stream.

The per-invocation audit records the MCP server writes to stderr now
identify the financial context of every call. `tool_call` and
`tool_result` lines carry the trading mode — paper or live, re-resolved
on every call so a mid-session workspace promotion is visible — and
`tool_call` lines add the values of allowlisted financial arguments
(amounts, assets, pairs, prices). Values outside the allowlist, such as
credentials and free-form text, never reach the log.

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
cursor into the evidence. Sessions distinguish recording, stopped, aborted,
and damaged states; repeated stops return the existing result.

**Tapes** store WebSocket data in DuckDB, JSONL, or both. They carry symbols,
channels, time bounds, integrity counters, and completion state. SIGINT and
SIGTERM finalize every backend. Concurrent writers are refused, readers can
coexist with a writer, and finalized tapes can be appended or replayed.
Incomplete captures cannot masquerade as replayable evidence.

`--monitor` adds JSONL diagnostics on stderr for latency, throughput, gaps,
staleness, heartbeats, reconnects, dropped events, and parse failures. Stdout
remains reserved for command data.

**Lab experiments** seal a hypothesis, thresholds, and optional live/replay
plan before results exist. The lab verifies the seal, identifies the next run,
checks tape hashes, scores stopped sessions, and compares every planned result.
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
Pair restrictions are enforced before price lookup or journal mutation.
Damaged manifests, journals, tapes, and seals fail closed. Destructive resets
require confirmation, and futures-paper mutations are refused inside a
workspace because that simulator still has one global account.

Release archives are built with cargo-dist for Apple Silicon and Intel macOS
plus ARM64 and x86-64 Linux. Archives include SHA-256 checksums, provenance
attestations, and Minisign signatures.

### Changes requiring migration

- `paper init`, `paper reset`, `paper status`, and `paper balance` remain as
  compatibility aliases, but lifecycle responses now use the workspace
  contract. Amounts are decimal strings.
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
5. Select paper accounts with `--workspace <NAME>` or
   `KRAKEN_WORKSPACE=<NAME>`. Empty, omitted, and `default` select the flat
   real-account layout.
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

[0.4.0]: https://github.com/krakenfx/kraken-cli/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/krakenfx/kraken-cli/releases/tag/v0.3.2
