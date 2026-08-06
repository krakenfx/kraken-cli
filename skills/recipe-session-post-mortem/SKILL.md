---
name: recipe-session-post-mortem
version: 1.0.0
description: "Narrate a recorded session's post-mortem: replay the window, explain the P&L decomposition in plain terms, and deliver a presentation-ready summary."
metadata:
  openclaw:
    category: "recipe"
    domain: "sessions"
  requires:
    bins: ["kraken"]
    skills: ["kraken-playground"]
---

# Session Post-Mortem

> **PREREQUISITE:** Load `kraken-playground` to understand what a session is and where it lives.

Turn a stopped, recorded session into a story a non-trader can follow: what the market did, what the agent did, where every dollar of P&L went, and what to change next session. The CLI supplies the deterministic facts (`kraken explain pnl`, `kraken replay`); this recipe is the interpretive layer on top. The output is good enough to paste into a board deck.

Use this skill for:
- reviewing a session after it stops
- explaining a P&L number component-by-component in plain language
- producing an executive-ready one-pager from a recorded session

## Important

The session must be **stopped** (`kraken session stop`). `explain pnl` decomposes a stopped session's window against its recorded tape; on a recording session, stop it first.

The CLI stays deterministic; the narrative is yours. Every dollar figure, percentage, and count in the narrative must come from the command output — never estimate, extrapolate, or invent a number the JSON does not contain.

## Step 1: Pull the P&L Decomposition

```bash
kraken explain pnl --session "$SESSION_ID" -o json 2>/dev/null   # --session defaults to latest
```

One JSON object. The parts that drive the narrative:

- `anchor`: `starting_balance`, `final_value`, `total_pnl`, `currency` — the headline numbers.
- `components[]`: the waterfall. Each entry has `kind`, `amount`, and a ready-made plain-terms `explanation`:
  - `price_movement` — position changes valued at mid-market, before any costs
  - `fees` — what the fee rate took across all fills
  - `spread` — the half-spread paid by market fills (and limit-fill timing)
  - `slippage` — simulated slippage on market fills
  - `residual` — the reconciliation gap; **components + residual always sum to `total_pnl`**
- `trades[]`: per-fill attribution (`side`, `volume`, `price`, `notional`, `fees`, `spread`, `price_movement`) **plus the recorded `reason`** — the agent's own words for why it traded.
- `window.symbols[]`: what the market did (`first_mid`, `last_mid`, `move_pct`, `avg_spread`, `frames`).
- `caveats[]` and `missed_fills`: honest unknowns. If either is non-empty, the narrative must mention them.

Useful digests:

```bash
# Headline + waterfall
kraken explain pnl --session "$SESSION_ID" -o json 2>/dev/null \
  | jq '{anchor, waterfall: [.components[] | {kind, amount}]}'

# Cost per trade, with the agent's stated reason
kraken explain pnl --session "$SESSION_ID" -o json 2>/dev/null \
  | jq -r '.trades[] | [.side, .volume, .price, .fees, .reason] | @tsv'
```

## Step 2: Replay the Timeline

```bash
kraken replay --session "$SESSION_ID" --speed 1000 -o json 2>/dev/null
```

NDJSON, one event per line, in recorded order. `--speed` is a multiplier (0.01–1000, default 1 = real time with recorded gaps reproduced); for analysis always use `--speed 1000` so the tape flushes as fast as pacing allows. Three record families interleave on the same clock:

- **Market frames** — `{channel, type: "snapshot"|"update", data: [...]}`: the price tape.
- **Account events** — `{event: "initialized"|"order_filled"|"command", ...}`: balances, fills with `reference_quote`, and command outcomes.
- **Decisions** — `{kind, symbol, reason, order_id}`: the why behind each action, including skips.

Digest it rather than reading every tick:

```bash
REPLAY=$(kraken replay --session "$SESSION_ID" --speed 1000 -o json 2>/dev/null)

# The action beats: every fill and every decision, in order
echo "$REPLAY" | jq -c 'select(.event == "order_filled" or .kind != null)'

# The price path: first, last, low, high of the recorded mids
echo "$REPLAY" | jq -s '[.[] | select(.channel == "ticker") | .data[0].last] |
  {first: .[0], last: .[-1], low: min, high: max}'
```

## Step 3: Narrate

Weave both outputs into one story, in this order:

1. **Headline** — one sentence: outcome and dominant cause, citing `total_pnl` and the largest component. *"The session lost $20.70 on a flat market: $20.18 of it was fees from churning three fills through an 0.02% move."*
2. **What the market did** — from `window.symbols` and the replay price path: direction, size of the move, spread conditions.
3. **What the agent did** — from the replay beats: each decision with its recorded `reason`, and whether the fill helped or hurt (per-trade `price_movement` vs `fees` + `spread`).
4. **Where the money went** — the waterfall, every component, summing to the total. Name the dominant cost in plain terms.
5. **Patterns** — call out what the numbers show: churning a flat market and paying the spread N times, buying strength that faded, skips that saved money, fees exceeding gross edge.
6. **Next steps** — concrete, tied to the evidence: fewer/larger fills to cut the fee bill, limit orders to earn the spread instead of paying it, a wider trigger threshold, a different window.

Honesty rules:
- Quote figures exactly (round for prose: dollars to cents, percentages to two decimals).
- If `caveats` or `missed_fills.orders` are non-empty, state them plainly — they bound what the decomposition can claim.
- Never attribute intent the decision log doesn't record. The `reason` fields are the only source for "why".

## Zero-Trade Sessions

A session with no fills is a valid post-mortem, not an error: `trades` is `[]` and every component is zero. Say so directly — "no trades were placed, so the balance is unchanged" — then narrate what the market did over the window and, if decisions were logged (skips with reasons), whether staying out was the right call given the tape.

## Presenting to an Executive Audience

When the post-mortem is for a demo or leadership review, format the same content as a one-pager:

- Lead with the headline sentence, then the waterfall as a small table: component, signed dollar amount, one plain-terms phrase each (crib from the `explanation` fields).
- Follow with 3–6 timeline beats (time, action, reason, effect) — not the full tape.
- Close with the next-step list.
- No jargon in the top half: "cost of crossing the bid-ask gap" beats "half-spread on taker fills".
- If the environment can render documents or artifacts, a single page with the waterfall as the centerpiece chart lands best; the narrative text stands alone if not.

The pitch this demonstrates: every automated trading session is fully auditable after the fact — the tape, the decisions, and the P&L reconcile to the cent, and an agent can explain it in plain language on demand.

## Hard Rules

- Read-only. This recipe never places orders, never starts or stops sessions (except telling the user to stop a recording one), and never writes into the session directory.
- Every number in the narrative traces to a field in `explain pnl` or `replay` output.
- The waterfall must be presented complete — components plus residual sum to the total; do not drop a component because it is small or unflattering.
- Surface `caveats` and `missed_fills` whenever they are non-empty. A polished story that hides a caveat is wrong, not polished.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
