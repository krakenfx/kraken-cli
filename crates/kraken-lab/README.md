# kraken-lab

The Autoresearch Lab's application layer: the [`Lab`] facade plus the read
models and experiment store beneath it.

## Purpose

Turn a recorded session into evidence: fold it into the two intermediates
every lab metric derives from — the marked equity curve and the fill ledger
(`replay()` → `Replay`) — score a stopped session against them (`score()` →
`Scorecard`), and judge that scorecard against a hypothesis frozen *before*
the run (`Criteria::evaluate` → `Verdict`). Valuation is against the session's
own recorded market track via `mark::Table`, never live prices and never the
stop anchor, so the same session folds to the same numbers forever.

## Architecture

One entry point, the **`Lab` facade** — the binary dispatches to it and does
nothing else lab-related. Each command resolves the config dir and session,
calls one `Lab` method (`freeze`/`show`/`score`/`compare`/`next`), and renders
the returned model; all orchestration lives behind the facade. Beneath it is a
**functional core, imperative shell**: the core computes, the shell is the
only code that touches the disk or the clock. `store` and the planner are
`pub(crate)`, so the seam is enforced by the compiler — the binary reaches them
only through `Lab`.

**Functional core — pure, deterministic, no I/O, no clock:**

- `mark` — the rolling mark `Table` (`observe`, `to_prices`): a left fold with
  per-symbol median robustness, so a timeline prefix always yields a curve
  prefix.
- `replay` — the curve fold: `replay()`, `Replay`, `Curve`, `Point`.
- `score` — the honest scorecard: carries the explain-pnl anchor and waterfall
  **verbatim** (the can't-disagree contract) plus mechanical metrics (return,
  drawdown, trade counts).
- `trades` — FIFO round-trip attribution over the fill ledger.
- `experiment` — the sealed-spec domain: `Experiment`/`Criteria`/`Verdict`,
  the run plan (`RunPlan`/`PlannedRun`), `seal`/`verify` (hash a spec's
  canonical bytes; recompute on load so a post-freeze edit reads as damage),
  and `Criteria::evaluate`.
- `naming` — the run-session grammar: `run_id`/`is_run_of`/`parse_run` and
  `RunRef`.
- `plan` — the pure planner: `next_action` derives the next step from a sealed
  plan and the facade's `RunObservation`s. Never stored — always re-derived.
- `window` — the `20s`/`5m`/`1h` run-window grammar, shared with the
  playground's `--duration` so a sealed live leg can never be one it rejects.
- `hash` — the `sha256:<hex>` vocabulary shared by the seal and the tape pin.

**Imperative shell — the one filesystem-and-clock seam:**

- `store` — `freeze` (stamps the clock, seals, writes atomically), `load`
  (reads, then delegates the seal check to `experiment::verify`), and
  `discover_runs` (the run-discovery seam with manifest-link validation).
  File-backed state makes every verdict reproducible from the store.

The core depends only on `kraken-session`/`kraken-replay` types; the shell adds
`std::fs` and `chrono::Utc::now`. `sha256_file` stays a free function: it
hashes arbitrary replay tapes under `recordings/`, which the store does not
own.

## Interface

The command seam is `Lab` — `new`, `freeze`, `show`, `score`, `compare`,
`next`, `ensure_planned_source` — returning `ScoredRun`/`Comparison`/
`NextAction`/`Experiment`. Its result and domain types cross upward too:
`Scorecard`/`Metrics`/`TradeStats`/`CurveSummary`, `Experiment`/`Criteria`/
`Verdict`/`Check`/`RunPlan`/`PlannedRun`, `RunOutcome`/`OutcomeResult`/`Judged`,
`RunRef`/`RunObservation`/`RunStatus`/`NextState`, and `LabError`. The read
models stay public for reuse (`replay()`/`Replay`/`Curve`/`Point`,
`mark::Table`, `score()`) alongside `run_id`/`is_run_of`/`parse_run`,
`parse_window`, `sha256_file`, and `experiments_root`; the store and planner
functions are `pub(crate)` behind the facade. The serde response structs are
the command contract — clap-free by construction.

## Errors

`LabError` — `NoEpoch`/`ExperimentNotFound`/`ExperimentExists`/`Spec`
(→ envelope `validation`), `Damaged` (a tampered or undecodable frozen spec;
→ `parse`), `Store` (→ `io`), and the transparent `Recording`/`Session`/`Replay`
bridges over the lower crates' errors. `LabError::category()` mirrors the wire
codes so a failed `compare` column can label itself; the binary's `errors`
module owns the authoritative envelope mapping.

## Testing

Fold determinism (byte-identical on a re-fold) and prefix/no-lookahead
proptests; the mark table's median, latch, and canonicalization pins; the
golden spec-hash pin (a serde drift breaks every seal on disk, and fails here
first); the seal/`verify` round-trip and tamper/rename refusals; the run-name
grammar round-trip; and `tests/*.rs` driving fixture sessions through the
public API only.

```text
cargo test -p kraken-lab
```
