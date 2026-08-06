//! Cross-process advisory file locks (`std::fs::File::try_lock`, flock
//! semantics).
//!
//! Locks live on a sidecar `<path>.lock` ([`lock_path`]), never on the
//! guarded file itself: an OS lock there would collide with the engine that
//! owns it (Windows file locks are mandatory and fail the engine's I/O; on
//! unix, closing any fd drops the process's POSIX locks, so even a probe
//! could release DuckDB's own lock).
//!
//! Acquisition is fail-fast: in use means no — overlapping commands are a
//! conflict, never a queue. The kernel releases a lock the moment its holder
//! dies, so there is no stale state and no reclaim logic; liveness is probed
//! by try-locking ([`is_lock_held`]), which a recycled PID cannot spoof.
//!
//! Writers hold the lock exclusive; readers hold it shared
//! ([`FileLock::acquire_shared`]) — mutually exclusive with writers, but
//! invisible to the liveness probe and to each other. Only an exclusive
//! `Drop` unlinks the sidecar, while still holding the lock: any other
//! unlink splits the name from the held inode and breaks exclusion. A crash
//! skips the unlink; the leftover 0-byte file is inert and reads as free.
//!
//! KNOWN RACE (accepted): locks bind to inodes, unlink retires the *name*. A
//! contender that opens just before a release and locks just after holds a
//! nameless inode while the next contender locks a fresh file — two holders
//! on one resource; the same window can make [`is_lock_held`] read a live
//! holder as free. Requires microsecond interleaving; accepted over carrying
//! the fd-vs-path inode recheck that would close it (in git history if it
//! ever bites).

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Writer (exclusive) vs reader (shared) posture; only exclusive holders
/// read as held to [`is_lock_held`].
#[derive(Debug, PartialEq)]
enum LockMode {
    Exclusive,
    Shared,
}

/// Advisory lock guard: the open fd *is* the lock. Dropping releases (so does
/// process death); an exclusive guard's drop also unlinks the sidecar.
#[derive(Debug)]
#[must_use = "the lock is released as soon as this guard is dropped"]
pub struct FileLock {
    _file: fs::File,
    /// The sidecar itself (`<resource>.lock`), not the guarded resource.
    path: PathBuf,
    /// Names the guarded resource in the release-failure warning.
    resource: String,
    mode: LockMode,
}

/// Unlink-then-close: the fd (and so the lock) outlives the unlink because
/// fields drop after this body — narrowing, not closing, the accepted race.
impl Drop for FileLock {
    fn drop(&mut self) {
        // A shared holder must not unlink: other readers may still hold the
        // inode, and a writer could then lock a fresh file mid-read.
        if self.mode == LockMode::Shared {
            return;
        }
        if let Err(e) = fs::remove_file(&self.path) {
            // Already gone (e.g. removed by hand) is the desired end state.
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    resource = %self.resource,
                    path = %self.path.display(),
                    error = %e,
                    "could not unlink lock file on release; the leftover is inert"
                );
            }
        }
    }
}

impl FileLock {
    /// Acquire the (exclusive, writer) lock guarding `path`, or reject when
    /// held — in use means no. `resource` names the contended thing in the
    /// rejection message.
    ///
    /// # Errors
    /// [`Rejected`](Error::Rejected) when another process holds the lock —
    /// rarely also when a racing [`is_lock_held`] probe transiently holds it
    /// (rerunning recovers); [`Io`](Error::Io) on filesystem failure.
    pub fn acquire(path: &Path, resource: impl Into<String>) -> Result<Self> {
        Self::acquire_mode(path, resource.into(), LockMode::Exclusive)
    }

    /// Reader counterpart of [`acquire`](Self::acquire): mutually exclusive
    /// with writers, coexists with readers, reads as free to [`is_lock_held`].
    pub fn acquire_shared(path: &Path, resource: impl Into<String>) -> Result<Self> {
        Self::acquire_mode(path, resource.into(), LockMode::Shared)
    }

    fn acquire_mode(path: &Path, resource: String, mode: LockMode) -> Result<Self> {
        let lock_file = lock_path(path);
        let file = open_lock_file(&lock_file)?;
        let locked = match mode {
            LockMode::Exclusive => file.try_lock(),
            LockMode::Shared => file.try_lock_shared(),
        };
        match locked {
            Ok(()) => Ok(Self {
                _file: file,
                path: lock_file,
                resource,
                mode,
            }),
            Err(fs::TryLockError::WouldBlock) => Err(Error::Rejected(format!(
                "{resource} is in use by another process."
            ))),
            Err(fs::TryLockError::Error(e)) => Err(Error::Io(e)),
        }
    }
}

