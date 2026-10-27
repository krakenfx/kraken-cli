//! The read side — pendant of [`sink`](crate::sink): the domain-free
//! [`Source`] contract and the generic JSONL backend [`JsonlSource`].
//! Write and read share each track's serde structs, so an undecodable line
//! is damage and fails loud; only a crash's torn tail is tolerated. The
//! market read contracts live with their domain under
//! [`frames`](crate::frames).

use crate::{error::Result, jsonl};
use serde::de::DeserializeOwned;
use std::{marker::PhantomData, path::PathBuf};

/// The read side of a recorded track, handed back as entries. The pendant
/// of [`Sink`](crate::sink::Sink), and generic over the entry type for the
/// same reason — one read contract for every track.
///
/// Construction stays on the concrete types: what it takes to open a track
/// (a path, a table, a lock already held) is exactly what differs per
/// backend.
pub trait Source {
    type Item;

    /// The whole track as an ordered stream. Damage fails here, before the
    /// first item, so no consumer ever observes a partial stream — the
    /// deliberate cost is that implementations materialize the entire track
    /// in memory before yielding (validation and cross-table ordering need
    /// it), so a GB-scale replay pays O(track) memory.
    fn read(&self) -> Result<impl Iterator<Item = Self::Item>>;
}

/// The generic JSONL source — the read pendant of
/// [`JsonlSink`](crate::sink::JsonlSink).
///
/// Lock-free by design, so it can read a log whose lock the caller already
/// holds (a journal owner reads its own log under its sink's lock). That
/// makes writer quiescence the CALLER's contract: read only while you hold
/// the log's sink or its [`FileLock`](crate::lock::FileLock), or after
/// probing [`is_lock_held`](crate::lock::is_lock_held). Racing a live writer
/// can observe a torn-tail repair mid-splice and read lines no process ever
/// wrote.
pub struct JsonlSource<T> {
    path: PathBuf,
    _entry: PhantomData<T>,
}

impl<T: DeserializeOwned> JsonlSource<T> {
    /// Open an existing log for reading, or `None` when there is nothing on
    /// disk.
    pub fn open(path: PathBuf) -> Option<Self> {
        path.exists().then_some(Self {
            path,
            _entry: PhantomData,
        })
    }

    /// [`Source::read`], each entry paired with its 1-based *physical* line
    /// number (blank lines are skipped but counted) — for consumers whose
    /// `seq` and diagnostics must agree with this module's own on-disk line
    /// reporting.
    pub fn read_numbered(&self) -> Result<impl Iterator<Item = (usize, T)>> {
        Ok(jsonl::numbered_json_lines(&self.path)?
            .collect::<Result<Vec<_>>>()?
            .into_iter())
    }
}

impl<T: DeserializeOwned> Source for JsonlSource<T> {
    type Item = T;

    /// Strict read: a torn tail is ignored (a crash artifact), a malformed
    /// interior line fails loud.
    fn read(&self) -> Result<impl Iterator<Item = T>> {
        Ok(jsonl::read_strict(&self.path)?.into_iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::{JsonlSink, Sink};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Entry {
        n: u32,
    }

    #[test]
    fn open_of_a_missing_log_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(JsonlSource::<Entry>::open(dir.path().join("log.jsonl")).is_none());
    }

    #[test]
    fn reads_back_what_the_sink_wrote() {
        // The pair's round-trip contract: source(sink(entries)) == entries.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let mut sink: JsonlSink<Entry> = JsonlSink::create(path.clone(), "test log").unwrap();
        sink.record(&[Entry { n: 1 }, Entry { n: 2 }]).unwrap();
        drop(sink); // release the writer lock before reading

        let source = JsonlSource::<Entry>::open(path).expect("log exists");
        assert_eq!(
            source.read().unwrap().collect::<Vec<_>>(),
            [Entry { n: 1 }, Entry { n: 2 }]
        );
    }
}
