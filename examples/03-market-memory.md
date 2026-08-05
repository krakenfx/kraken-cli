# Market memory

Every stream the CLI touches can be captured to clean local storage. Over time you build a private dataset of real market conditions: raw material for backtests, dashboards, and the replayable experiments in later walkthroughs.

## Record a window

```bash
kraken record --symbols BTC/USD,ETH/USD --channels ticker,trade --to duckdb --tape morning-session
```

Let it run for as long as you want the window to be, then Ctrl-C. The recorder finalizes the tape and prints a summary to stderr naming the files it wrote.

- `--to` takes `duckdb`, `jsonl`, or both (`duckdb,jsonl`). DuckDB gives you SQL over the tape; JSONL gives you a plain-text file you can inspect with anything.
- `--channels` covers the public feed: `ticker`, `trade`, `book`, `ohlc`, `instrument`. Book depth and OHLC interval are tunable (`--book-depth 25`, `--ohlc-interval 5`).
- Tapes land in the global tape library under your config dir, shared across all workspaces.

## See what you have

```bash
kraken tape list
```

Lists every tape in the library and every session-scoped recording, with its backend, state, symbols, and whether something is using it right now. A finalized tape is addressable as `tape:<name>` anywhere the CLI accepts a replay source.

## Query it

A DuckDB tape is just a database file in the tape library — `~/Library/Application Support/kraken/tapes/` on macOS, `~/.config/kraken/tapes/` on Linux:

```bash
duckdb "$HOME/Library/Application Support/kraken/tapes/morning-session.duckdb" \
  "SELECT symbol, count(*) AS executions FROM trades GROUP BY symbol"
```

## Stream without recording

When you want the live feed itself, `streamd` multiplexes any mix of symbols and channels to stdout as JSONL:

```bash
kraken streamd start --symbols BTC/USD,ETH/USD,SOL/USD --channels ticker,trade
```

## Trust the tape

A gap you can't see is a backtest you can't trust. Add `--monitor` to any streaming command (`ws`, `streamd`, `record`) and the CLI emits health advisories to stderr while the data flows on stdout:

```bash
kraken record --symbols BTC/USD --channels ticker,trade --to duckdb --tape audited --monitor
```

You get periodic health summaries, latency and gap detection, stale-stream alerts, and heartbeat timeouts, without any of it polluting the recorded data. Finalized recordings carry their own integrity counters, and the [autoresearch lab](05-autoresearch-lab.md) seals a content hash of every tape an experiment depends on.

## Hand it to an agent

Streams and recordings are plain JSONL and flags, so agents drive them directly. The [`kraken-ws-streaming`](../skills/kraken-ws-streaming/SKILL.md) skill covers the streaming surface, and watcher recipes like [`recipe-multi-pair-breakout-watch`](../skills/recipe-multi-pair-breakout-watch/SKILL.md) and [`recipe-track-orderbook-depth`](../skills/recipe-track-orderbook-depth/SKILL.md) build on it ([install](06-agent-skills.md)):

> *"Record BTC and ETH ticker and trades for ten minutes as a tape called `tuesday-open`, keep an eye on stream health, and tell me if anything drops."*

Next: recording your own trading, not just the market — [sessions, replay, and P&L explain](04-sessions-replay-explain.md).
