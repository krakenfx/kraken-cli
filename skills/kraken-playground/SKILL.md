---
name: kraken-playground
version: 2.0.0
description: "Paper-trading sandbox over workspace sessions: record live market data and agent decisions inside a session window, replay and score it later."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken"]
---

# kraken-playground

A session ties a market recorder and the workspace's paper account to one window. Live prices stream in and get recorded as the session's tape, paper orders fill against those prices on the shared account journal, and every decision is logged, so the window can be replayed and analyzed later. No real order is ever placed inside a paper workspace.

Use this skill for:
- running a paper-trading hypothesis on live prices with no real money
- recording ticker, trade, and book data alongside paper fills and decision reasons
- driving an agent hypothesis over a window at a fixed cadence
- replaying a session and computing P&L after the fact

## The One-Command Demo

`kraken playground` is the fastest way in: it creates (or reuses, never re-funding) a paper workspace named `playground` with 10,000 USD, prints how to watch it, and starts a recorded, self-stopping demo session:

```bash
kraken playground --symbols BTC/USD --for 1h 2>/dev/null &
export KRAKEN_WORKSPACE=playground
```

Everything it makes is a plain workspace plus a session — the commands below drive it like any other scope.

## What a Session Is

A session is a recorded window over the active scope's account journal. One session id (`s1`, `s2`, …) binds three things: the recorder (live market data to `sessions/s<n>/tape.*`), the window markers on the account journal (which trades belong to the session), and the session's decision log (the "why" behind each action). There is no daemon and no shared memory between rounds. State lives on disk, and each round reads what it needs.

The CLI has no scheduler and runs no strategy. It records, fills paper orders, and logs. The agent decides, and something external holds the pace (see Pacing).

## Start a Session

Work inside a paper workspace (create one once with `kraken workspace create <name> --capital 10000 --mode paper`), then:

```bash
export KRAKEN_WORKSPACE=<name>

kraken session start \
  --symbols BTC/USD --channels ticker,trade --to duckdb,jsonl \
  --label my-hypothesis-r1 \
  -o json 2>/dev/null &
```

The `session_started` JSON line on stdout carries the session id (`"session":"s<n>"`). **Print it to the user as soon as the session starts, and again in the final report.** It is the handle for everything after: `kraken session show --session s<n>`, resuming a `/loop`, and locating the artifacts under `sessions/s<n>/`.

- `--symbols`: instruments to record and trade against
- `--channels`: which feeds to record (e.g., `ticker,trade`, add `book` for depth)
- `--to`: recording sinks (`duckdb` for query, `jsonl` for raw replay)
- `--label`: a human handle, usable anywhere a `--session` ref is
- `--strategy` / `--strategy-params`: name and JSON knobs of the driver, recorded in `session.json` so two sessions compare knob-for-knob
- `--for`: auto-stop after a window (e.g. `1h`) — the session closes itself cleanly
- `--from tape:<name> --speed 10`: replay a recorded tape instead of the live market

## Paper Trades

Buy or sell against recorded prices — the order verbs are mode-routed, so inside a paper workspace they fill on the paper account. Always pass a reason; it lands in the active session's decision log:

```bash
kraken order buy BTC/USD <volume> --type market --reason "<why>" -o json 2>/dev/null
kraken order sell BTC/USD <volume> --type market --reason "<why>" -o json 2>/dev/null
```

Log a non-trade decision (a skip, a gate, an alert) so the log records rounds where nothing traded:

```bash
kraken session note --kind skip --symbol BTC/USD --reason "<why>" -o json 2>/dev/null
```

## Recording

Data lands under the session directory of the active scope:

```bash
# Read the decision log (evidence) and the typed state cell (the loop's
# cursor: round, legs_done, zone, last_action_at) through the CLI —
# no layout knowledge needed:
kraken session decisions --session s<n> -o json
kraken session state get --session s<n> -o json
```

- `session.json`: the session's contract — window, opening equity anchor, strategy, experiment
- `decisions.jsonl`: one line per decision (kind, symbol, reason, timestamp, order id)
- `tape.duckdb` / `tape.jsonl`: the raw tape for replay and P&L

## Pacing

Pacing is external to the CLI. Two mechanisms, one per hypothesis shape:

- Time-gated: `/loop` owns the schedule. Set its interval to the decision spacing and it fires one round per interval.
- Event-gated: a WebSocket stream's event rate owns the schedule. The loop acts on qualifying events (see `kraken-ws-streaming`).

For time-gated runs, the interval is a floor. A round may start late (the scheduler adds jitter), never early. Do not add a second pacing gate inside the round; `/loop` already enforces spacing.

**The scheduler floor is 60 seconds.** `/loop`, cron, and `ScheduleWakeup` all clamp to a one-minute minimum, so `interval_s` must be `>= 60` and a sub-minute cadence is impossible. If the user asks for "every 30 seconds", clamp to 60s and say so — never set a sub-minute `interval_s`, and never let the report claim a cadence the harness cannot run.

## Running over a Window

A session over many rounds is interventional: one visit per interval, paced from outside. Pick the mechanism by what triggers a decision.

- "Act every N minutes on the current price": time-gated. `/loop N` fires the round, each round takes one REST snapshot (`kraken ticker`, `kraken ohlc`), decides, acts. See `recipe-playground-dca`.
- "Act the instant a condition trips": event-gated. Subscribe to a stream and act on each qualifying tick, rate-limited so you do not act faster than intended. See `recipe-playground-dca-triggered`.

Time-gated misses conditions that appear and resolve between visits. Event-gated catches them but runs as a long-lived process and is blind during reconnect gaps. Choose per hypothesis.

## Driving Your Own Hypothesis

A driver is a recipe that turns a hypothesis into recorded rounds. The shape of every round:

1. READ: pull the market state the decision needs (price, an average, spread).
2. THINK: apply the numeric gates from `--strategy-params`.
3. ACT: `order buy` / `order sell` on a trigger, `session note` on a skip, always with a numeric reason.

Keep every number that gates a buy or skip in `--strategy-params`, not only in prose reasons. That is what makes two sessions comparable. Keep the arithmetic deterministic (compute in `jq`/`bc`, do not eyeball it) so a rerun on the same tape reaches the same decisions.

## Show, Stop, and Score

```bash
kraken session show -o json 2>/dev/null | jq '.window, .valuation'
kraken session stop -o json 2>/dev/null
kraken explain pnl --session latest -o json 2>/dev/null
kraken lab score --session latest -o json 2>/dev/null
```

`--session` accepts `latest` (the default everywhere), an ordinal (`s3`), or a label.

## Hard Rules

- A session inside a paper workspace never places a live order. It records and fills paper only.
- Paper results may overstate live performance: fees and slippage are simulated and there are no partial fills (see `kraken-paper-strategy`).
- No daemon, no shared memory between rounds. State lives on disk under the workspace and its sessions.
- Pace from outside the CLI: `/loop` for time-gated, a stream's event rate for event-gated. Never add a redundant pacing gate inside a round. The scheduler floor is 60s — no sub-minute cadence.
- Always pass a reason on every trade and skip, so the decision log carries the full "why".
- The session directory (`session.json`, `decisions.jsonl`, and the tape sinks) is owned by the CLI recorder. The agent only **reads** it, and only for status and the stop-time summary. Never write, edit, `mkdir`, or append inside it — every decision goes through `kraken order buy` / `kraken session note`, so the recorder stays the single writer.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
