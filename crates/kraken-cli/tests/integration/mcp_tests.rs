//! MCP stdio session-lifecycle contract tests.
//!
//! rmcp 3 serves sessions that never call `initialize`: a 2026-07-28 client
//! may open with any request carrying the required `_meta` keys (inline
//! lifecycle). The audit trail's session boundary must not depend on which
//! lifecycle the client picked, so these tests drive the real binary over
//! stdio and count `session_start` records on stderr.
//!
//! `kraken_paper_status` against a fresh `$HOME` is deterministic and offline:
//! it returns a stable validation envelope without touching the network.

use super::common::kraken_mcp_in;

/// One request/line per element; the server exits cleanly on stdin EOF.
fn mcp_stderr(home: &tempfile::TempDir, requests: &[&str]) -> String {
    let output = kraken_mcp_in(home)
        .args(["-s", "paper"])
        .write_stdin(format!("{}\n", requests.join("\n")))
        .output()
        .unwrap();
    assert!(output.status.success(), "mcp server exited nonzero");
    String::from_utf8(output.stderr).unwrap()
}

fn audit_events(stderr: &str, event: &str) -> usize {
    stderr
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["mcp_audit"]["event"] == event)
        .count()
}

/// Regression (rmcp 2→3): v2 hard-refused non-`initialize` openers, so every
/// tool-executing session implied a `session_start` record. v3's inline
/// lifecycle must keep that invariant via the lazy session audit.
#[test]
fn initialize_less_session_still_audits_session_start() {
    let home = tempfile::tempdir().unwrap();
    let stderr = mcp_stderr(
        &home,
        &[concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"#,
            r#""name":"kraken_paper_status","arguments":{},"#,
            r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","#,
            r#""io.modelcontextprotocol/clientCapabilities":{}}}}"#
        )],
    );

    assert_eq!(
        audit_events(&stderr, "session_start"),
        1,
        "an initialize-less session must still open with session_start: {stderr}"
    );
    assert_eq!(
        audit_events(&stderr, "tool_result"),
        1,
        "the inline-lifecycle call must execute and be audited: {stderr}"
    );
}

/// A classic initialize handshake followed by tool activity must not
/// duplicate the session boundary.
#[test]
fn session_start_audited_once_per_session() {
    let home = tempfile::tempdir().unwrap();
    let stderr = mcp_stderr(
        &home,
        &[
            concat!(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"#,
                r#""protocolVersion":"2025-06-18","capabilities":{},"#,
                r#""clientInfo":{"name":"test","version":"0"}}}"#
            ),
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            concat!(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"#,
                r#""name":"kraken_paper_status","arguments":{}}}"#
            ),
        ],
    );

    assert_eq!(
        audit_events(&stderr, "session_start"),
        1,
        "initialize + tool activity must audit exactly one session_start: {stderr}"
    );
}
