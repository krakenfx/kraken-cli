//! The `kraken` binary: build a runtime, name the front-ends, hand over.
//!
//! The one-shot product: one command per invocation. Its siblings
//! `kraken-shell` and `kraken-mcp` are separate binaries over the same engine,
//! and none of the three can start another.

use std::{process::ExitCode, time::Duration};

/// How long runtime shutdown may wait on outstanding blocking work (a wedged
/// sink `CHECKPOINT` on a spawn_blocking thread) after `run` returns. Without
/// this bound the runtime drop joins those threads indefinitely and the exiting
/// process hangs.
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

fn main() -> ExitCode {
    // The MCP server serves tool calls on spawned worker tasks, and
    // `execute_command` builds a large future (the `Command` enum spans 150+
    // variants). Tokio's default 2 MiB worker stack overflows on that future,
    // aborting the process on the first tool call. 8 MiB matches the main-thread
    // default with headroom. Worker stacks are committed lazily, so the larger
    // size is effectively free: non-streaming commands await their work on the
    // main thread, and the tasks the streaming commands spawn are small.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .build()
    {
        Ok(runtime) => runtime,
        // Pre-runtime, so pre-subscriber and pre-CLI-parse: a bare stderr line
        // and exit 1 are all the contract we can honor this early.
        Err(e) => {
            eprintln!("kraken: failed to build tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    // `run` returns the code rather than exiting so the drain below also runs on
    // the error path — `process::exit` here would kill a mid-flight sink finalizer.
    let code = runtime.block_on(kraken_cli::run());
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
    code
}
