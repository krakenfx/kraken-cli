---
name: kraken-lab-experiment
version: 2.0.0
description: "Run the Autoresearch Lab lifecycle end-to-end: freeze a hypothesis with mechanical success criteria and a sealed session plan, drive recorded and replayed paper runs, score them honestly, and read the verdict — resumable from disk at every step, designed for /loop."
metadata:
  openclaw:
    category: "lab"
    domain: "sessions"
  requires:
    bins: ["kraken", "jq"]
    skills: ["kraken-playground"]
---

# Lab Experiment Lifecycle

> **PREREQUISITE:** Load `kraken-playground` to understand what a session is
> and where it lives.

Turn a trading hunch into a mechanical verdict: pre-register the hypothesis
and its success criteria (hash-sealed, so the goalposts cannot move), run it
as recorded paper runs, score each session against its own tape, and let the
frozen criteria — not vibes — say pass or fail. Every step reads and writes
plain files under the active scope, so the lifecycle survives kills,
restarts, and context loss: **whatever step you find half-done on disk is
the step you resume**.

Use this skill for:

- driving one experiment end-to-end under `/loop` (freeze → run → score →
  verdict)
- resuming an experiment a previous context started
- reproducing a verdict cold, from files alone

## Safety rules

- Paper only: run experiments inside a paper workspace
  (`export KRAKEN_WORKSPACE=<name>`), where the order verbs fill on the
  paper account and no real funds are reachable.
- Never edit a frozen experiment file. The seal exists to catch exactly
  that; a tampered spec fails every later step with a `parse` envelope.
- Never report a verdict you did not obtain from `lab score --experiment`
  or `lab compare` output in this session. No estimating, no "it would
  probably pass".

## The state machine

With a sealed session plan the whole table collapses to one command — **run
`kraken lab next <exp>` and do what it says**:

```bash
kraken lab next momentum-1 -o json 2>/dev/null
# state: "start"         → run the exact `command` it prints, then stop+score
# state: "running"       → drive or wait, then `kraken session stop` + `lab score`
# state: "plan_complete" → `kraken lab compare <exp>`, conclude (Step 5)
```

`lab next` re-derives progress from the sessions tree on every call: completed
runs consume plan entries in order, aborted sessions keep their ordinals and
count for nothing, and a replay entry whose tape no longer matches its
sealed hash is refused outright. Stop and score a finished run *before*
asking again — an unstopped run has no summary and reads as aborted.

For a plan-less experiment (registered before run plans existed), fall back
to the manual table:

| State | Evidence | Next action |
|-------|----------|-------------|
| unregistered | `kraken lab show <exp>` → `validation` error | Step 1: freeze |
| no run yet | `kraken session list` → no run stamped with the experiment | Step 2: start run |
| run live | `kraken session show` → status `recording` | Step 3: drive or wait, then stop |
| run stopped, unjudged | status `stopped`, no verdict recorded in your notes | Step 4: score + verdict |
| judged | verdict obtained | Step 5: conclude or start run n+1 |

## Step 1: Freeze the hypothesis (once)

Pick a name (path-safe segment: letters, digits, `.`, `-`, `_`) and state
the falsifiable claim plus at least one mechanical criterion:

```bash
kraken lab new momentum-1 \
  --hypothesis "buying 15m strength beats sitting in cash" \
  --strategy recipe-playground-dca \
  --min-return-pct 1 --max-drawdown-pct 5 --min-fills 2 \
  --session replay:tape:jun-crash --session replay:tape:jun-chop \
  --session live:24h \
  -o json 2>/dev/null
```

Seal the **session plan** with the spec: repeatable `--session` entries, in
execution order — `replay:<ref>` (a finalized tape from
`kraken tape list`; its content hash is sealed, so a re-recorded tape
can never satisfy the plan) or `live:<window>`. Replay first to screen
cheaply, live last: the promotion gate requires a passing live session because
replay cannot enforce lookahead. The session count is sealed too — that is the
point: no running until a pass appears.

The output echoes the sealed spec with its `frozen` hash (`sha256:…`).
Freezing is once-only: a second `lab new` under the same name is a
`validation` error saying the spec is immutable. Verify any time with:

```bash
kraken lab show momentum-1 -o json 2>/dev/null
```

If `show` returns a `parse` error mentioning "modified after freezing", the
file was tampered with. Stop the experiment and report it — do not re-freeze
over it.

## Step 2: Start a session

With a session plan, `kraken lab next <exp>` prints the exact start command —
use it verbatim; adjusting `--speed` per the reaction-time note below is
the one permitted edit. Runs are ordinary recorded sessions stamped with
`--experiment` — the stamp is what discovery keys on; the `--label
<exp>-s<n>` is the human handle:

```bash
# live entry (fill in the market to record):
kraken session start --label momentum-1-s1 --experiment momentum-1 \
  --symbols BTC/USD --channels ticker,trade \
  -o json 2>/dev/null &

# replay entry (the tape defines the market; 10x compresses the wait):
kraken session start --label momentum-1-s2 --experiment momentum-1 \
  --from tape:jun-crash --speed 10 -o json 2>/dev/null &
```

