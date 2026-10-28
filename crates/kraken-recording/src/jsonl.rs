//! JSONL logs: one compact JSON line per entry, one writer at a time.
//!
//! [`Log`] is the write engine — constructing one *is* acquiring the file's
//! writer lock, so appends, tail repair, and removal are serialized by the
//! type, not by convention.
//!
//! Crash contract, honestly stated: an append is one `write_all` + fsync of
//! the whole batch, so a *clean return* means the batch is durable. A crash
//! mid-append offers no such atomicity — the filesystem may persist any
//! prefix of the batch's lines (and, on out-of-order page writeback, even a
//! hole inside one). The trailing newline is each line's commit marker:
//! a torn final line is silently repaired before the next append and ignored
//! by readers, while an interior hole fails the read loud. Consumers that
//! need multi-event atomicity must encode the transaction as a single line.

use crate::{
    error::{Error, Result},
    fsync::fsync_parent,
    lock::FileLock,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

/// An append-only JSONL log with its writer lock held for the handle's
/// lifetime, so a caller's whole read→decide→append sequence is one
/// serialized transaction.
#[derive(Debug)]
#[must_use = "the log's writer lock is released as soon as this handle is dropped"]
pub(crate) struct Log {
    path: PathBuf,
    _lock: FileLock,
}

impl Log {
    /// Open the log for appending, creating it if absent. Lock first, then
    /// touch the file: exclusivity (and the recording sink's liveness beacon)
    /// starts before the first append, and the file is visible from creation,
    /// which `playground`'s presence probes rely on. Parent directories come
    /// from acquiring the sibling lock.
    pub(crate) fn open(path: PathBuf, purpose: impl Into<String>) -> Result<Self> {
        let lock = FileLock::acquire(&path, purpose)?;
        OpenOptions::new().create(true).append(true).open(&path)?;
        // Without the directory fsync a crash could roll the creation back,
        // erasing a name that liveness probes already observed.
        fsync_parent(&path)?;
        Ok(Self { path, _lock: lock })
    }

    /// Append the batch as one fsync'd `write_all`, repairing any torn tail
    /// first. Durable on return; on a crash mid-append only a prefix of the
    /// batch may survive (see the module contract).
    pub(crate) fn append<T: Serialize>(&self, entries: &[T]) -> Result<()> {
        repair_torn_tail(&self.path)?;
        let mut buf = Vec::new();
        for entry in entries {
            serde_json::to_writer(&mut buf, entry)
                .map_err(|e| Error::Damaged(format!("unencodable log entry: {e}")))?;
            buf.push(b'\n');
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(&buf)?;
        file.sync_all()?;
        Ok(())
    }

    /// Delete the log under its lock, consuming the handle. Already
    /// absent counts as success.
    pub(crate) fn remove(self) -> Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// A JSONL entry's `seq` from `line_no`, its **1-based** physical line number
/// (writers emit no blank lines, so that is also its 1-based position).
/// Saturates — a silent wrap would corrupt the order.
pub fn line_seq(line_no: usize) -> i64 {
    i64::try_from(line_no).unwrap_or(i64::MAX)
}

/// Damage: content the reader can't interpret — never a skippable diagnostic.
pub fn damaged(path: &Path, line: usize, what: &str) -> Error {
    Error::Damaged(format!(
        "damaged log {}: {what} on line {line}",
        path.display()
    ))
}

/// Stream a log's committed, non-blank lines as raw bytes, each with its
/// 1-indexed *physical* line number (blank lines are skipped but still
/// counted, so diagnostics name the on-disk line). Buffers one line at a
/// time (recordings run to GBs). Lock-free: the writer-quiescence contract
/// it requires is stated on [`JsonlSource`](crate::source::JsonlSource).
///
/// The trailing newline is a line's commit marker: bytes after the last one
/// are a crash's torn tail, not yielded.
pub(crate) fn committed_lines(
    path: &Path,
) -> Result<impl Iterator<Item = Result<(usize, Vec<u8>)>>> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut line_no = 0usize;
    // Lines are yielded owned, so a buffer can't be reused outright — but
    // seeding each with the previous line's capacity keeps the read_until
    // growth (alloc-and-copy doublings from empty) off the per-line path.
    let mut capacity = 0usize;
    Ok(std::iter::from_fn(move || {
        loop {
            let mut buf = Vec::with_capacity(capacity);
            match reader.read_until(b'\n', &mut buf) {
                Err(e) => return Some(Err(e.into())),
                Ok(0) => return None, // clean end of log
                Ok(_) => {}
            }
            if !buf.ends_with(b"\n") {
                return None; // no commit marker: a crash's torn tail
            }
            buf.pop();
            if buf.ends_with(b"\r") {
                buf.pop();
            }
            line_no += 1;
            if buf.is_empty() {
                continue;
            }
            capacity = buf.capacity();
            return Some(Ok((line_no, buf)));
        }
    }))
}

/// Stream a log's committed lines, each deserialized as `T` and paired with
/// its 1-based physical line number — [`committed_lines`] with decoding. A
/// malformed committed line is a hard error naming its physical line —
/// skipping one would silently corrupt replayed state.
pub(crate) fn numbered_json_lines<T: DeserializeOwned>(
    path: &Path,
) -> Result<impl Iterator<Item = Result<(usize, T)>>> {
    let lines = committed_lines(path)?;
    let path = path.to_path_buf();
    Ok(lines.map(move |line| {
        let (line_no, raw) = line?;
        serde_json::from_slice(&raw)
            .map(|entry| (line_no, entry))
            .map_err(|err| {
                Error::Damaged(format!(
                    "malformed log line {line_no} in {}: {err}",
                    path.display()
                ))
            })
    }))
}

/// All committed lines of the log, eagerly decoded:
/// [`numbered_json_lines`] collected, line numbers dropped.
pub(crate) fn read_strict<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    numbered_json_lines(path)?
        .map(|entry| Ok(entry?.1))
        .collect()
}

/// Backward-scan window for locating the last committed newline: a torn
/// fragment is at most one line, so the first window almost always hits.
const TAIL_SCAN_CHUNK: usize = 64 * 1024;

/// Truncate to the last newline when a torn fragment follows it, so an
/// append can't glue onto the fragment and turn a recoverable torn *tail*
/// into a malformed *interior* line. No-op for a missing, empty, or clean
/// log. The clean-case probe reads one byte, and the repair scans backwards
/// in [`TAIL_SCAN_CHUNK`] windows rather than reading the whole file —
/// appends are frequent and logs grow to GBs.
fn repair_torn_tail(path: &Path) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        // Absent: nothing to repair.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)?;
    if last[0] == b'\n' {
        return Ok(());
    }
    let mut buf = vec![0u8; TAIL_SCAN_CHUNK];
    let mut end = len;
    let clean_len = loop {
        if end == 0 {
            break 0; // no newline anywhere: the whole file is the fragment
        }
        let start = end.saturating_sub(TAIL_SCAN_CHUNK as u64);
        let window = &mut buf[..(end - start) as usize];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(window)?;
        if let Some(i) = window.iter().rposition(|&b| b == b'\n') {
            break start + i as u64 + 1;
        }
        end = start;
    };
    drop(file);
    tracing::warn!(path = %path.display(), "truncating torn tail before append");
    let file = OpenOptions::new().write(true).open(path)?;
    file.set_len(clean_len)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Entry {
        n: u32,
    }

    /// The setup operation, as `init`/provision/the sink run it.
    fn create(path: &Path) -> Log {
        Log::open(path.to_path_buf(), "test log").unwrap()
    }

    /// A later command's handle: `open` on an existing log locks and appends.
    fn open(path: &Path) -> Log {
        create(path)
    }

    #[test]
    fn open_locks_before_touching_and_is_visible_from_creation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let held = create(&path);
        assert!(
            path.exists(),
            "the setup operation touches the file eagerly"
        );
        // While the setup handle lives, a second handle cannot be opened.
        let err = Log::open(path.clone(), "test log");
        assert!(err.is_err(), "contender must be rejected while held");
        drop(held);
        assert!(read_strict::<Entry>(&path).unwrap().is_empty());
    }

    #[test]
    fn torn_tail_is_repaired_not_glued_onto() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        create(&path).append(&[Entry { n: 1 }]).unwrap();
        // Simulate a crash mid-append: a partial line with no trailing newline.
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"n\": 2").unwrap();
        drop(file);

        open(&path).append(&[Entry { n: 3 }]).unwrap();
        let entries: Vec<Entry> = read_strict(&path).unwrap();
        assert_eq!(entries, [Entry { n: 1 }, Entry { n: 3 }]);
    }

    #[test]
    fn torn_tail_spanning_the_scan_window_is_repaired() {
        // A fragment longer than one backward-scan window forces the repair
        // to cross a chunk boundary before it finds the last newline.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        create(&path).append(&[Entry { n: 1 }]).unwrap();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&vec![b'x'; TAIL_SCAN_CHUNK + 1024]).unwrap();
        drop(file);

        open(&path).append(&[Entry { n: 3 }]).unwrap();
        let entries: Vec<Entry> = read_strict(&path).unwrap();
        assert_eq!(entries, [Entry { n: 1 }, Entry { n: 3 }]);
    }

    #[test]
    fn read_ignores_a_torn_tail_without_repairing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        create(&path).append(&[Entry { n: 1 }]).unwrap();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"n\": 2").unwrap();
        drop(file);

        let entries: Vec<Entry> = read_strict(&path).unwrap();
        assert_eq!(entries, [Entry { n: 1 }]);
        // Reads are read-only: the fragment stays until the next append
        // repairs it (pinned by torn_tail_is_repaired_not_glued_onto).
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.ends_with("{\"n\": 2"), "fragment left in place");
    }

    #[test]
    fn malformed_interior_line_fails_loud() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, "{\"n\": 1}\nnot json\n{\"n\": 2}\n").unwrap();
        let err = read_strict::<Entry>(&path).unwrap_err();
        assert!(matches!(err, Error::Damaged(_)));
        assert!(err.to_string().contains("line 2"), "got: {err}");
    }

    #[test]
    fn blank_lines_are_skipped_but_keep_line_numbering() {
        // `Log` never writes blank lines; reads tolerate them anyway.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, "{\"n\": 1}\n\nnot json\n").unwrap();
        let err = read_strict::<Entry>(&path).unwrap_err();
        assert!(err.to_string().contains("line 3"), "got: {err}");
    }

    #[test]
    fn remove_deletes_the_log_under_its_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        create(&path).append(&[Entry { n: 1 }]).unwrap();
        open(&path).remove().unwrap();
        assert!(!path.exists());
        // Dropping the handle release-unlinks the lock sibling too: a
        // removed log leaves nothing behind.
        assert!(
            !crate::lock::lock_path(&path).exists(),
            "release unlinks the lock sibling"
        );
    }

    proptest! {
        /// The repair invariant: truncation lands exactly on the last commit
        /// marker — every committed byte survives, no fragment byte does.
        #[test]
        fn repair_truncates_exactly_to_the_committed_prefix(
            lines in proptest::collection::vec("[ -~]{0,16}", 0..8),
            // Sized, not regex-generated: fragments must reach past
            // TAIL_SCAN_CHUNK to exercise the window-crossing scan.
            fragment_len in 1usize..(TAIL_SCAN_CHUNK * 2),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("log.jsonl");
            let committed: String = lines.iter().map(|l| format!("{l}\n")).collect();
            let mut raw = committed.clone().into_bytes();
            raw.extend(std::iter::repeat_n(b'x', fragment_len));
            std::fs::write(&path, raw).unwrap();

            repair_torn_tail(&path).unwrap();
            prop_assert_eq!(std::fs::read_to_string(&path).unwrap(), committed);
        }
    }
}
