---
name: kraken-paper-to-live
version: 1.2.0
description: "Promote a validated paper strategy to live trading, gated on lab experiment evidence plus safety checks."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["kraken", "jq"]
    skills: ["kraken-paper-strategy", "kraken-spot-execution", "kraken-risk-operations", "kraken-lab-experiment"]
---

# kraken-paper-to-live

Use this skill for:
- validating a strategy has stable paper results before going live
- running pre-flight checks before first live trade
- migrating paper commands to their live equivalents
- establishing safety controls for the live session

## Paper-to-Live Performance Gap

Both spot and futures paper trading simulate taker fees (0.26% spot, configurable for futures). Spot paper supports optional flat-rate slippage simulation via `--slippage-rate`. Gaps remain between paper and live. Before promoting, factor in:

- **Fees are approximate.** Paper uses flat fee rates. Real fees depend on your volume tier and maker/taker status. Each round-trip trade costs 0.32-0.52% at base tier.
- **Slippage is a flat estimate.** If `--slippage-rate` was set (spot), market orders include a fixed slippage factor. Real slippage depends on order book depth, order size, and volatility. Futures paper does not model depth-based slippage.
- **No partial fills or latency.** Limit orders fill fully or not at all. Live orders may partially fill, queue, or be rejected. Network and matching-engine latency can cause missed entries or exits.
- **Maker fees:** Paper uses taker fee rates for all fills. Live limit orders that provide liquidity pay lower maker fees (0.16% spot).

When presenting promotion analysis to the user, note the remaining gaps (partial fills, depth-dependent slippage, tier-based fees). If slippage was not configured in paper, explicitly state that live market orders will fill at worse prices.

## Promotion Gate — evidence, not narrative

**Never propose a live order without the evidence block below.** The gate
rule: **at least 2 passing sessions of one frozen experiment, of which a live
paper session is mandatory.** Replay sessions enter the evidence only with their
lookahead caveat attached — replay cannot enforce lookahead (the agent can
read the source tape), so replay screens and live confirms; two replay
passes must never promote on their own.

**Start with the CLI's own checklist** — `kraken workspace promote` grades
the whole gate mechanically. Missing evidence returns the checklist in the
refusal envelope; a passing checklist requires explicit confirmation before
the workspace changes mode:

```bash
kraken workspace promote <workspace> -o json 2>/dev/null | jq '.checklist'
# .criteria[]: one row per gate criterion, satisfied true/false + detail
# .experiments[]: per-experiment passing_sessions / passing_live_sessions
# .manual_trades_in_windows: disclosed manual interference
# .blockers[]: active sessions or paper orders that must be resolved first
```

After promotion, orders use the normal configured Kraken credentials and the
venue's own fills are reconciled back into the workspace ledger by txid, so its
P&L is real. The workspace is an allocation of the account, not a Kraken
subaccount: the CLI enforces the allocation, Kraken does not, and anything
using the same API key can spend allocated funds. Expect two pre-venue
refusals, both `validation` envelopes:

- **Allocation budget** — an order spending beyond what the workspace's own
  ledger holds free of resting orders is refused. Route on the envelope's
  typed fields, never the message: `refusal: "allocation_budget"` with
  `available` and `required` (decimal strings), `funding` (the asset code the
  order spends — a pair leg, not necessarily the allocation currency), and
  `reconciled` (false means `available` may be stale). Reduce the size or
  cancel resting orders; this is the workspace's ledger, not the account
  balance.
- **Backing** — refused with `refusal: "over_allocated"` or `"unverified"`
  plus the full `backing` object. An over-allocated workspace stops spending
  the allocated asset until funds return to the account; an unverified one
  (balance unreadable) stops spending until credentials and connectivity are
  restored.

Size orders before placing them: reconciling live workspace commands (orders,
`balance`, `open-orders`, `trades-history`, `workspace status`) stamp
`.backing.spendable` — the allocation-asset ledger balance free of resting
orders. For orders that spend the allocation asset this is the figure the
budget gate checks `required` against; an order on a pair carrying the
allocation on neither leg is gated in its own funding leg instead (read that
asset from `balance`).

The order that pays the allocated asset back in — closing a position — passes
both refusals, so never tell a user they must abandon the workspace to exit a
trade.

Present that checklist verbatim. Then, for the per-session detail behind it,
build the evidence block from `kraken lab compare` — reproduced cold from
disk, seals verified via `kraken lab show`, no numbers of your own:

```bash
kraken lab show <exp> -o json 2>/dev/null      # seal must verify
kraken lab compare <exp> -o json 2>/dev/null | jq '{
  experiment, frozen, pass_count, total,
  sessions: [.sessions[] | {session,
                    verdict: (if .error != null then "error" else .verdict.pass end),
                    source: (.source.kind // "unknown"),
                    caveats}],
  live_run_passed: ([.sessions[] | select((.verdict.pass // false) and .source.kind == "live")] | length > 0)
}'
```

The evidence block you present for sign-off:

- `experiment` + `frozen` hash (the sealed goalposts),
- every session: session id, `source.kind` (`live`, `replay`, or `unknown` for an
  errored session — errored sessions carry no scorecard), verdict (`true`, `false`, or
  `"error"` — a mechanical FAIL is not an error), caveats verbatim,
- `pass_count` of `total`, and `live_run_passed` — **a passing run whose
  `source.kind` is `live`** (first-class, no caveat-string parsing).

**Refusals must name what is missing.** If `pass_count < 2`: say which sessions
failed or errored. If no live session passed: say "replay evidence only — a
passing live session is mandatory; run the plan's live entry" even when
`pass_count` ≥ 2. A tampered or missing experiment file blocks promotion
with its own `parse`/`validation` envelope — report it, never reconstruct.