Replay reaction-time note: at speed N you have 1/N of live reaction time —
suits slow-cadence strategies (DCA); drop to `--speed 1` for reactive ones.

(At least one `--channels` entry is required on live entries, and the
recorder runs in the foreground, so background it with `&` as the
playground skill does. The `session_started` stdout line carries the allocated
session id.)

Drive the strategy the hypothesis names (e.g. the recipe skill from
`--strategy`), passing `--reason` on every trade so the scorecard's
decided-fill join has data — the rationale lands in the active session's
decision log automatically.

## Step 3: Stop the session

A session must stop before it can be scored — the stop summary is the anchor
every later number decomposes:

```bash
kraken session stop
```

## Step 4: Score and judge

One command produces the honest scorecard *and* the mechanical verdict:

```bash
kraken lab score --session latest --experiment momentum-1 -o json 2>/dev/null
```

- `source`: what the session traded against — `{kind:"replay", dataset, speed}`
  or `{kind:"live", symbols}`, derived from the session's contract, not
  asserted.
- `anchor` + `components[]`: the stop totals and the explain-pnl waterfall,
  carried verbatim — this output can never disagree with
  `kraken explain pnl`.
- `metrics`: `return_pct`, `max_drawdown`, `max_drawdown_pct`, `fees`,
  `friction`, `turnover_pct`, `hit_rate`, `profit_factor`,
  `slippage_sensitivity` — every `null` is an honest "not derivable", never
  zero (a buys-only DCA run has no closed round trips, so `hit_rate` and
  `profit_factor` read `null` by design, not by breakage).
- `verdict.pass` and `verdict.checks[]`: one check per frozen criterion with
  `threshold`, `observed`, `pass`. An unknown observation fails its check.
- `caveats[]`: read them; a verdict with caveats is reported *with* them.

Digest for the loop log:

```bash
kraken lab score --session latest --experiment momentum-1 -o json 2>/dev/null \
  | jq '{session, experiment, pass: .verdict.pass,
         checks: [.verdict.checks[] | {criterion, threshold, observed, pass}],
         caveats}'
```

The retry cues are in the error envelopes themselves: "still recording — stop
it with 'kraken session stop' first" means go back to Step 3; "was aborted
before it stopped … only a stopped session can be scored" means that session
is dead evidence — start a fresh one; "experiment '…' not found; run 'kraken
lab new …' first" means Step 1 never happened in this scope.

## Step 5: Conclude — or run again

The verdict is mechanical; the conclusion is yours to *report, not adjust*:

- **pass**: state the hypothesis, the checks that carried it, and every
  caveat. One passing run is evidence, not proof — follow the plan to its
  end before promoting anything.
- **fail**: name the failing checks (`threshold` vs `observed`) and what the
  waterfall says the money actually went to (fees? drawdown? never traded?).
  A failed experiment is a completed experiment.

**Promotion:** when `lab next` says `plan_complete`, assemble the promotion
evidence block from `kraken lab compare` and hand off to the
`kraken-paper-to-live` skill. Its gate is mechanical: at least 2 passing
runs with a passing **live** run among them — replay verdicts count only
with their lookahead caveat attached. Two replay passes never promote.

More runs: repeat Steps 2–4 (each start allocates the next ordinal), then
read every session side by side:

```bash
kraken lab compare momentum-1 -o json 2>/dev/null
```

One verdict column per session (`sessions[].verdict`), plus `pass_count`/`total`
and a named error column for any run that cannot be scored (a still-running
run says so — stop it first). Only the verdicts compare across runs: they
cover different market windows, so the metric rows are per-session context.
Never average scorecards across runs into a blended number the CLI did not
produce — `compare` deliberately never aggregates.

## Cold-context verdict reproduction

Any context — including one that never saw the sessions — reproduces a verdict
from disk alone:

```bash
kraken lab show momentum-1 -o json 2>/dev/null          # the sealed criteria
kraken lab score --session s1 --experiment momentum-1 -o json 2>/dev/null
```

Same files, same exact-Decimal math, same verdict, byte for byte. If the two
disagree with a previously reported verdict, something on disk changed —
`lab show` tells you whether it was the spec.

## Running under /loop

Invoke as `/loop` with this skill and the experiment name. Each firing:

1. `kraken lab show <exp>` — missing → freeze (Step 1, with `--session`
   entries) and return.
2. `kraken lab next <exp> -o json` and act on its `state`. If it refuses
   with a no-run-plan `validation` error (an experiment frozen before run
   plans existed), drive the manual state table above instead.
   `start` → run its `command`; `running` → drive/wait, then stop + score
   (Steps 3–4) and log the digest; `plan_complete` → `kraken lab compare`,
   conclude (Step 5), and stop the loop with the per-session verdicts (and the
   promotion evidence block, if concluding toward promotion) as the final
   report.

Idempotent by construction: a firing that finds nothing to advance does
nothing. A killed loop resumes at the same step.

If you hit a mismatch between what you are trying to do and the CLI's interface or responses — including a mismatch between this skill and the installed CLI version's contract — feel free to submit feedback with `kraken feedback`.
