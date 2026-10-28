//! Process supervision for `kraken session`: liveness probes and graceful
//! termination of the recorder it spawns. Unix signal plumbing — deliberately
//! beside the CLI, not in the recording crate, which owns file locks only.

use crate::errors::{KrakenError, Result};

/// Whether a process with `pid` is still alive — the fallback liveness signal
/// for recording-less runs with no lock to probe. Deliberately
/// conservative: any ambiguity reports alive.
#[cfg(unix)]
#[allow(unsafe_code)] // SAFETY on the `libc::kill` call below
pub(crate) fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return true; // not a real, individually-signalable PID
    }
    // SAFETY: `kill` with signal 0 performs only the existence/permission check —
    // it delivers no signal and dereferences no memory — so any `pid` value is
    // sound to pass.
    let rc = unsafe { libc::kill(pid, 0) };
    // rc == 0: a signal could be delivered → alive. EPERM: the process exists but
    // is owned by another user → alive. ESRCH (the remaining errno) → no such
    // process → dead.
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
pub(crate) fn process_alive(_pid: i32) -> bool {
    true // no portable liveness probe; never conclude a holder is gone
}

/// Ask process `pid` to stop by sending `SIGTERM`, so a running recorder takes
/// its graceful shutdown path (checkpoint + release its tape lock). A process
/// that has already exited (`ESRCH`) counts as success — the caller's goal
/// (that pid gone) already holds.
///
/// # Errors
/// Returns [`KrakenError::Io`] if the signal fails for a reason other than the
/// process being gone (e.g. `EPERM`).
#[cfg(unix)]
#[allow(unsafe_code)] // SAFETY on the `libc::kill` call below
pub(crate) fn terminate(pid: i32) -> Result<()> {
    if pid <= 0 {
        // kill(0) signals the caller's whole process group and kill(-n) a
        // group by id — never what a caller of this helper means.
        return Err(KrakenError::Validation(format!(
            "pid {pid} is not an individually signalable process id"
        )));
    }
    // SAFETY: `kill` delivers a signal to (or checks) an existing process and
    // dereferences no memory, so any `pid` value is sound to pass.
    let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
    if rc == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) {
        return Ok(()); // already gone — the desired end state, not a failure
    }
    Err(KrakenError::Io(err))
}

/// Signalling a process by pid is unix-only; elsewhere report a clear error
/// rather than failing to build.
///
/// # Errors
/// Always returns [`KrakenError::Validation`].
#[cfg(not(unix))]
pub(crate) fn terminate(_pid: i32) -> Result<()> {
    Err(KrakenError::Validation(
        "stopping a session is only supported on unix (SIGTERM)".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_alive_is_true_for_self_and_false_for_an_impossible_pid() {
        // This test process is, by definition, alive.
        assert!(process_alive(i32::try_from(std::process::id()).unwrap()));
        // `i32::MAX - 1` is far above any system's pid_max, so no such process exists.
        assert!(!process_alive(i32::MAX - 1));
    }

    #[cfg(unix)]
    #[test]
    fn terminate_rejects_an_unsignalable_pid() {
        // pid 0 would SIGTERM the caller's entire process group.
        assert!(matches!(terminate(0), Err(KrakenError::Validation(_))));
        assert!(matches!(terminate(-1), Err(KrakenError::Validation(_))));
    }

    #[cfg(unix)]
    #[test]
    fn terminate_is_ok_for_an_already_dead_process() {
        // `i32::MAX - 1` names no live process, so `kill` reports `ESRCH`, which
        // `terminate` treats as success — the process is already gone, which is
        // the caller's goal.
        assert!(terminate(i32::MAX - 1).is_ok());
    }
}
