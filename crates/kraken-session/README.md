# kraken-session

The on-disk session contract and the timeline read model.

## Purpose

A session is a named directory `<base>/sessions/<name>/` holding a market
recording, an isolated paper-account journal (`events.jsonl`), a decision log
(`decisions.jsonl`), and a versioned manifest (`manifest.json`). This crate
owns that contract — the validated `SessionName`, the path layout, the
manifest MAJOR gate, the decision writer — plus the one read model over it:
`timeline::read`, merging all three tracks into a single `(at, track, seq)`
ordered event stream.

## Architecture

Repository + read model (CQRS-lite). The write side (provisioning, stop
finalization) stays in the binary; this crate reads. Every CLI seam is a
feature or a parameter: `base: &Path` instead of config-dir resolution,
`cli_version` as an argument instead of a telemetry global, clap derives
behind the `clap` feature, the DuckDB read path behind `duckdb`.

- `lib.rs` — `SessionName`, path layout, `MANIFEST_VERSION` gate
- `manifest.rs` — the durable session description, `load`/`save`
- `decision.rs` — the "why" track and its locked appender
- `timeline.rs` / `timeline/event.rs` — the three-track merge and its event
  vocabulary; journal-accurate `fixtures` for downstream tests

## Interface

What crosses upward: `SessionName`, the path fns (`sessions_root`, `dir`,
`events_path`, `manifest_path`, `decisions_path`, `ensure_provisioned`),
`SessionManifest`/`SessionOutcome`/`StrategyRef`, `DecisionKind` and
`decision::append`, `timeline::read` → `SessionTimeline`/`TimelineEvent`/
`EventPayload`/`Track`/`final_epoch_start`, and `SessionError`.

What never enters: `config_dir()` resolution, clap (outside the feature),
telemetry.

## Errors

`SessionError` — `Rejected` (→ envelope `validation`), `IncompatibleManifest`
(→ `config`), `Damaged` (→ `parse`), `Recording` (transparent over
`kraken_recording::Error`), `Io` (→ `io`). Bridged once in the binary's
`src/errors.rs`.

## Features

| feature    | enables                                            | default |
|------------|----------------------------------------------------|---------|
| `clap`     | `clap::ValueEnum` on `DecisionKind`                 | off     |
| `duckdb`   | the DuckDB timeline read path                       | off     |
| `fixtures` | `timeline::fixtures` for downstream crates' tests   | off     |

All strictly additive.

## Testing

Manifest round-trip and MAJOR-gate pins, merge/lock/damage suites, and the
`tests/session_roundtrip.rs` flow through the public API (plus a
`duckdb`-gated sibling). Feature-matrix proof:

```text
cargo test -p kraken-session
cargo test -p kraken-session --all-features
cargo check -p kraken-session --no-default-features
```
