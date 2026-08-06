# kraken-paper

The event-sourced spot paper-trading engine.

## Purpose

Owns the money rules of spot paper trading: order validation and the fill
state machine (`PaperState` and its `decide_*`/`apply` pair), exact-`Decimal`
valuation, and the journal-backed account store (`PaperAccount` over
`events.jsonl`). It defends one boundary: everything above it — CLI flags,
config directories, sessions, live prices — stays out.

## Architecture

Ports & Adapters. Prices are **arguments** (`decide_market_order(side, pair,
volume, ask, bid)`), never a market-data dependency; persistence is composed
from kraken-recording's `Sink`/`Source` pair; where a journal lives on disk is
the caller's policy (`PaperAccount::open_at` takes the path). State is a fold:
`decide_*` validates and computes without mutating, `apply` folds recorded
events back into state, so a replayed journal always reproduces the live
account exactly.

- `lib.rs` — engine: money guards, pair parsing, order/trade vocabulary,
  `PaperState`
- `account.rs` — the journal-backed store: locked open, durable-first appends,
  the replay `Projection`

## Interface

What crosses upward: `PaperState` (`apply`, `valuation`, `decide_*`),
`PaperConfig`, `PaperTrade`, `PaperOrder`, `OrderSide`, `AccountEvent`,
`account::{PaperAccount, AccountRecord, Origin, RECORD_VERSION}`,
`ReferenceQuote`, `parse_pair`, `canonical_key`, `valid_quote`, the money
constants (`DEFAULT_FEE_RATE`, `DEFAULT_SLIPPAGE_RATE`, `BALANCE_DUST`,
`MAX_MONEY`), and `PaperError`.

What never enters: clap, config-dir resolution, session naming, network I/O.

## Errors

`PaperError` — `Rejected` (refused up front; → envelope `validation`),
`Incompatible` (journal from a newer build; → `config`), `Journal` (storage
failure, transparent over `kraken_recording::Error`; → its categories).
The binary bridges these to the JSON envelope exactly once, in `src/errors.rs`.

## Testing

Exact-Decimal golden tests for every fee/fill/valuation rule, journal
round-trips on `tempfile`, and `tests/account_lifecycle.rs` driving the full
open → trade → cancel → reload flow through the public API only.

```text
cargo test -p kraken-paper
```
