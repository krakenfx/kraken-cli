//! `kraken tape list` end to end: refs, states, and the one-bad-file rule.

use predicates::prelude::*;

use super::common::{config_dir_in, kraken_in};

/// A finalized sidecar in the exact pinned shape the recorder stamps.
pub(crate) fn write_tape(home: &tempfile::TempDir, name: &str, finalized: bool) {
    let dir = config_dir_in(home).join("tapes");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.jsonl")), "").unwrap();
    let mut sidecar = serde_json::json!({
        "schema_version": "1.0",
        "source": "kraken-spot-ws-v2",
        "symbols": ["BTC/USD"],
        "channels": ["trade"],
        "window_start": "2026-01-01T00:00:00Z",
        "cli_version": "0.0.0-test",
    });
    if finalized {
        for (key, value) in [
            ("window_end", serde_json::json!("2026-01-01T01:00:00Z")),
            ("events_dropped", serde_json::json!(0)),
            ("frames_unparsed", serde_json::json!(0)),
            ("reconnect_count", serde_json::json!(0)),
        ] {
            sidecar[key] = value;
        }
    }
    std::fs::write(
        dir.join(format!("{name}.jsonl.meta.json")),
        sidecar.to_string(),
    )
    .unwrap();
}

#[test]
fn tape_list_of_a_fresh_home_is_empty_not_an_error() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["tape", "list", "-o", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"tapes\":[]"))
        .stdout(predicate::str::contains("\"group\":\"tape\""));
}

#[test]
fn tape_list_names_library_tapes_and_run_tapes_as_refs() {
    let home = tempfile::tempdir().unwrap();
    write_tape(&home, "jun-crash", true);
    // A run tape: the same recording layout under the scope's sessions/r1/.
    let session_dir = config_dir_in(&home).join("sessions").join("s1");
    std::fs::create_dir_all(&session_dir).unwrap();
    let tapes_dir = config_dir_in(&home).join("tapes");
    std::fs::rename(
        tapes_dir.join("jun-crash.jsonl"),
        session_dir.join("tape.jsonl"),
    )
    .unwrap();
    std::fs::rename(
        tapes_dir.join("jun-crash.jsonl.meta.json"),
        session_dir.join("tape.jsonl.meta.json"),
    )
    .unwrap();
    write_tape(&home, "jun-chop", false);

    let out = kraken_in(&home)
        .args(["tape", "list", "-o", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let tapes = json["tapes"].as_array().unwrap();
    assert_eq!(tapes.len(), 2);
    // Refs sort as strings: run:* precedes tape:*.
    assert_eq!(tapes[0]["tape"], "session:s1");
    assert_eq!(tapes[0]["state"], "finalized");
    assert_eq!(tapes[0]["integrity"]["events_dropped"], 0);
    assert_eq!(tapes[0]["in_use"], false);
    assert_eq!(tapes[1]["tape"], "tape:jun-chop");
    assert_eq!(tapes[1]["state"], "unfinalized");
}

#[test]
fn tape_list_ignores_the_active_workspace() {
    // Market memory is global: inside a workspace, the one shared library
    // still lists — tapes never fragment per account.
    let home = tempfile::tempdir().unwrap();
    write_tape(&home, "jun-chop", true);
    std::fs::create_dir_all(config_dir_in(&home).join("workspaces").join("w1")).unwrap();

    let out = kraken_in(&home)
        .args(["--workspace", "w1", "tape", "list", "-o", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["tapes"][0]["tape"], "tape:jun-chop");
}

#[test]
fn tape_list_works_without_the_named_workspace_existing() {
    // Global market memory never resolves the workspace, so the existence
    // gate must not fire here.
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["--workspace", "ghost", "tape", "list", "-o", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"tapes\":[]"));
}

#[test]
fn one_damaged_tape_never_kills_the_listing() {
    let home = tempfile::tempdir().unwrap();
    write_tape(&home, "healthy", true);
    let dir = config_dir_in(&home).join("tapes");
    std::fs::write(dir.join("broken.jsonl"), "").unwrap();
    std::fs::write(dir.join("broken.jsonl.meta.json"), "not json").unwrap();

    let out = kraken_in(&home)
        .args(["tape", "list", "-o", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let tapes = json["tapes"].as_array().unwrap();
    assert_eq!(tapes.len(), 2);
    assert_eq!(tapes[0]["tape"], "tape:broken");
    assert_eq!(tapes[0]["state"], "damaged");
    assert!(tapes[0]["detail"].as_str().unwrap().contains("sidecar"));
    assert_eq!(tapes[1]["tape"], "tape:healthy");
    assert_eq!(tapes[1]["state"], "finalized");
}