/// The sidecar `<path>.lock` — appended, not `with_extension`, so sibling
/// tapes (`BTC-USD.duckdb`, `BTC-USD.jsonl`) get distinct locks. `path`
/// is the resource, never a lock file itself.
pub fn lock_path(path: &Path) -> PathBuf {
    debug_assert!(
        path.extension() != Some(std::ffi::OsStr::new("lock")),
        "expected a resource path, got a lock file path: {}",
        path.display()
    );
    let mut raw = path.as_os_str().to_owned();
    raw.push(".lock");
    PathBuf::from(raw)
}

/// Open (creating if absent, never truncating) the lock file, with its parent
/// directories in place.
fn open_lock_file(lock_path: &Path) -> Result<fs::File> {
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?)
}

/// Whether a live process holds the lock guarding `path`, probed with a
/// *shared* lock: a holder's exclusive lock reads held, while concurrent
/// probes never see one another. A missing file means no holder; an
/// unopenable one conservatively reads held, so a live holder is never
/// concluded gone.
pub fn is_lock_held(path: &Path) -> bool {
    let file = match fs::File::open(lock_path(path)) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    match file.try_lock_shared() {
        // Acquired shared: nobody holds it exclusively. The probe's own
        // shared lock dies with `file`.
        Ok(()) => false,
        Err(fs::TryLockError::WouldBlock) => true,
        Err(fs::TryLockError::Error(_)) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_released_on_drop_and_reacquirable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        {
            let _lock = FileLock::acquire(&path, "Test resource").unwrap();
            assert!(is_lock_held(&path), "held while the guard lives");
            assert!(lock_path(&path).exists(), "sidecar exists while held");
        }
        // Release unlinks the sidecar again and frees the lock; the guarded
        // resource itself is never created.
        assert!(!lock_path(&path).exists(), "release removes the sidecar");
        assert!(!path.exists());
        assert!(!is_lock_held(&path));
        drop(FileLock::acquire(&path, "Test resource").expect("reacquire after drop"));
    }

    #[test]
    fn second_acquire_is_rejected_while_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let _held = FileLock::acquire(&path, "Test resource").unwrap();

        let err = FileLock::acquire(&path, "Test resource").unwrap_err();
        assert!(matches!(err, Error::Rejected(_)));
        assert!(err.to_string().contains("in use by another process"));
    }

    #[test]
    fn probe_does_not_steal_or_keep_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let held = FileLock::acquire(&path, "Test resource").unwrap();
        assert!(is_lock_held(&path));
        drop(held);
        // Probe a crash-leftover sidecar (release unlinks, so seed one): the
        // probe transiently acquires and must not still hold afterwards.
        drop(open_lock_file(&lock_path(&path)).unwrap());
        assert!(!is_lock_held(&path));
        assert!(!is_lock_held(&path));
        drop(FileLock::acquire(&path, "Test resource").expect("probe left the lock free"));
    }

    #[test]
    fn missing_lock_file_reads_as_not_held() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_lock_held(&dir.path().join("never-created")));
    }

    #[test]
    fn shared_holder_excludes_writers_but_not_probes_or_readers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let reader = FileLock::acquire_shared(&path, "Test resource").unwrap();

        assert!(!is_lock_held(&path), "a reader must not read as a writer");
        let err = FileLock::acquire(&path, "Test resource").unwrap_err();
        assert!(matches!(err, Error::Rejected(_)));
        drop(FileLock::acquire_shared(&path, "Test resource").expect("readers coexist"));

        drop(reader);
        drop(FileLock::acquire(&path, "Test resource").expect("writer acquires after the read"));
    }

    #[test]
    fn exclusive_holder_rejects_shared_acquisition() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let _writer = FileLock::acquire(&path, "Test resource").unwrap();

        let err = FileLock::acquire_shared(&path, "Test resource").unwrap_err();
        assert!(matches!(err, Error::Rejected(_)));
        assert!(err.to_string().contains("in use"), "got: {err}");
    }

    #[test]
    fn shared_release_leaves_the_sidecar_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let first = FileLock::acquire_shared(&path, "Test resource").unwrap();
        let second = FileLock::acquire_shared(&path, "Test resource").unwrap();

        drop(first);
        assert!(
            lock_path(&path).exists(),
            "sidecar survives a shared release"
        );
        let err = FileLock::acquire(&path, "Test resource").unwrap_err();
        assert!(
            matches!(err, Error::Rejected(_)),
            "the remaining reader still excludes writers through the same inode"
        );
        drop(second);
    }

    #[test]
    fn probes_do_not_see_each_other() {
        // Probes take shared locks: with another probe mid-flight on the
        // same free lock, is_lock_held must still read it as free — only a
        // real (exclusive) holder reads as held.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        // A crash-leftover sidecar nobody holds (release would unlink it).
        drop(open_lock_file(&lock_path(&path)).unwrap());
        let probe_like = fs::File::open(lock_path(&path)).unwrap();
        probe_like.try_lock_shared().unwrap();
        assert!(
            !is_lock_held(&path),
            "a concurrent probe must not read as a holder"
        );
    }
}
