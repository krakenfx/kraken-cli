---
name: recipe-playground-price-alert
version: 1.0.0
description: "Example driver: drive a price-level-alert hypothesis into a recorded session."
metadata:
  openclaw:
    category: "recipe"
    domain: "playground"
  requires:
    bins: ["kraken"]
    skills: ["kraken-playground", "kraken-alert-patterns"]
---

# Playground: Price Alert

> **PREREQUISITE:** Load `kraken-playground` and `kraken-alert-patterns` to run this recipe.
> **This recipe is an example, not a boundary.** Adapt the steps to your hypothesis, or write your own driver (see `kraken-playground` → Driving Your Own Hypothesis).

Watch key price levels on live prices and record each crossing as an alert decision, capturing the market data and the "why" into one session for later replay and analysis. Alert-only: it records crossings, it never places a trade.

Use this skill for:
- watching one or more price levels over a window
- recording each level break as an alert with the numbers behind it
- grading afterward which levels were hit, when, and which were never touched

## Important

Alerts are informational and never execute a trade (see `kraken-alert-patterns`). This recipe records a session (market data + alert decisions); it never places a live order.

Pacing is external. `/loop` owns the schedule and fires one round per interval (see `kraken-playground` → Pacing). A cadence the operator names is a commitment: record it as `interval_s` and honor it — polls may run late, never early. A session whose params omit `interval_s` may not claim or imply any cadence in its report.

Over a long window this recipe is **observational** — a missed poll is not a missed crossing. The recorder holds every level break, so late visits (or none) still grade the hypothesis at stop time (see `kraken-playground` → Running over a Window).

## Params

Every level that gates an alert goes in `--strategy-params`, not only in prose reasons. That is what makes two sessions comparable knob-for-knob:

- `upper`: alert when price breaks above this level
- `lower`: alert when price breaks below this level
- `interval_s`: spacing between rounds; set the `/loop` interval to this value

The values in the example are placeholders. A level the operator names replaces the example verbatim — params that don't match the request record a hypothesis nobody asked to test.

## Quick Start

Natural language:

```
Watch BTC for 30 minutes. Alert me if price breaks above $70,000 or below
$60,000. Check every 5 minutes. At the end, show which levels were hit and when.
```

## Start the Session

Work inside a paper workspace (create one once: `kraken workspace create alerts --capital 10000 --mode paper`), then:

```bash
export KRAKEN_WORKSPACE=alerts

kraken session start \
  --symbols BTC/USD --channels ticker,trade --to duckdb,jsonl \
  --label alert-btc-$(date +%Y%m%d-%H%M%S) \
  --strategy recipe-playground-price-alert \
  --strategy-params '{"upper":70000,"lower":60000,"interval_s":300}' \
  -o json 2>/dev/null &

# The session_started stdout line carries the id: {"type":"session_started","session":"s<n>",...}
```

Print the session id to the user right after starting, and again in the final report — it is the handle for resuming the `/loop`, checking `kraken session show`, and locating the artifacts.

## Schedule the Rounds

`/loop` owns the pacing. Set its interval to `interval_s` and it fires each round on cadence (late under jitter, never early). Parse the interval from the user's request: "every 5 minutes" to `5m`, "every 2 minutes" to `2m`, "every hour" to `1h`.

**The scheduler floor is 60 seconds** (`/loop`, cron, and `ScheduleWakeup` all clamp to a one-minute minimum). `interval_s` must be `>= 60`. If the user asks for a sub-minute cadence ("every 30 seconds"), clamp to `60` and tell them — never set a sub-minute `interval_s` and never let the report claim a cadence the harness cannot run.

```
/loop 5m "Run one price-alert round for session s<n> in workspace alerts per recipe-playground-price-alert"
```

Do not re-add an elapsed-time gate inside the round. `/loop` already enforces the spacing.

## Each Round

On each firing: READ, THINK, ACT.

READ: current price.

```bash
kraken ticker BTCUSD -o json 2>/dev/null
```

Price is the ticker's last-trade close. Extract it with `jq`; do not eyeball it. Bind the band from the same `--strategy-params` you recorded — never hardcode the levels into the arithmetic:

