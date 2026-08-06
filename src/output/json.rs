/// JSON output renderer.
///
/// Success payloads and error envelopes are printed to stdout as single-line
/// JSON objects. This keeps the contract simple for machine consumers.
use std::io::Write;

use crate::errors::KrakenError;

/// Write a single line to stdout without panicking on write failure.
///
/// Unlike `println!`, this never panics. A broken pipe (e.g. when the consumer
/// is `head` and closes the stream early) is expected and silently ignored —
/// there is nothing useful to report once the reader is gone. Any other write
/// failure (e.g. a full disk when redirected to a file) is reported to stderr.
fn print_line(s: &str) {
    if let Err(e) = writeln!(std::io::stdout(), "{s}")
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        tracing::error!(error = %e, "error writing to stdout");
    }
}

pub(crate) fn render_success(data: &serde_json::Value) {
    match serde_json::to_string(data) {
        Ok(s) => print_line(&s),
        Err(e) => {
            tracing::error!(error = %e, "JSON serialization failed");
            print_line(r#"{"error":"parse","message":"JSON serialization failed"}"#);
        }
    }
}

/// Render an error envelope as JSON to stdout (per spec: errors go to stdout in JSON mode).
pub(crate) fn render_error(err: &KrakenError) {
    let envelope = err.to_json_envelope();
    match serde_json::to_string(&envelope) {
        Ok(s) => print_line(&s),
        Err(e) => {
            tracing::error!(error = %e, "JSON serialization failed");
            print_line(r#"{"error":"parse","message":"JSON serialization failed"}"#);
        }
    }
}

/// Render a single JSONL line to stdout (for WebSocket streaming).
pub(crate) fn render_jsonl(data: &serde_json::Value) {
    match serde_json::to_string(data) {
        Ok(s) => print_line(&s),
        Err(e) => {
            tracing::error!(error = %e, "JSON serialization failed");
            print_line(r#"{"error":"parse","message":"JSON serialization failed"}"#);
        }
    }
}
