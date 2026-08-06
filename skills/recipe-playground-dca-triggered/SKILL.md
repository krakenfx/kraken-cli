---
name: recipe-playground-dca-triggered
version: 1.0.0
description: "Example driver: dip-triggered dollar-cost-averaging on a live WebSocket stream, recorded to a session."
metadata:
  openclaw:
    category: "recipe"
    domain: "sessions"
  requires:
    bins: ["kraken"]
    skills: ["kraken-playground", "kraken-dca-strategy", "kraken-ws-streaming"]
---

# Playground: DCA (Dip-Triggered)

> **PREREQUISITE:** Load `kraken-playground`, `kraken-dca-strategy`, and `kraken-ws-streaming` to run this recipe.
> **This recipe is an example, not a boundary.** Adapt the steps to your hypothesis, or write your own driver (see `kraken-playground` → Driving Your Own Hypothesis).

Buy the instant price dips below a short-term average, not on a fixed clock. A WebSocket ticker stream drives the loop and every qualifying tick is a candidate buy, rate-limited so buys stay spaced. Record every buy and skip into one session for later replay and P&L.

Use this skill for:
- catching an intra-interval dip that a time-based sampler would miss
- event-driven DCA where the trigger is a price condition, not a clock
- recording a streamed decision loop for replay

## Important

This recipe records a session. It never places a live order. Paper results may overstate live performance: fees and slippage are simulated and there are no partial fills (see `kraken-paper-strategy`).

The stream is the loop, not `/loop`. A `while read` over a stream is a long-lived process that holds state (the reference average, the last-buy time) in memory across events. This is a deliberate exception to the stateless-round model (see `kraken-playground` → Running over a Window); keep the session short and the state minimal.

The stream is blind during reconnect gaps. The CLI reconnects with paced exponential backoff (up to 12 attempts per stream lifecycle, see `kraken-ws-streaming`), and any dip inside that window is missed. The recorder tolerates gaps; a decision loop does not.

## Params

Every number that gates a buy goes in `--strategy-params`:

- `dollars_per_buy`: quote currency deployed per triggered buy
- `dip_threshold_pct`: buy when price is at least this far below the short-term average
- `min_spacing_s`: minimum seconds between buys; the rate limit on the trigger. Enforce it from the typed cursor: `kraken session state set --last-action-at <now>` after each buy, and skip the trigger while `now - .cursor.last_action_at < min_spacing_s` (`kraken session state get`)
- `max_buys`: session length in buys; stop after this many
- `sma_refresh_s`: how often to refresh the reference average from REST

## Quick Start

Natural language:

```
Dollar-cost-average into Bitcoin. Watch the live price and buy $100 whenever
it dips more than 0.10% below the 1-hour SMA, but no more than once every
20 minutes, up to 10 buys. Record all buys and skips. At the end, show P&L.
```

## Start the Session

Work inside a paper workspace (create one once: `kraken workspace create dcatrig --capital 10000 --mode paper`), then:

```bash
export KRAKEN_WORKSPACE=dcatrig

kraken session start \
  --symbols BTC/USD --channels ticker,trade --to duckdb,jsonl \
  --label dcatrig-btc-$(date +%Y%m%d-%H%M%S) \
  --strategy recipe-playground-dca-triggered \
  --strategy-params '{"dollars_per_buy":100,"dip_threshold_pct":-0.10,"min_spacing_s":1200,"max_buys":10,"sma_refresh_s":300}' \
  -o json 2>/dev/null &

# The session_started stdout line carries the id: {"type":"session_started","session":"s<n>",...}
```

Print the session id to the user right after starting, and again in the final report — it is the handle for checking `kraken session show` and locating the artifacts.

## Stream and Decide

Subscribe to the ticker with the BBO trigger to cut noise, and act on each tick. Hold the reference average and the last-buy time in the loop; refresh the average from REST every `sma_refresh_s`, not on the tick rate.

Bind the gating numbers to the same `--strategy-params` you recorded, so the loop runs the hypothesis you started — never hardcode them into the arithmetic:

