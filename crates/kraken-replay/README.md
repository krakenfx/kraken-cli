# kraken-replay

Session projections: the explain-pnl decomposition and paced playback.

## Purpose

Two pure consumers of a recorded session timeline. `explain_pnl` decomposes a
stopped session's P&L into plain-terms components — anchored on the totals
`playground stop` wrote, with the identity `price_movement + fees + spread +
slippage + residual = total_pnl` holding exactly by construction.
`replay_events` plays a timeline back into a caller-owned sink on the
recorded gaps divided by a speed factor.

## Architecture

Projections over `kraken_session::SessionTimeline` — no I/O construction, no
network, no clap. Playback takes a caller-owned `&mut impl Write` and a
**pure** renderer `Fn(&TimelineEvent) -> Result<String, serde_json::Error>`;
locking stdout and choosing the render format stay at the call site. The
`PlaybackClock` anchors absolute deadlines, so an emission that falls behind
flushes back-to-back instead of re-pacing.

- `explain.rs` / `explain/report.rs` — the decomposition and its serialized
  vocabulary
- `playback.rs` — `PlaybackClock`, `replay_events`, the speed bounds

## Interface

What crosses upward: `explain_pnl(&SessionTimeline) -> Result<PnlReport>`,
the report vocabulary (`PnlReport`, `Anchor`, `ComponentLine`,
`ComponentKind`, `TradeLine`, `MissedFillSection`, `OriginSlice`,
`WindowSummary`, `SymbolWindow`, `EndMarkSource`, `Coverage`),
`PlaybackClock`, `replay_events`, `MIN_SPEED`/`MAX_SPEED`, and `ReplayError`.

`PnlReport`'s serialized shape is a **public contract** (agents consume it
over MCP); fields are additive-only.

## Errors

`ReplayError` — `NotStopped` (no final summary to anchor on; → envelope
`validation`, hint byte-stable), `Emit` (sink failure; → `io`), `Render`
(→ `parse`). Bridged once in the binary's `src/errors.rs`.

## Testing

The full explain suite including proptests and the `PnlReport` serialization
pin; playback pacing under paused tokio (no real sleeps); the
BrokenPipe-is-a-clean-end pin; and `tests/replay_pipeline.rs` driving a
fixture session end-to-end through the public API only.

```text
cargo test -p kraken-replay
```
