//! The write side: the domain-free [`Sink`] contract and its generic JSONL
//! backend [`JsonlSink`]. Deliberately free of any event domain — any
//! `Serialize` entry type — so every recorded track shares one write
//! contract; domain extensions (e.g. the market capture's
//! `CaptureSink`) live with their domains under [`frames`](crate::frames).

use crate::{error::Result, jsonl::Log};
use serde::Serialize;
use std::{marker::PhantomData, path::PathBuf};

/// The write side of a recorded track: a destination that persists (or
/// renders) batches of entries. Generic over the entry type so every log
/// shares one write contract instead of a bespoke implementation per file.
///
/// Consumed **by value** — drivers are generic over the concrete sink, never
/// `Box<dyn Sink>`. Driven from a single dedicated blocking thread, so the
/// methods are synchronous: the async boundary is the bounded channel in
/// front of the thread, not the sink.
///
/// The defaults encode the layer's baseline: every current backend is
/// durable-per-batch on a clean return (fsync'd log append, DB transaction,
/// flushed stdout write), so [`flush`](Self::flush) has nothing to do and
/// [`close`](Self::close) only flushes before resources release on drop.
/// A backend that buffers across `record` calls must override `flush`;
/// one with end-of-life work beyond flushing must override `close`.
pub trait Sink {
    type Item;

    fn record(&mut self, batch: &[Self::Item]) -> Result<()>;

    /// Make everything recorded so far durable (or visible).
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }

    /// Flush, then release resources by consuming the sink. Stamping a
    /// stream's capture summary is the market layer's concern
    /// ([`CaptureSink::finalize`](crate::frames::CaptureSink::finalize)).
    fn close(mut self) -> Result<()>
    where
        Self: Sized,
    {
        self.flush()
    }
}

/// The generic JSONL sink — the [`Sink`] view of the crate's log engine.
///
/// One fsync'd JSONL line per entry, for any `Serialize` entry type, so any
/// log reuses it instead of growing its own write path. Deliberately
/// domain-free; the market tape variant with frame filtering and the
/// metadata sidecar is [`TapeSink`](crate::frames::jsonl::TapeSink).
#[derive(Debug)]
pub struct JsonlSink<T> {
    log: Log,
    _entry: PhantomData<T>,
}

impl<T> JsonlSink<T> {
    /// Open the log at `path` (creating it if absent) and hold its
    /// single-writer lock for the sink's lifetime. `purpose` names the
    /// holder in the busy-log error a contender sees.
    pub fn create(path: PathBuf, purpose: impl Into<String>) -> Result<Self> {
        Ok(Self {
            log: Log::open(path, purpose)?,
            _entry: PhantomData,
        })
    }

    /// Delete the log under its lock, consuming the sink. Already absent
    /// counts as success.
    pub fn remove(self) -> Result<()> {
        self.log.remove()
    }
}

impl<T: Serialize> Sink for JsonlSink<T> {
    type Item = T;

    /// One fsync'd batch per call; the log is durable after every append and
    /// the writer lock releases on drop — there is nothing to finalize.
    fn record(&mut self, batch: &[T]) -> Result<()> {
        self.log.append(batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::{cell::Cell, rc::Rc};

    struct Probe {
        flushes: Rc<Cell<u32>>,
    }

    impl Sink for Probe {
        type Item = ();

        fn record(&mut self, _batch: &[()]) -> Result<()> {
            Ok(())
        }

        fn flush(&mut self) -> Result<()> {
            self.flushes.set(self.flushes.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn default_close_flushes_before_release() {
        // The trio's composition contract: a buffering sink that overrides
        // only `flush` must still be flushed by the default `close`.
        let flushes = Rc::new(Cell::new(0));
        let sink = Probe {
            flushes: Rc::clone(&flushes),
        };

        sink.close().unwrap();
        assert_eq!(flushes.get(), 1);
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Entry {
        n: u32,
    }

    #[test]
    fn appends_each_entry_as_one_line_across_reopens() {
        // The sink is a thin typed view of `Log`: entries land one per line, a
        // later create() appends rather than truncates, and close releases the
        // lock so the next writer can open.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");

        let mut sink: JsonlSink<Entry> = JsonlSink::create(path.clone(), "test log").unwrap();
        sink.record(&[Entry { n: 1 }, Entry { n: 2 }]).unwrap();
        drop(sink); // the writer lock releases on drop

        let mut sink: JsonlSink<Entry> = JsonlSink::create(path.clone(), "test log").unwrap();
        sink.record(&[Entry { n: 3 }]).unwrap();
        drop(sink);

        let entries: Vec<Entry> = crate::jsonl::read_strict(&path).unwrap();
        assert_eq!(entries, [Entry { n: 1 }, Entry { n: 2 }, Entry { n: 3 }]);
    }
}
