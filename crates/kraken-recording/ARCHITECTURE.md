# kraken-recording Architecture

Durable persistence for recorded events: market frames today, the paper-trading
journal, and (planned) trade actions.

## Principles

1. **Symmetric** — every domain pairs a `Sink` (write) with a `Source` (read).
   What you write is exactly what you read back.
2. **Domain-free core** — the `Sink<T>`/`Source<T>` traits and the JSONL log
   engine know nothing about markets. Domains build on the core; the core never
   imports a domain.
3. **One name per type** — `JsonlSink`/`JsonlSource` are the generic pair;
   `TapeSink`/`TapeSource` the market JSONL backend; `DuckdbSink`/`DuckdbSource`
   the market columnar backend.
4. **Fail loud on damage** — an undecodable committed line or row fails the
   read. The only tolerated states are a crash's torn tail and a newer-minor
   writer's vocabulary.

## Module Layout

```text
kraken-recording/
├── sink.rs              ← Sink trait + JsonlSink<T> (any Serialize type → JSONL)
├── source.rs            ← Source trait + JsonlSource<T> (JSONL → any Deserialize type)
├── jsonl.rs             ← Log engine: locked appends, torn-tail repair, committed-line reads
├── lock.rs              ← Advisory file locking (exclusive writers, shared readers)
├── schema.rs            ← Version stamp and MAJOR/MINOR compatibility rule
├── error.rs             ← Rejected / Damaged / Engine / Io failure taxonomy
├── fsync.rs             ← Directory-entry durability for creates and renames
├── time.rs              ← RFC3339 instant parsing shared by everything that orders events
├── tape.rs              ← TapeRef grammar and the read-only catalog behind `kraken tape list`
│
├── frames/              ← Market data (ticker, trade, book, ohlc)
│   ├── mod.rs           ← CaptureSink/CaptureSource, MarketEvent, merge-instant rules
│   ├── capture.rs       ← RecordingManifest/Declaration/Integrity, sidecar I/O, frame gate
│   ├── jsonl/           ← TapeSink/TapeSource: frames + .meta.json sidecar
│   └── duckdb/          ← DuckdbSink/DuckdbSource (feature `duckdb`)
│       ├── table.rs     ← TableSpec: DDL/INSERT/SELECT generated from one column list
│       └── tables/      ← Recordable codec per channel (ticker, trade, book, ohlc)
│
├── paper/               ← Paper-trading account journal: journal()/replay() over the core
└── actions/             ← Trade actions (placements, fills) — planned, unimplemented
```

## Event Domains

### frames: Market Data — complete

- **Write**: `TapeSink` (frames → JSONL + sidecar) or `DuckdbSink` (→ columnar
  tables)
- **Read**: `TapeSource` / `DuckdbSource`, both yielding `MarketEvent`s in
  capture order
- **Contract**: `CaptureSink::finalize` stamps the completeness summary;
  `CaptureSource::capture` hands it back before any frame decodes
- Both backends must yield identical event sequences for one capture, pinned by
  backend-equivalence tests (including across a crash-and-resume)

### paper: Account Journal — in use

The paper account is its append-only journal; state is derived by folding events
in order. The event types live with the simulator, so the domain exposes the
generic pair under the journal's contract: `paper::journal()` (locked writer)
and `paper::replay()`. A columnar backend for querying trading history is
planned.

### actions: Trade Actions — planned

Order placements, cancellations, and fills. Will mirror the `frames/` shape once
the event types are settled.

## Symmetry Contract

```rust
// Write
let mut sink = TapeSink::open(&path, &declaration)?;
sink.record(&frames)?;
sink.finalize(summary)?;

// Read (exact same data)
let source = TapeSource::open(&path)?;
let report = source.capture()?;             // version-gated self-description
let events: Vec<_> = source.read()?.collect();
```

`Source::read` validates the whole track before yielding the first item, which
deliberately costs O(track) memory — no consumer ever observes a partial stream.

## Schema & Versioning

- **Version**: `MAJOR.MINOR` (`schema::SCHEMA_VERSION`), stamped by the first
  writer; a same-MAJOR reopen appends without downgrading the stamp
- **MAJOR mismatch**: rejected at open, for reads and appends, in both backends
- **Newer MINOR**: readable. Vocabulary this build can't decode (an unknown
  channel line, an unknown enum value) is skipped with a warning — whole frames,
  never partial ones. Under the build's own stamp the same content is damage and
  fails loud. One rule, both backends.
- **Location**: the `.meta.json` sidecar (JSONL) and the `_meta` table (DuckDB).
  A partial completeness stamp — `window_end` without its loss counts, or the
  reverse — is damage in both.

## Crash Contract

Stated honestly; the tests pin exactly this, no more:

1. **Durability on return**: an append is one `write_all` + fsync; when `record`
   returns, the batch is on disk. File creation and sidecar renames also fsync
   the parent directory, so a power loss cannot roll them back.
2. **Torn tail**: a crash mid-append can leave a fragment after the last
   newline; the next append truncates it, readers ignore it.
3. **No batch atomicity**: a crash mid-append may persist any prefix of a
   multi-line batch (and, on out-of-order page writeback, a hole inside one —
   the reader fails loud on that). Consumers needing multi-event atomicity
   encode the transaction as a single line.
4. **Unfinalized runs**: a reopen clears the previous summary; a crash reads
   back as `summary: None` (completeness unknown), never as the old run's
   finalized stamp.
5. **Empty log, whatever the sidecar says**: zero committed bytes means zero
   recorded frames — the dataset bootstraps fresh rather than adopting an
   orphaned sidecar's stale identity.

## Locking

Writers hold an exclusive advisory lock on a `<path>.lock` sidecar for the
sink's lifetime; readers hold it shared. A second writer, or a reader racing a
live writer, is rejected up front ("in use"), never queued. `is_lock_held`
probes recorder liveness without ever reading a reader as a recorder. See
`lock.rs` for the honestly-documented inode race and why it is accepted.

## Testing Strategy

- **Unit tests** per module: table codecs, frame gating, sidecar strictness,
  lock semantics
- **Round-trip tests**: write→finalize→read symmetry per backend, pinned on-disk
  shapes guarding serde drift
- **Backend equivalence**: DuckDB and JSONL yield identical events and capture
  reports, including across a crash
- **Crash artifacts**: torn tails (also spanning the scan window), unfinalized
  reopens, orphaned sidecars, half stamps
- **Property tests**: torn-tail repair truncates exactly to the committed
  prefix; the merge-instant clamp never reorders or rewinds