```bash
UPPER=70000
LOWER=60000
PRICE=$(kraken ticker BTCUSD -o json 2>/dev/null | jq -r '.[] | .last_price')
```

THINK — alert on the *crossing*, not on the *level*. A sustained excursion is one event, not one per round. Classify the price into a **zone** — `above` / `within` / `below` — and alert only when the zone *changes* into a breakout. `kind` alone can't carry this (an `alert` doesn't say which side, and the per-round skip note would erase the last alert), so the zone lives in the session's typed state cell; reasons stay narrative:

```bash
PREV_ZONE=$(kraken session state get --session s<n> -o json 2>/dev/null \
  | jq -r '.cursor.zone // "within"')

# Fail loud: an empty price must not classify a zone (a fabricated "within"
# would silently swallow a real breakout).
if [ -z "$PRICE" ]; then
  kraken session note --kind alert --symbol BTC/USD \
    --reason "round skipped: READ failed (price empty)" -o json 2>/dev/null
  exit 0
fi
if   (( $(echo "$PRICE > $UPPER" | bc -l) )); then ZONE=above
elif (( $(echo "$PRICE < $LOWER" | bc -l) )); then ZONE=below
else ZONE=within; fi

# Persist the zone for the next firing (typed: a typo refuses instead of
# silently resetting the crossing detector).
kraken session state set --zone "$ZONE" -o json 2>/dev/null
```

- Zone entered `above` from a different zone → the upper level was just crossed — **alert**.
- Zone entered `below` from a different zone → the lower level was just crossed — **alert**.
- Zone unchanged (still `above`, still `below`, or still `within`) → same state, no new crossing — **skip**.
- `within` re-arms both boundaries: the next move to `above` or `below` alerts again.

Because the marker is on every line, `above → below` reads as a genuine change and alerts, and a sustained `above → above` reads as unchanged and does not.

ACT — alert on a fresh breakout, always ending the reason with the new `zone=` marker:

```bash
# upper break (ZONE=above, PREV_ZONE != above)
kraken session note --kind alert --symbol BTC/USD \
  --reason "alert round N: BTC/USD broke above <upper> at <price>; zone=above" \
  -o json 2>/dev/null

# lower break (ZONE=below, PREV_ZONE != below)
kraken session note --kind alert --symbol BTC/USD \
  --reason "alert round N: BTC/USD broke below <lower> at <price>; zone=below" \
  -o json 2>/dev/null
```

ACT (skip) — no zone change; record the round and carry the current zone forward so the next round sees it:

```bash
kraken session note --kind skip --symbol BTC/USD \
  --reason "skip round N: BTC/USD at <price>, no new crossing (<lower>/<upper>); zone=$ZONE" \
  -o json 2>/dev/null
```

Always pass the numeric reason, and always end it with the `zone=` marker — it is the state the next round reads to tell a fresh crossing from a sustained move.

## Stop and Review

```bash
kraken session stop -o json 2>/dev/null
kraken session show -o json 2>/dev/null | jq '.summary'

kraken session decisions --session s<n> -o json 2>/dev/null \
  | jq -c '.decisions[] | select(.kind=="alert") | {timestamp, reason}'
```

Report:
- which levels were crossed (upper and/or lower)
- when each crossing occurred
- any level never touched (INCONCLUSIVE for that level)

## Hard Rules

- This recipe records a session. It never places a live order — alerts are informational only.
- `/loop` drives the pace and the agent runs each round. The CLI has no scheduler and runs no strategy.
- Do not add a pacing gate inside the round; `/loop` enforces spacing.
- A scheduled `/loop` fire only runs what the agent may invoke unprompted. Keep `kraken ticker`, `kraken session note`, and `kraken session show` permitted non-interactively, or a fire stalls waiting on an approval you never see.
- Keep every level in `--strategy-params` and the comparison in `jq`/`bc`, so a rerun on the same tape reproduces the decisions.
- A session whose params omit `interval_s` may not claim any cadence in its report.
- The session directory (`session.json`, `decisions.jsonl`, and the tape sinks) is owned by the CLI recorder. The agent only **reads** it, and only for the stop-time summary. Never write, edit, `mkdir`, or append inside it — every decision goes through `kraken session note`, so the recorder stays the single writer.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
