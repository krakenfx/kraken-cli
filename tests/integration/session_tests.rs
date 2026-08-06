//! End-to-end CLI wiring tests for `kraken session`.
//!
//! These run the real binary and assert `start` validates the session contract
//! before any socket or file is touched. A session is a *recorded* window an
//! agent trades through and later replays, so a stdout-only `--to` (which
//! persists nothing) must be rejected up front — the guard fires before the
//! recorder opens a socket, so no live/mock server is needed.
//! `HOME`/`XDG_CONFIG_HOME` point at a temp dir so a session never touches the
//! developer's real state.

use assert_cmd::Command;
use predicates::prelude::*;

#[allow(deprecated)]
fn kraken() -> Command {
    Command::cargo_bin("kraken").unwrap()
}

#[test]
fn session_start_rejects_stdout_only_to() {
    // `--to stdout` leaves no durable tape, so the session would have nothing to
    // trade against or replay (and no recording lock to derive liveness
    // from). The contract guard rejects it as a validation error — a JSON
    // envelope on stdout — before any socket opens, and names the durable
    // backends to use.
    let home = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args([
            "session",
            "start",
            "--symbols",
            "BTC/USD",
            "--channels",
            "ticker",
            "--to",
            "stdout",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("must record durably"))
        .stdout(predicate::str::contains("duckdb"))
        .stdout(predicate::str::contains("jsonl"));
}

#[test]
fn session_start_does_not_announce_when_the_stream_never_comes_up() {
    // `session_started` is gated on the stream going live (`stream::run` fires
    // `on_ready` on the first subscribe ack), so a start that can't connect
    // must print no session handle and exit non-zero — never announce a session the
    // recorder never began. Point the public WS at a rejected URL so the session
    // fails fast; `--to jsonl` is durable, so the session reaches `stream::run`
    // (the contract guard passes).
    let home = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env("KRAKEN_WS_PUBLIC_URL", "ws://127.0.0.1:1")
        .env("KRAKEN_WS_RECONNECT_BASE_MS", "1")
        .timeout(std::time::Duration::from_secs(30)) // safety net; the refused connect is far faster
        .args([
            "session",
            "start",
            "--symbols",
            "BTC/USD",
            "--channels",
            "ticker",
            "--to",
            "jsonl",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("session_started").not());
}

#[test]
fn session_start_unknown_to_target_is_rejected_at_parse_time() {
    // `--to bogus` is not a `SinkTarget` value, so clap rejects it during
    // argument parsing (exit code 2, usage error on stderr) before the command runs.
    kraken()
        .args([
            "session",
            "start",
            "--symbols",
            "BTC/USD",
            "--channels",
            "ticker",
            "--to",
            "bogus",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid value 'bogus'"));
}

#[test]
fn session_note_without_an_active_session_is_refused() {
    // A rationale must land in the window it explains; with nothing
    // recording there is no window.
    let home = tempfile::tempdir().unwrap();
    kraken()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args([
            "session",
            "note",
            "--kind",
            "skip",
            "--reason",
            "spread too wide",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("no active session"));
}

/// The listing distinguishes replay evidence from live: the JSON row carries
/// the source tape ref and the table gains a Source column ("live" otherwise).
#[test]
fn session_list_carries_replay_provenance() {
    use kraken_session::manifest::{SessionSource, SessionWindow};
    use kraken_session::timeline::fixtures;

    let home = tempfile::tempdir().unwrap();
    let base = super::common::config_dir_in(&home);
    let mut manifest = fixtures::manifest(vec![fixtures::jsonl_recording()]);
    manifest.id = "s1".to_string();
    manifest.window = SessionWindow {
        session: "s1".to_string(),
        started_at: "2026-01-01T00:00:00Z".parse().unwrap(),
        opening_equity: rust_decimal_macros::dec!(10_000),
        opening_complete: true,
        ended_at: Some("2026-01-01T00:30:00Z".parse().unwrap()),
    };
    manifest.source = Some(SessionSource {
        tape: "tape:jun-crash".to_string(),
        speed: 4.0,
        anchor: "2026-01-01T00:00:00Z".parse().unwrap(),
        started_at: "2026-01-01T00:00:01Z".parse().unwrap(),
        content_hash: None,
    });
    std::fs::create_dir_all(kraken_session::session::session_dir(&base, 1)).unwrap();
    manifest
        .save(&kraken_session::session::session_file_path(&base, 1))
        .unwrap();

    let json_out = kraken()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["session", "list", "-o", "json"])
        .output()
        .unwrap();
    assert!(json_out.status.success(), "{json_out:?}");
    let json: serde_json::Value = serde_json::from_slice(&json_out.stdout).unwrap();
    assert_eq!(json["sessions"][0]["source"], "tape:jun-crash");

    let table = kraken()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["session", "list"])
        .output()
        .unwrap();
    let rendered = String::from_utf8_lossy(&table.stdout);
    assert!(rendered.contains("Source"), "{rendered}");
    assert!(rendered.contains("tape:jun-crash"), "{rendered}");
}

#[test]
fn session_list_is_empty_before_any_session() {
    let home = tempfile::tempdir().unwrap();
    let out = kraken()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["session", "list", "-o", "json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["sessions"], serde_json::json!([]));
}

#[test]
fn session_list_warns_when_unscoped_and_stays_quiet_once_scoped() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join(".config");

    // No workspace: the inventory is the default account's, and stderr says so.
    let unscoped = kraken()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", &config)
        .args(["session", "list", "-o", "json"])
        .output()
        .unwrap();
    assert!(unscoped.status.success(), "{unscoped:?}");
    let stderr = String::from_utf8_lossy(&unscoped.stderr);
    assert!(
        stderr.contains("KRAKEN_WORKSPACE"),
        "unscoped list must name the missing scope: {stderr}"
    );

    // Scoped to a paper workspace: the empty list is unambiguous, no warning.
    kraken()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", &config)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "10000",
            "--mode",
            "paper",
        ])
        .assert()
        .success();
    let scoped = kraken()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", &config)
        .env("KRAKEN_WORKSPACE", "w1")
        .args(["session", "list", "-o", "json"])
        .output()
        .unwrap();
    assert!(scoped.status.success(), "{scoped:?}");
    let stderr = String::from_utf8_lossy(&scoped.stderr);
    assert!(
        !stderr.contains("no active workspace"),
        "scoped list must not warn: {stderr}"
    );
}

/// `session decisions`: the round-gate read. Verbatim entries plus a count,
/// resolved through the CLI so an agent never learns the on-disk layout.
#[test]
fn session_decisions_reads_the_log_verbatim_with_a_count() {
    use kraken_session::manifest::SessionWindow;
    use kraken_session::timeline::fixtures;

    let home = tempfile::tempdir().unwrap();
    let base = super::common::config_dir_in(&home);
    let mut manifest = fixtures::manifest(vec![fixtures::jsonl_recording()]);
    manifest.id = "s1".to_string();
    manifest.window = SessionWindow {
        session: "s1".to_string(),
        started_at: "2026-01-01T00:00:00Z".parse().unwrap(),
        opening_equity: rust_decimal_macros::dec!(10_000),
        opening_complete: true,
        ended_at: Some("2026-01-01T00:30:00Z".parse().unwrap()),
    };
    let dir = kraken_session::session::session_dir(&base, 1);
    std::fs::create_dir_all(&dir).unwrap();
    manifest
        .save(&kraken_session::session::session_file_path(&base, 1))
        .unwrap();
    std::fs::write(
        dir.join("decisions.jsonl"),
        concat!(
            "{\"timestamp\":\"2026-01-01T00:01:00Z\",\"kind\":\"buy\",\"symbol\":\"BTCUSD\",\"reason\":\"dip\",\"order_id\":\"PAPER-00001\"}\n",
            "{\"timestamp\":\"2026-01-01T00:02:00Z\",\"kind\":\"skip\",\"symbol\":\"ETHUSD\",\"reason\":\"no edge\"}\n",
        ),
    )
    .unwrap();

    let out = kraken()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["session", "decisions", "--session", "s1", "-o", "json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["session"], "s1");
    assert_eq!(json["count"], 2);
    assert_eq!(json["decisions"][0]["kind"], "buy");
    assert_eq!(json["decisions"][0]["reason"], "dip");
    assert_eq!(json["decisions"][1]["kind"], "skip");
}

/// An unwritten log is zero rounds, loudly distinguishable from a missing
/// session: count 0 on the one hand, a validation refusal on the other.
#[test]
fn session_decisions_empty_log_is_count_zero_and_unknown_ref_refuses() {
    use kraken_session::manifest::SessionWindow;
    use kraken_session::timeline::fixtures;

    let home = tempfile::tempdir().unwrap();
    let base = super::common::config_dir_in(&home);
    let mut manifest = fixtures::manifest(vec![fixtures::jsonl_recording()]);
    manifest.id = "s1".to_string();
    manifest.window = SessionWindow {
        session: "s1".to_string(),
        started_at: "2026-01-01T00:00:00Z".parse().unwrap(),
        opening_equity: rust_decimal_macros::dec!(10_000),
        opening_complete: true,
        ended_at: None,
    };
    std::fs::create_dir_all(kraken_session::session::session_dir(&base, 1)).unwrap();
    manifest
        .save(&kraken_session::session::session_file_path(&base, 1))
        .unwrap();

    let out = kraken()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["session", "decisions", "-o", "json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["count"], 0);
    assert_eq!(json["decisions"], serde_json::json!([]));

    kraken()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["session", "decisions", "--session", "ghost", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("not found"));
}

/// `session state`: the typed cursor. `set` targets the ACTIVE session only
/// (liveness = the recording lock), `get` reads any ref, unset reads as an
/// empty cursor — never an error.
#[test]
fn session_state_set_needs_a_recording_session_and_get_reads_it_back() {
    use kraken_session::manifest::SessionWindow;
    use kraken_session::timeline::fixtures;

    let home = tempfile::tempdir().unwrap();
    let base = super::common::config_dir_in(&home);
    let mut manifest = fixtures::manifest(vec![kraken_session::manifest::RecordingRef {
        file: "tape.jsonl".to_string(),
        ..fixtures::jsonl_recording()
    }]);
    manifest.id = "s1".to_string();
    manifest.window = SessionWindow {
        session: "s1".to_string(),
        started_at: "2026-01-01T00:00:00Z".parse().unwrap(),
        opening_equity: rust_decimal_macros::dec!(10_000),
        opening_complete: true,
        ended_at: None,
    };
    let dir = kraken_session::session::session_dir(&base, 1);
    std::fs::create_dir_all(&dir).unwrap();
    manifest
        .save(&kraken_session::session::session_file_path(&base, 1))
        .unwrap();

    // No recorder holds the tape → the session reads aborted → set refuses.
    kraken()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["session", "state", "set", "--round", "2", "-o", "json"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("no active session"));

    // Hold the tape's writer lock from this process: the CLI's liveness probe
    // now sees a live recorder, so the session is ACTIVE and set lands.
    let _recorder = kraken_recording::FileLock::acquire(&dir.join("tape.jsonl"), "test recorder")
        .expect("lock");
    kraken()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args([
            "session",
            "state",
            "set",
            "--round",
            "2",
            "--zone",
            "above",
            "--legs-done",
            "BTC/USD,SOL/USD",
            "-o",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"round\":2"));

    let out = kraken()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["session", "state", "get", "--session", "s1", "-o", "json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["cursor"]["round"], 2);
    assert_eq!(json["cursor"]["zone"], "above");
    assert_eq!(
        json["cursor"]["legs_done"],
        serde_json::json!(["BTC/USD", "SOL/USD"])
    );
}

#[test]
fn session_state_get_unset_is_an_empty_cursor_not_an_error() {
    use kraken_session::manifest::SessionWindow;
    use kraken_session::timeline::fixtures;

    let home = tempfile::tempdir().unwrap();
    let base = super::common::config_dir_in(&home);
    let mut manifest = fixtures::manifest(vec![fixtures::jsonl_recording()]);
    manifest.id = "s1".to_string();
    manifest.window = SessionWindow {
        session: "s1".to_string(),
        started_at: "2026-01-01T00:00:00Z".parse().unwrap(),
        opening_equity: rust_decimal_macros::dec!(10_000),
        opening_complete: true,
        ended_at: Some("2026-01-01T00:30:00Z".parse().unwrap()),
    };
    std::fs::create_dir_all(kraken_session::session::session_dir(&base, 1)).unwrap();
    manifest
        .save(&kraken_session::session::session_file_path(&base, 1))
        .unwrap();

    let out = kraken()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["session", "state", "get", "-o", "json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["cursor"], serde_json::json!({}));
    assert_eq!(json["updated_at"], serde_json::Value::Null);
}
