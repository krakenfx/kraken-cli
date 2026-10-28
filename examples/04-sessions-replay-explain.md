# Sessions, replay, and P&L explain

A session is a recorded window over a workspace: the market tape, every fill,
every balance change, and every decision you logged, all on one timeline. Once a
session exists you can rewind it at any speed and ask for a plain-terms
breakdown of where the money went.

You need a paper workspace ([walkthrough 2](02-paper-workspaces.md)). Activate
it first:

```bash
export KRAKEN_WORKSPACE=btc-momentum
```

## Record a trading window

```bash
kraken session start --symbols BTC/USD --channels ticker,trade --for 5m --label first-window
```

The session streams live market data into the workspace and recording begins.
While it runs, trade in a second terminal:

```bash
kraken paper buy BTCUSD 0.05 --reason "testing the session recorder"
kraken session note --kind skip --symbol ETHUSD --reason "spread too wide, sitting out"
kraken paper sell BTCUSD 0.05
```

`--reason` and `session note` write to the session's decision log (`--kind`
takes `buy`, `sell`, `cancel`, `skip`, or `alert`; deliberate no-ops are worth
recording too). They cost nothing now and turn a P&L number into a story later.

The window closes itself after `--for` elapses, or close it early:

```bash
kraken session stop                # stops the latest session
kraken session show                # window, status, valuation
kraken session list                # every session in this workspace
```

## Replay it

```bash
kraken replay --session latest --speed 10
```

The recorded timeline plays back to stdout at 10x: market events, fills, and
decisions interleaved in original order. `--session` also accepts an ordinal
(`s3`) or a label (`first-window`). Speed ranges from 0.01 (slow motion)
to 1000.

Replay reads only from disk. No connection, no API calls, and the tape cannot
differ from what was recorded.

## Explain the P&L

```bash
kraken explain pnl --session latest
```

Instead of one opaque number, the session's P&L is decomposed line by line:

- **Price movement** — what the market did to your positions
- **Fees** — what execution cost you
- **Spread and slippage** — what crossing the book cost you
- **Missed fills** — limit orders that never executed
- **Residual** — anything the model cannot attribute, stated rather than hidden

Each trade appears with its context, including the reasons you logged. With
`-o json` the full report is machine-readable, so an agent can consume it too.

## Rerun a window against old data

`session start --from` replays a recorded tape into a fresh session instead of
subscribing live:

```bash
kraken session start --from tape:morning-session --speed 20 --label rerun-on-history
```

Same recorder, same journal, same scoring surface, but the market is
yesterday's. This is the primitive the
[autoresearch lab](05-autoresearch-lab.md) builds on: test an idea on captured
windows before you let it near a live feed.

## Hand it to an agent

The
[`recipe-session-post-mortem`](../skills/recipe-session-post-mortem/SKILL.md)
skill turns a stopped session into a story a non-trader can follow: it replays
the window, walks the P&L decomposition, and writes a summary you could paste
into a deck. The CLI supplies the deterministic facts; the agent supplies the
narration. With the skills installed ([how](06-agent-skills.md)):

> _"Do a post-mortem on my latest session. What did the market do, what did I
> do, and where did every dollar of P&L go?"_
