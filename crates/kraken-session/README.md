# kraken-session

The on-disk session contract and the timeline read model.

## Purpose

A session is a recorded window over a scope's shared paper-account journal,
stored as `<scope>/sessions/s<n>/`: the session contract (`session.json`), a
market tape (`tape.jsonl` or `tape.duckdb`), a decision log (`decisions.jsonl`),
and, once an agent writes one, a state cell (`state.json`). This crate owns that
contract — the `s<n>` ordinal grammar, the path layout, the manifest MAJOR gate,
the decision writer — plus the read model over it: `session::read_window`,
merging the journal window, the decisions, and the tape into a single
`(at, track, seq)` ordered event stream.

The older layout, a named directory `<base>/sessions/<name>/` with its own
`events.jsonl` and `manifest.json` read by `timeline::read`, is kept as a legacy
API.

## Architecture

Repository + read model (CQRS-lite). The write side (provisioning, stop
finalization) stays in the binary; this crate reads. Every CLI seam is a feature
or a parameter: `base: &Path` instead of config-dir resolution, `cli_version` as
an argument instead of a telemetry global, clap derives behind the `clap`
feature, the DuckDB read path behind `duckdb`.

- `lib.rs` — `SessionError`, the `MANIFEST_VERSION` gate, `SessionName` (the
  shared artifact-name grammar), and the legacy named-session layout (`dir`,
  `manifest_path`)
- `session.rs` — the `s<n>` layout, `read_window`/`read_account_window`, the
  decision and state readers
- `manifest.rs` — the durable session description, `load`/`save`
- `decision.rs` — the "why" track and its locked appender
- `timeline.rs` / `timeline/event.rs` — the three-track merge and its event
  vocabulary; journal-accurate `fixtures` for downstream tests

## Interface

What crosses upward: the `session` module (`sessions_root`, `session_dir`,
`session_file_path`, `parse_ordinal`, `tape_file`, `read_window` over
`SessionTracks`, `read_account_window`, `read_decisions`,
`read_state`/`write_state`), `SessionManifest`/`SessionOutcome`/`StrategyRef`,
`DecisionKind` and `decision::append`, `SessionTimeline`/`TimelineEvent`/
`EventPayload`/`Track`/`final_epoch_start`, `SessionName`, which recipe, lab
experiment and plugin names are validated against, and `SessionError`. The
legacy named-session API (`dir`, `events_path`, `manifest_path`,
`decisions_path`, `ensure_provisioned`, `timeline::read`) is still public.

What never enters: `config_dir()` resolution, clap (outside the feature),
telemetry.

## Errors

`SessionError` — `Rejected` (→ envelope `validation`), `IncompatibleManifest` (→
`config`), `Damaged` (→ `parse`), `Recording` (transparent over
`kraken_recording::Error`), `Io` (→ `io`). Bridged once in the binary's
`src/errors.rs`.

## Features

| feature    | enables                                           | default |
| ---------- | ------------------------------------------------- | ------- |
| `clap`     | `clap::ValueEnum` on `DecisionKind`               | off     |
| `duckdb`   | the DuckDB timeline read path                     | off     |
| `fixtures` | `timeline::fixtures` for downstream crates' tests | off     |

All strictly additive.

## Testing

Manifest round-trip and MAJOR-gate pins, merge/lock/damage suites, and
`tests/session_window.rs`, which pins the windowed reader's anchor and journal
slice through the public API. Feature-matrix proof:

```text
cargo test -p kraken-session
cargo test -p kraken-session --all-features
cargo check -p kraken-session --no-default-features
```
