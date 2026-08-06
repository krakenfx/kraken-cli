//! The paper-trading domain: the account's event journal.
//!
//! A paper account **is** its append-only journal — state is derived by
//! folding events in order, never snapshotted — so the domain's storage
//! contract is exactly [`journal`]/[`replay`]: the generic JSONL pair under
//! this domain's lock-holder name. The event type stays with the simulator
//! (the crate persists any `Serialize` envelope), so the pair is generic.
//!
//! A columnar (DuckDB) backend for querying trading history is planned but
//! unimplemented; the journal is the source of truth either way.

use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::Result;
use crate::sink::JsonlSink;
use crate::source::JsonlSource;

/// Names the journal's lock holder in the "in use" error a contender sees.
pub const JOURNAL_PURPOSE: &str = "paper account log";

/// Open (creating if absent) an account journal for appending, holding its
/// single-writer lock for the sink's lifetime — a concurrent open is
/// rejected, so one command's read→decide→append is one transaction.
pub fn journal<T: Serialize>(path: PathBuf) -> Result<JsonlSink<T>> {
    JsonlSink::create(path, JOURNAL_PURPOSE)
}

/// Open an account journal for replay, or `None` when none exists. Lock-free:
/// the caller guarantees writer quiescence, normally by holding the journal's
/// own [`journal`] sink (see [`JsonlSource`]).
pub fn replay<T: DeserializeOwned>(path: PathBuf) -> Option<JsonlSource<T>> {
    JsonlSource::open(path)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::error::Error;
    use crate::sink::Sink;
    use crate::source::Source;

    /// Stands in for the simulator's record envelope.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Event {
        balance: u32,
    }

    #[test]
    fn journal_round_trips_events_across_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let mut sink = journal::<Event>(path.clone()).unwrap();
        sink.record(&[Event { balance: 100 }]).unwrap();
        drop(sink); // release the writer lock

        let mut sink = journal::<Event>(path.clone()).unwrap();
        sink.record(&[Event { balance: 90 }]).unwrap();
        drop(sink);

        let replayed: Vec<Event> = replay(path)
            .expect("journal exists")
            .read()
            .unwrap()
            .collect();
        assert_eq!(replayed, [Event { balance: 100 }, Event { balance: 90 }]);
    }

    #[test]
    fn concurrent_journal_open_is_rejected_naming_the_holder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let _held = journal::<Event>(path.clone()).unwrap();

        let err = journal::<Event>(path).unwrap_err();
        assert!(matches!(err, Error::Rejected(_)));
        assert!(err.to_string().contains(JOURNAL_PURPOSE), "got: {err}");
    }

    #[test]
    fn replay_of_a_missing_journal_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(replay::<Event>(dir.path().join("events.jsonl")).is_none());
    }
}