**Reset rule:** a sealed spec cannot change. A new hypothesis, new criteria,
or a re-recorded tape is a **new experiment**, and its evidence count
starts at zero.

A strategy is ready for live promotion when:
1. The evidence block above passes the gate rule (≥2 passing sessions, live session
   mandatory, caveats attached).
2. Error handling works correctly (rate limits, network failures).
3. The strategy stays within defined risk parameters.
4. Paper returns remain positive with fees (0.26% spot, 0.05% futures) and slippage enabled (`--fee-rate`, `--slippage-rate`).
5. The user explicitly approves the transition.

## Pre-Flight Checklist

### Spot

Before the first live spot trade:

1. **Verify credentials**:
   ```bash
   kraken auth test -o json 2>/dev/null
   ```

2. **Check balance**:
   ```bash
   kraken balance -o json 2>/dev/null
   ```

3. **Confirm pair is tradable**:
   ```bash
   kraken pairs --pair BTCUSD -o json 2>/dev/null
   ```

4. **Validate a sample order** (does not execute):
   ```bash
   kraken order buy BTCUSD 0.001 --type limit --price 50000 --validate -o json 2>/dev/null
   ```

5. **Enable dead man's switch** — account-wide, so it cannot run inside a
   workspace: the timeout cancels every order on the API key, including other
   workspaces' and the account's own. Run it with no workspace active, and tell
   the user it covers their whole account, not just this strategy:
   ```bash
   KRAKEN_WORKSPACE= kraken order cancel-after 600 -o json 2>/dev/null
   ```

### Futures

Before the first live futures trade:

1. **Verify futures credentials**:
   ```bash
   kraken futures accounts -o json 2>/dev/null
   ```

2. **Check margin availability**:
   ```bash
   kraken futures accounts -o json 2>/dev/null
   ```

3. **Confirm instrument is tradable**:
   ```bash
   kraken futures instrument-status --symbol PF_XBTUSD -o json 2>/dev/null
   ```

4. **Set leverage**:
   ```bash
   kraken futures set-leverage PF_XBTUSD 10 -o json 2>/dev/null
   ```

5. **Enable dead man's switch**:
   ```bash
   kraken futures cancel-after 600 -o json 2>/dev/null
   ```

## Command Migration

Most paper and live commands differ only in the prefix. The order type default
does not carry over: `kraken paper buy` and `sell` default to `--type market`, while
`kraken order` and `kraken futures order` default to `--type limit`, so pass
`--type market` on live market orders.

### Spot

| Paper | Live |
|-------|------|
| `kraken paper buy BTCUSD 0.01` | `kraken order buy BTCUSD 0.01 --type market` |
| `kraken paper sell BTCUSD 0.01` | `kraken order sell BTCUSD 0.01 --type market` |
| `kraken paper status` | `kraken balance` + `kraken open-orders` |
| `kraken paper orders` | `kraken open-orders` |
| `kraken paper history` | `kraken trades-history` |
| `kraken paper cancel <ID>` | `kraken order cancel <TXID>` |

### Futures

| Paper | Live |
|-------|------|
| `kraken futures paper buy PF_XBTUSD 1 --leverage 10 --type market` | `kraken futures order buy PF_XBTUSD 1 --type market` |
| `kraken futures paper sell PF_XBTUSD 1 --leverage 10 --type market` | `kraken futures order sell PF_XBTUSD 1 --type market` |
| `kraken futures paper positions` | `kraken futures positions` |
| `kraken futures paper orders` | `kraken futures open-orders` |
| `kraken futures paper fills` | `kraken futures fills` |
| `kraken futures paper cancel --order-id <ID>` | `kraken futures cancel --order-id <ID>` |
| `kraken futures paper cancel-all` | `kraken futures cancel-all` |

**Leverage note:** Paper accepts `--leverage` inline on buy/sell commands. Live futures configures leverage separately via `kraken futures set-leverage <SYMBOL> <LEVERAGE>` before placing orders.

## Gradual Promotion

Start with smaller size than paper:

1. **Paper size**: the volume used during testing.
2. **Initial live size**: 10-25% of paper size.
3. **Scale up**: increase gradually after confirming live behavior matches paper.

## Live Session Safety

After going live, maintain these controls:

- Dead man's switch refreshed periodically.
- Balance check after every trade.
- Open orders verified after every placement.
- Error handling active for all error categories.
- Maximum loss threshold that triggers session shutdown.

## Rollback

If live behavior diverges from paper:

### Spot
1. Cancel all open spot orders once the user approves (`--yes` skips the CLI prompt):
   ```bash
   kraken order cancel-all --yes -o json 2>/dev/null
   ```
2. Assess balances.
3. Return to `kraken paper` to debug.

### Futures
1. Cancel all open futures orders once the user approves (`--yes` skips the CLI prompt):
   ```bash
   kraken futures cancel-all --yes -o json 2>/dev/null
   ```
2. Close open positions with `--reduce-only`.
3. Return to `kraken futures paper` to debug.

## Hard Rules

- Never promote without explicit user sign-off.
- Never promote without the promotion evidence block: ≥2 passing sessions of a
  seal-verified experiment, a passing **live** run among them, caveats
  attached. Refusals name exactly which sessions or verdicts are missing.
- Start at reduced size.
- Always validate live spot orders with `--validate` before executing. Futures orders have no dry run, so each one needs explicit user approval.
- Maintain dead man's switch throughout the live session.
- First live session should run at autonomy level 3 (supervised) regardless of prior paper autonomy.
- If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
