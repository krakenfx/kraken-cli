# kraken-recording

Durable persistence for recorded events: market frames, the paper-trading journal, and (planned) trade actions.

## Overview

A symmetric storage layer: every domain pairs a `Sink` (write) with a `Source` (read), and what was written is exactly what reads back. The core is generic — any `Serialize`/`Deserialize` type records through `JsonlSink<T>`/`JsonlSource<T>` — and the market domain adds capture contracts over two backends, JSONL and DuckDB.

## Quick Start

### Recording market frames

```rust
use kraken_recording::{CaptureSink, DatasetSink, Sink};

let mut sink = DatasetSink::open(&path, &declaration)?;
sink.record(&frames)?;
sink.finalize(summary)?;
```

### Reading them back

```rust
use kraken_recording::{CaptureSource, DatasetSource, Source};

let source = DatasetSource::open(&path)?;
let report = source.capture()?; // version-gated self-description
for event in source.read()? {
    println!("{:?}", event.frame);
}
```

With the `duckdb` feature, `DuckdbSink`/`DuckdbSource` drive the columnar backend through the same `CaptureSink`/`CaptureSource` contracts — the two backends yield identical event sequences for one capture.

### A generic event log

```rust
use kraken_recording::{JsonlSink, JsonlSource, Sink, Source};

let mut sink: JsonlSink<MyEvent> = JsonlSink::create(path.clone(), "my event log")?;
sink.record(&events)?;
drop(sink); // releases the writer lock

let replayed: Vec<MyEvent> = JsonlSource::open(path)
    .expect("log exists")
    .read()?
    .collect();
```

## Architecture

Three layers, dependency arrows pointing down only:

1. **Domains** — `frames/` (market data, complete — including the capture contracts `CaptureSink`/`CaptureSource`), `paper/` (the account journal, in use), `actions/` (planned)
2. **Contracts** — the generic `Sink<T>`/`Source<T>` pair every domain builds on
3. **Engine** — the locked JSONL log (torn-tail repair, fsync'd appends), advisory locks, schema versioning

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full design, including the honestly-stated crash contract.

## Guarantees

- **Symmetric**: write→finalize→read round-trips byte-identically (JSONL) or semantically identically (DuckDB)
- **Fail loud**: an undecodable committed line or row fails the read; only a crash's torn tail and a newer-minor writer's vocabulary are tolerated
- **Single writer**: writers hold an exclusive advisory lock; concurrent opens are rejected "in use", never interleaved
- **Versioned**: `MAJOR.MINOR` stamps; MAJOR mismatches are rejected at open in both backends

## DuckDB Backend

Enable with the `duckdb` feature (off by default; it pulls in the bundled libduckdb C++ build):

```toml
kraken-recording = { version = "0.3", features = ["duckdb"] }
```

## Testing

```bash
cargo nextest run -p kraken-recording --all-features
```

Unit, round-trip, backend-equivalence, crash-artifact, and property tests; on-disk shapes are pinned so protocol drift surfaces as a failing test.
