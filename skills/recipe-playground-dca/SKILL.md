---
name: recipe-playground-dca
version: 1.0.0
description: "Example driver: drive a time-based dollar-cost-averaging hypothesis into a recorded session."
metadata:
  openclaw:
    category: "recipe"
    domain: "sessions"
  requires:
    bins: ["kraken"]
    skills: ["kraken-playground", "kraken-dca-strategy"]
---

# Playground: DCA (Time-Gated)

> **PREREQUISITE:** Load `kraken-playground` and `kraken-dca-strategy` to run this recipe.
> **This recipe is an example, not a boundary.** Adapt the steps to your hypothesis, or write your own driver (see `kraken-playground` → Driving Your Own Hypothesis).

Test a dollar-cost-averaging hypothesis on live prices with no real money. Buy on a fixed cadence, optionally only when price dips below a short-term average, and record every buy, skip, and average cost into one session for later replay and P&L.

Use this skill for:
- running a time-based DCA hypothesis over a window
- gating each buy on a dip below a short-term average
- recording every decision with the numbers behind it

## Important

This recipe records a session inside a paper workspace. It never places a live order. Paper results may overstate live performance: fees and slippage are simulated and there are no partial fills (see `kraken-paper-strategy`).

Pacing is external. `/loop` owns the schedule and fires one round per interval (see `kraken-playground` → Pacing). One decision per interval, paced from outside, so the session is interventional over the window (see `kraken-playground` → Running over a Window).

## Params

Every number that gates a buy or skip goes in `--strategy-params`, not only in prose reasons. That is what makes two sessions comparable knob-for-knob:

- `dollars_per_buy`: quote currency deployed per qualifying round
- `rounds`: session length in decision rounds
- `interval_s`: spacing between rounds; set the `/loop` interval to this value
- `dip_threshold_pct` (dip-gated variant): only buy when price is at least this far below the short-term average; omit for unconditional time-based DCA

## Quick Start

Natural language:

```
Dollar-cost-average into Bitcoin over 10 hours. Buy $100 every hour if the
price dips more than 0.10% below the 1-hour SMA. Record all buys, skips, and
average cost. At the end, show P&L.
```

## Start the Session

Work inside a paper workspace (create one once: `kraken workspace create dca --capital 10000 --mode paper`), then:

```bash
export KRAKEN_WORKSPACE=dca

kraken session start \
  --symbols BTC/USD --channels ticker,trade --to duckdb,jsonl \
  --label dca-btc-$(date +%Y%m%d-%H%M%S) \
  --strategy recipe-playground-dca \
  --strategy-params '{"dollars_per_buy":100,"rounds":10,"interval_s":3600,"dip_threshold_pct":-0.10}' \
  -o json 2>/dev/null &

# The session_started stdout line carries the id: {"type":"session_started","session":"s<n>",...}
```

Print the session id to the user right after starting, and again in the final report — it is the handle for resuming the `/loop`, checking `kraken session show`, and locating the artifacts.

## Schedule the Rounds

`/loop` owns the pacing. Set its interval to `interval_s` and it fires each round on cadence (late under jitter, never early). Parse the interval from the user's request: "every hour" to `1h`, "every 30 minutes" to `30m`, "twice a day" to `12h`.

**The scheduler floor is 60 seconds** (`/loop`, cron, and `ScheduleWakeup` all clamp to a one-minute minimum). `interval_s` must be `>= 60`; a sub-minute cadence is impossible — clamp to `60` and tell the user if they ask for less.

```
/loop 1h "Run one DCA round for session s<n> in workspace dca per recipe-playground-dca"
```

Do not re-add an elapsed-time gate inside the round. `/loop` already enforces the spacing.

## Each Round

On each firing: stop if the session is complete, otherwise READ, THINK, ACT.

Check the last completed round against `rounds`. The round cursor lives in the session's typed state cell (`kraken session state get/set`) — not in the decision log, whose reasons stay free narrative (keep writing "round N" in them for the post-mortem story, but nothing parses it). Reading the cursor is O(1) however long the session runs, and a typo'd field refuses at `set` instead of silently steering the loop:

```bash
ROUNDS=10   # the `rounds` you set in --strategy-params
SESSION_ID=s1   # from the session_started line

# The typed cursor is the loop's memory: unset reads as round 0.
LAST_ROUND=$(kraken session state get --session "$SESSION_ID" -o json 2>/dev/null \
  | jq '.cursor.round // 0')
if [ "${LAST_ROUND:-0}" -ge "$ROUNDS" ]; then
  kraken session stop -o json 2>/dev/null
  exit 0
fi
ROUND=$((LAST_ROUND + 1))
# CLAIM the round BEFORE acting: a crash after the claim skips one buy
# (harmless); the reverse order would redo the round and buy twice. `set`
# replaces the whole cursor — carry every field your strategy tracks.
kraken session state set --round "$ROUND" -o json 2>/dev/null
```

READ: price and the short-term average. Mind the response shapes: the ticker
is keyed by Kraken's INTERNAL pair name (`XXBTZUSD`, not `BTC/USD`), so read
the price through the value (`.[].last_price`). The ohlc rows live under
`.candles`; read that array directly — the sibling `last` cursor would poison
naive iteration of the object.

```bash
PRICE=$(kraken ticker BTCUSD -o json 2>/dev/null | jq -r '.[].last_price')
# True 1h SMA: the last twelve 5-minute closes (NOT --interval 60, whose
# full-window mean is a multi-day average).
SMA=$(kraken ohlc BTCUSD --interval 5 -o json 2>/dev/null | jq '[.candles[-12:][].close] | add / length')
```

**Fail loud, never fabricate:** if either value comes back empty, the round
must not guess — note an alert and end the firing, or the decision log fills
with plausible-looking false reasons:

```bash
if [ -z "$PRICE" ] || [ -z "$SMA" ]; then
  kraken session note --kind alert --symbol BTC/USD \
    --reason "round $ROUND aborted: READ failed (price='$PRICE' sma='$SMA')" -o json 2>/dev/null
  exit 0
fi
```

Compute in `jq`/`bc`, do not eyeball it.

THINK:
- `vs_sma = (price - sma) / sma * 100`
- If the ticker spread is wider than 1%, skip and note it. A wide spread distorts the paper fill.
- If `vs_sma <= dip_threshold_pct`, buy `dollars_per_buy / price` units. Otherwise skip.

ACT (buy):

```bash
kraken order buy BTC/USD <volume> --type market \
  --reason "DCA round N: BTC/USD at <price> is <vs_sma>% below 1h SMA <sma> (threshold <dip>%); deploying \$<dollars_per_buy>" \
  -o json 2>/dev/null
```

Inside the paper workspace the order fills on the paper account, and the reason lands in the active session's decision log.

ACT (skip):

```bash
kraken session note --kind skip --symbol BTC/USD \
  --reason "skip round N: BTC/USD at <price> is <vs_sma>% from 1h SMA (threshold <dip>%)" \
  -o json 2>/dev/null
```

The round is already claimed, so nothing to write after acting: buy or skip,
log the reason, and exit the firing.

Always pass the numeric reason so the decision log carries the "why" behind every round.

## Stop and Review

```bash
kraken session stop -o json 2>/dev/null
kraken session show -o json 2>/dev/null | jq '.summary'
kraken explain pnl --session latest -o json 2>/dev/null | jq '{anchor, waterfall: [.components[] | {kind, amount}]}'

kraken session decisions --session "$SESSION_ID" -o json 2>/dev/null \
  | jq -c '.decisions[] | {kind, symbol, reason, timestamp}'
```

Report:
- total deployed vs expected (`dollars_per_buy × rounds`)
- buys vs skips, with dip measurements
- average fill cost vs window mean
- P&L, and whether the dip threshold fit the tape

## Hard Rules

- This recipe records a session inside a paper workspace. It never places a live order.
- `/loop` drives the pace and the agent runs each round. The CLI has no scheduler and runs no strategy.
- Do not add a pacing gate inside the round; `/loop` enforces spacing.
- A scheduled `/loop` fire only runs what the agent may invoke unprompted. Keep `kraken order buy`, `kraken session note`, and `kraken session show` permitted non-interactively, or a fire stalls waiting on an approval you never see.
- Keep every gating number in `--strategy-params` and the arithmetic in `jq`/`bc`, so a rerun on the same tape reproduces the decisions.
- The session directory (`session.json`, `decisions.jsonl`, and the tape sinks) is owned by the CLI recorder. The agent only **reads** it, and only for the stop-time summary. Never write, edit, `mkdir`, or append inside it — every buy and skip goes through `kraken order buy` / `kraken session note`, so the recorder stays the single writer.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
