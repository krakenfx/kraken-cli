---
name: recipe-playground-rebalance
version: 1.1.0
description: "Example driver: drive a portfolio-rebalance hypothesis into a recorded session."
metadata:
  openclaw:
    category: "recipe"
    domain: "sessions"
  requires:
    bins: ["kraken"]
    skills: ["kraken-playground", "kraken-rebalancing"]
---

# Playground: Rebalance

> **PREREQUISITE:** Load the following skills to execute this recipe: `kraken-playground`, `kraken-rebalancing`

> **This recipe is an example, not a boundary.** Adapt its steps to your hypothesis, or write your own driver — see `kraken-playground` → Driving Your Own Hypothesis.

Test a target-allocation rebalancing hypothesis on live prices with no real money, recording the market data, paper trades, and the "why" behind each adjustment into one session for later replay and P&L analysis.

## Important

Paper results may overstate live performance: fees and slippage are simulated and there are no partial fills (see `kraken-paper-strategy`). This recipe records a session inside a paper workspace; it never places a live order.

Every number that gates a trade or a skip (`targets`, `threshold_usd`, `drift_pct`) belongs in `--strategy-params` — it's what makes two sessions comparable knob-for-knob. A recurring run adds `rounds` and `interval_s` (see step 8). The values in the step-2 example are placeholders: a number the operator names (band, threshold, cadence) replaces the example verbatim — params that don't match the mandate record a hypothesis nobody asked to test. Over a long window this recipe is **interventional** — one visit per `interval_s`, paced by the harness (see `kraken-playground` → Running over a Window).

## Quick Start

Rebalance a portfolio toward target allocations:

```
Hold a portfolio 50% BTC, 35% ETH, 15% SOL over 24 hours. 
Rebalance when any asset drifts more than 5% from target AND the delta exceeds $50. 
Check every 2 hours. Report final weights and P&L.
```

## Start the Session

Work inside a paper workspace (create one once: `kraken workspace create rebalance --capital 10000 --mode paper`), then:

```bash
export KRAKEN_WORKSPACE=rebalance

kraken session start \
  --symbols BTC/USD,ETH/USD,SOL/USD --channels ticker --to duckdb,jsonl \
  --label rebalance-$(date +%Y%m%d-%H%M%S) \
  --strategy recipe-playground-rebalance \
  --strategy-params '{"targets":{"BTCUSD":0.50,"ETHUSD":0.35,"SOLUSD":0.15},"interval_s":7200,"drift_pct":5,"threshold_usd":50}' \
  -o json 2>/dev/null &

# The session_started stdout line carries the id: {"type":"session_started","session":"s<n>",...}
```

Print the session id to the user right after starting, and again in the final report — it is the handle for resuming the `/loop`, checking `kraken session show`, and locating the artifacts.

## Schedule the Rounds

`/loop` owns the pacing. Set its interval to `interval_s` and it fires each round on cadence (late under jitter, never early). Parse the interval from the user's request: "every 2 hours" to `2h`, "every 4 hours" to `4h`, "every day" to `24h`.

**The scheduler floor is 60 seconds** (`/loop`, cron, and `ScheduleWakeup` all clamp to a one-minute minimum). `interval_s` must be `>= 60`. If the user asks for a sub-minute cadence, clamp to `60` and tell them — never set a sub-minute `interval_s` and never let the report claim a cadence the harness cannot run.

```
/loop 2h "Run one rebalance round for session s<n> in workspace rebalance per recipe-playground-rebalance"
```

Do not re-add an elapsed-time gate inside the round. `/loop` already enforces the spacing. The round cursor lives in the session's typed state cell — a rebalance round writes one decision **per leg**, so the decision log cannot count rounds; the cursor can. Reasons stay narrative (keep "round N" in them for the story; nothing parses it). Stop once the cursor reaches `rounds`:

```bash
LAST_ROUND=$(kraken session state get --session s<n> -o json 2>/dev/null \
  | jq '.cursor.round // 0')
if [ "${LAST_ROUND:-0}" -ge "$ROUNDS" ]; then
  kraken session stop -o json 2>/dev/null
  exit 0
fi
ROUND=$((LAST_ROUND + 1))
```

Between legs, record progress so a crashed firing resumes at the first
missing leg instead of re-trading executed ones; completing the round writes
only the round (the replace drops `legs_done` automatically):

```bash
kraken session state set --round "$LAST_ROUND" --legs-done "BTC/USD" \
  --note "round $ROUND: leg 1 of 3 done" -o json 2>/dev/null   # after each leg
kraken session state set --round "$ROUND" -o json 2>/dev/null   # round complete
```