```bash
DOLLARS_PER_BUY=100
DIP_THRESHOLD_PCT=-0.10
MIN_SPACING_S=1200
MAX_BUYS=10
SMA_REFRESH_S=300

# True 1h SMA: last twelve 5-minute closes. (--interval 60 would average the
# whole returned window — a multi-day mean, not 1h.)
SMA=$(kraken ohlc BTCUSD --interval 5 -o json 2>/dev/null | jq '[.candles[-12:][].close] | add/length')
SMA_TS=$(date +%s)
LAST_BUY=0
BUYS=0

kraken ws ticker BTC/USD --event-trigger bbo -o json 2>/dev/null | while read -r line; do
  PRICE=$(echo "$line" | jq -r '.data[0].last // empty'); [ -z "$PRICE" ] && continue
  NOW=$(date +%s)

  # Refresh SMA on its own cadence, not per tick
  if [ $((NOW - SMA_TS)) -ge $SMA_REFRESH_S ]; then
    SMA=$(kraken ohlc BTCUSD --interval 5 -o json 2>/dev/null | jq '[.candles[-12:][].close] | add/length')
    SMA_TS=$NOW
  fi

  # Fail loud, never fabricate: a broken READ must not gate a buy or write a reason.
  if [ -z "$PRICE" ] || [ -z "$SMA" ]; then
    kraken session note --kind alert --symbol BTC/USD \
      --reason "tick skipped: READ failed (price='$PRICE' sma='$SMA')" -o json 2>/dev/null
    continue
  fi
  VS_SMA=$(echo "scale=6; (($PRICE - $SMA) / $SMA) * 100" | bc -l)

  # THINK: dip tripped AND rate limit satisfied?
  if (( $(echo "$VS_SMA <= $DIP_THRESHOLD_PCT" | bc -l) )) && [ $((NOW - LAST_BUY)) -ge $MIN_SPACING_S ]; then
    VOL=$(echo "scale=8; $DOLLARS_PER_BUY / $PRICE" | bc -l)
    kraken order buy BTC/USD "$VOL" --type market \
      --reason "dip-triggered buy: BTC/USD at $PRICE is ${VS_SMA}% below 1h SMA $SMA (threshold ${DIP_THRESHOLD_PCT}%); deploying \$$DOLLARS_PER_BUY" \
      -o json 2>/dev/null
    LAST_BUY=$NOW
    BUYS=$((BUYS + 1))
    [ $BUYS -ge $MAX_BUYS ] && break
  fi
done

kraken session stop -o json 2>/dev/null
```

Notes on the decision:
- `min_spacing_s` is a real rate limit on the trigger, not the sampling bookkeeping the time-gated recipe removed. It stops a sustained dip from firing on every tick.
- Refresh the average on `sma_refresh_s`. Recomputing it per tick burns REST calls for a number that barely moves.
- Log skips sparingly. A stream produces many non-qualifying ticks; noting each one floods the decision log. Note only meaningful events (a dip that was rate-limited, a wide spread), never every tick.

## Stop and Review

```bash
kraken session show -o json 2>/dev/null | jq '.summary'

kraken session decisions --session s<n> -o json 2>/dev/null \
  | jq -c '.decisions[] | {kind, symbol, reason}'
```

Report:
- buys placed vs `max_buys`, and time between them
- average fill cost vs window mean
- how many dips were caught vs rate-limited
- P&L, and whether the dip threshold and spacing fit the tape

## Hard Rules

- This recipe records a session. It never places a live order.
- The stream is the loop. Do not also wrap it in `/loop`; that is two clocks on one decision.
- Treat stream output as NDJSON, one object per line. Never parse it as a single document (see `kraken-ws-streaming`).
- The loop is blind during reconnect gaps. Keep the session short and accept that dips inside a backoff window are missed.
- Rate-limit buys with `min_spacing_s` so a sustained dip does not fire on every tick.
- Keep every gating number in `--strategy-params` and the arithmetic in `jq`/`bc`.
- The session directory (`decisions.jsonl` and the DuckDB/JSONL sinks) is owned by the CLI recorder. The agent only **reads** it, and only for the stop-time summary. Never write, edit, `mkdir`, or append inside it — every buy goes through `kraken paper buy`, so the recorder stays the single writer.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
