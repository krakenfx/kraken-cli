/// JSON output renderer.
///
/// Success payloads and error envelopes are printed to stdout as single-line
/// JSON objects. This keeps the contract simple for machine consumers.
use crate::errors::KrakenError;

/// Render a success payload as JSON to stdout.
pub(crate) fn render_success(data: &serde_json::Value) {
    match serde_json::to_string(data) {
        Ok(s) => println!("{s}"),
        Err(e) => {
            eprintln!("JSON serialization failed: {e}");
            println!(r#"{{"error":"parse","message":"JSON serialization failed"}}"#);
        }
    }
}

/// Render an error envelope as JSON to stdout (per spec: errors go to stdout in JSON mode).
pub(crate) fn render_error(err: &KrakenError) {
    let envelope = err.to_json_envelope();
    match serde_json::to_string(&envelope) {
        Ok(s) => println!("{s}"),
        Err(e) => {
            eprintln!("JSON serialization failed: {e}");
            println!(r#"{{"error":"parse","message":"JSON serialization failed"}}"#);
        }
    }
}

/// Render a single NDJSON line to stdout (for WebSocket streaming).
pub(crate) fn render_ndjson(data: &serde_json::Value) {
    match serde_json::to_string(data) {
        Ok(s) => println!("{s}"),
        Err(e) => {
            eprintln!("JSON serialization failed: {e}");
            println!(r#"{{"error":"parse","message":"JSON serialization failed"}}"#);
        }
    }
}