On a fresh firing, `state get`'s `legs_done` names any half-done round — but
a crash can land between a leg executing and its `legs_done` write, so never
trust the cursor alone before re-trading: cross-check the evidence
(`kraken session decisions`, reasons mentioning the in-flight round) and skip
any leg that already filled. The cursor is the fast path; the decision log is
the truth.
 The first firing is the seed round: every asset is underweight, so it allocates the whole portfolio toward the targets.

## Each Round

On each firing: READ, THINK, ACT.

READ: a fresh balance and each symbol's price. Read the balance **every round** — never carry a snapshot across rounds or across trades within a round; each fill mutates cash and holdings.

```bash
kraken paper balance -o json 2>/dev/null
kraken ticker BTCUSD -o json 2>/dev/null | jq -r '.[] | .last_price'   # repeat per symbol
```

`kraken paper balance` returns `{"balances":{"USD":{"total","reserved","available"},...}}`. Cash is `.balances.USD.available`; a holding is `.balances.<ASSET>.total` (0 if the key is absent). Price is the ticker's last-trade close (`.[] | .last_price`).

**Fail loud before any leg trades:** validate the cash figure and EVERY price
before computing deltas — a rebalance acts on several legs, so one silently
failed read sizes every order from garbage. On an empty read, note an alert
and end the firing (the claimed round stays claimed; the next firing retries):

```bash
if [ -z "$CASH" ] || [ -z "$PRICE_BTC" ]; then
  kraken session note --kind alert \
    --reason "round $ROUND aborted: READ failed (cash='$CASH' btc='$PRICE_BTC')" -o json 2>/dev/null
  exit 0
fi
```

THINK: value the portfolio, then decide each leg.
- `total_value = cash + Σ(holdings × price)`
- Per symbol: `value = holdings × price`, `current_pct = value / total_value × 100`, `target_pct = target × 100`, `drift = current_pct − target_pct`, `delta_usd = target_value − value` where `target_value = total_value × target`.
- A leg needs action only when `|drift| > drift_pct` **AND** `|delta_usd| > threshold_usd`. Otherwise leave it.

ACT: **sells before buys.** Execute every overweight leg (`delta_usd < 0`) first so its proceeds land back in cash, then execute the underweight legs. Size each buy against a fee/slippage reserve so the last leg never rejects: keep the buy cost at or below `available_cash / (1 + fee_rate + slippage_rate)` (defaults 0.0026 fee, 0.0 slippage — read them from `paper status` if unsure). Between legs, re-read the balance rather than assuming the pre-trade cash, and skip or shrink a buy the remaining cash can no longer fund.

```bash
# Overweight — sell the excess first
kraken order sell BTC/USD <volume> --type market \
  --reason "rebalance round N: BTC/USD weight <current_pct>% above target <target_pct>%; selling <volume>" \
  -o json 2>/dev/null

# Underweight — buy toward target, capped by fee-reserved cash
kraken order buy SOL/USD <volume> --type market \
  --reason "rebalance round N: SOL/USD weight <current_pct>% below target <target_pct>%; buying <volume>" \
  -o json 2>/dev/null
```

If no leg breaches both thresholds, note a single skip:

```bash
kraken session note --kind skip --symbol BTC/USD \
  --reason "skip round N: all weights within <drift_pct>%/$<threshold_usd> band" \
  -o json 2>/dev/null
```

Always pass the numeric reason so the decision log carries the "why" behind every round.

## Stop and Review

```bash
kraken session stop -o json 2>/dev/null
kraken session show -o json 2>/dev/null | jq '.summary'

kraken session decisions --session s<n> -o json 2>/dev/null \
  | jq -c '.decisions[] | {kind, symbol, reason, timestamp}'
```

Report:
- Final weights vs targets (50/35/15)
- Number of rebalance trades vs skips
- Cumulative turnover (fees paid)
- P&L vs a buy-and-hold baseline
- Did the `drift_pct`/`threshold_usd` band hold the portfolio?

## Hard Rules

- This recipe records a session inside a paper workspace. It never places a live order.
- `/loop` drives the pace and the agent runs each round; the CLI has no scheduler and runs no strategy. The scheduler floor is 60s — no sub-minute cadence.
- Read a fresh balance every round and after each fill. Never size a trade off a stale snapshot — the seed round alone issues three buys, and a fixed pre-trade cash figure overspends once fees are charged.
- Sells before buys, and reserve fees on every buy, so a multi-leg round cannot reject its last order after earlier fills were already recorded.
- Keep every gating number (`targets`, `drift_pct`, `threshold_usd`, `interval_s`) in `--strategy-params` and the arithmetic in `jq`/`bc`, so a rerun on the same tape reproduces the decisions.
- The session directory (`session.json`, `decisions.jsonl`, and the tape sinks) is owned by the CLI recorder. The agent only **reads** it, and only for the stop-time summary. Never write, edit, or append inside it — every trade and skip goes through `kraken order buy` / `kraken session note`, so the recorder stays the single writer.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
