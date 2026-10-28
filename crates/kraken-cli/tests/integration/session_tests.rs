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
        // The refused connect exits on its own; nextest's slow-timeout is the
        // hang guard, so a per-test deadline would only add a contention flake.
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
    // argument parsing (exit code 2, usage error on stderr) before the command
    // runs.
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
/// the source tape ref and the table gains a `Market data` column ("live"
/// otherwise). Named for the feed, not the funds — a paper session reads "live"
/// here too, so this column must never be mistaken for the money's mode.
#[test]
fn session_list_carries_replay_provenance() {
    use kraken_session::{
        manifest::{SessionSource, SessionWindow},
        timeline::fixtures,
    };

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
    assert!(rendered.contains("Market data"), "{rendered}");
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
    use kraken_session::{manifest::SessionWindow, timeline::fixtures};

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
            "{\"timestamp\":\"2026-01-01T00:01:00Z\",\"kind\":\"buy\",\"symbol\":\"BTCUSD\",\"\
             reason\":\"dip\",\"order_id\":\"PAPER-00001\"}\n",
            "{\"timestamp\":\"2026-01-01T00:02:00Z\",\"kind\":\"skip\",\"symbol\":\"ETHUSD\",\"\
             reason\":\"no edge\"}\n",
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
    use kraken_session::{manifest::SessionWindow, timeline::fixtures};

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
    use kraken_session::{manifest::SessionWindow, timeline::fixtures};

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
    use kraken_session::{manifest::SessionWindow, timeline::fixtures};

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

/// A replay source is only meaningful inside a paper workspace.
/// On the real account the refusal fires before anything is allocated — no
/// session directory, no journal anchor, no tape lookup.
#[test]
fn replay_start_on_the_real_account_is_refused_before_allocation() {
    let home = tempfile::tempdir().unwrap();
    super::common::kraken_in(&home)
        .args(["session", "start", "--from", "tape:whatever", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("paper workspace"));
    assert!(
        !super::common::config_dir_in(&home)
            .join("sessions")
            .exists(),
        "the refusal must precede session allocation"
    );
}

/// Plants an open-window replay session (`s1`) under `scope` — the shape a
/// crashed recorder leaves behind: source stamped, no summary, no ended_at.
fn plant_open_replay_session(scope: &std::path::Path) {
    use kraken_session::{
        manifest::{SessionSource, SessionWindow},
        timeline::fixtures,
    };

    let mut manifest = fixtures::manifest(vec![fixtures::jsonl_recording()]);
    manifest.id = "s1".to_string();
    manifest.window = SessionWindow {
        session: "s1".to_string(),
        started_at: "2026-01-01T00:00:00Z".parse().unwrap(),
        opening_equity: rust_decimal_macros::dec!(10_000),
        opening_complete: true,
        ended_at: None,
    };
    manifest.source = Some(SessionSource {
        tape: "tape:jun-crash".to_string(),
        speed: 4.0,
        anchor: "2026-01-01T00:00:00Z".parse().unwrap(),
        started_at: "2026-01-01T00:00:01Z".parse().unwrap(),
        content_hash: None,
    });
    std::fs::create_dir_all(kraken_session::session::session_dir(scope, 1)).unwrap();
    manifest
        .save(&kraken_session::session::session_file_path(scope, 1))
        .unwrap();
}

/// A bug-era replay session already sitting on the real account
/// must not settle simulated fills on its way out — neither `stop` nor the
/// read-only `show` (whose open-window valuation folds durably) may touch
/// the mirrored journal.
#[test]
fn a_bug_era_real_account_replay_session_refuses_stop_and_show() {
    let home = tempfile::tempdir().unwrap();
    plant_open_replay_session(&super::common::config_dir_in(&home));

    super::common::kraken_in(&home)
        .args(["session", "show", "--session", "s1", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("ledger of real executions"));

    super::common::kraken_in(&home)
        .args(["session", "stop", "--session", "s1", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("cannot be closed"));
}

/// The same fold is unreachable through a live workspace — an
/// aborted paper replay session that survived `workspace promote` must not
/// settle tape fills into the allocation's real-money journal when shown.
#[test]
fn an_open_replay_session_in_a_live_workspace_refuses_show() {
    let home = tempfile::tempdir().unwrap();
    let ws = super::common::config_dir_in(&home)
        .join("workspaces")
        .join("promoted");
    std::fs::create_dir_all(&ws).unwrap();
    let manifest = serde_json::json!({
        "workspace_version": "1.0",
        "cli_version": "0.0.0",
        "name": "promoted",
        "capital": "1000",
        "currency": "USD",
        "mode": "live",
        "fee_rate": "0.0026",
        "slippage_rate": "0",
        "created_at": "2026-01-01T00:00:00Z"
    });
    std::fs::write(ws.join("workspace.json"), manifest.to_string()).unwrap();
    plant_open_replay_session(&ws);

    super::common::kraken_in(&home)
        .args([
            "--workspace",
            "promoted",
            "session",
            "show",
            "--session",
            "s1",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("ledger of real executions"));
}

/// A live workspace manifest named `promoted`, planted directly — the end
/// state a promotion reaches, without needing a venue.
fn plant_promoted_workspace(home: &tempfile::TempDir) -> std::path::PathBuf {
    let ws = super::common::config_dir_in(home)
        .join("workspaces")
        .join("promoted");
    std::fs::create_dir_all(&ws).unwrap();
    let manifest = serde_json::json!({
        "workspace_version": "1.0",
        "cli_version": "0.0.0",
        "name": "promoted",
        "capital": "1000",
        "currency": "USD",
        "mode": "live",
        "fee_rate": "0.0026",
        "slippage_rate": "0",
        "created_at": "2026-01-01T00:00:00Z"
    });
    std::fs::write(ws.join("workspace.json"), manifest.to_string()).unwrap();
    ws
}

/// A promoted live workspace whose journal carries the promotion marker at
/// `2026-01-02T00:00:00Z`, plus one stopped session sealed over the given
/// window — the fixture behind the money-source stamping tests.
fn plant_promoted_workspace_with_stopped_session(
    home: &tempfile::TempDir,
    started_at: &str,
    ended_at: &str,
) {
    use kraken_session::{
        manifest::{SessionOutcome, SessionWindow},
        timeline::fixtures,
    };

    let ws = plant_promoted_workspace(home);
    let init = serde_json::json!({
        "v": 1, "ts": "2026-01-01T00:00:00Z", "origin": "cli", "event": "initialized",
        "balance": "1000", "currency": "USD", "fee_rate": "0.0026", "slippage_rate": "0"
    });
    let start = serde_json::json!({
        "v": 1, "ts": started_at, "origin": "cli", "event": "command",
        "name": "session start", "outcome": {"result": "ok"}, "session": "s1"
    });
    let stop = serde_json::json!({
        "v": 1, "ts": ended_at, "origin": "cli", "event": "command",
        "name": "session stop", "outcome": {"result": "ok"}, "session": "s1"
    });
    let marker = serde_json::json!({
        "v": 1, "ts": "2026-01-02T00:00:00Z", "origin": "cli",
        "event": "fill_source_changed", "from": "simulated", "to": "venue"
    });
    // Journal order follows time: a pre-promotion window closes before the
    // marker lands, a post-promotion one opens after it. Plain string order
    // is sound here — every stamp is same-length RFC 3339 UTC.
    let lines = if ended_at < "2026-01-02T00:00:00Z" {
        [init, start, stop, marker]
    } else {
        [init, marker, start, stop]
    };
    let journal = lines.map(|line| format!("{line}\n")).concat();
    std::fs::write(ws.join("journal.jsonl"), journal).unwrap();

    let mut session = fixtures::manifest(vec![fixtures::jsonl_recording()]);
    session.id = "s1".to_string();
    session.summary = Some(SessionOutcome {
        ended_at: ended_at.parse().unwrap(),
        final_value: rust_decimal_macros::dec!(10_000),
        pnl: rust_decimal_macros::dec!(0),
    });
    session.window = SessionWindow {
        session: "s1".to_string(),
        started_at: started_at.parse().unwrap(),
        opening_equity: rust_decimal_macros::dec!(10_000),
        opening_complete: true,
        ended_at: Some(ended_at.parse().unwrap()),
    };
    std::fs::create_dir_all(kraken_session::session::session_dir(&ws, 1)).unwrap();
    session
        .save(&kraken_session::session::session_file_path(&ws, 1))
        .unwrap();
}

/// A session sealed before the workspace went live earned simulated
/// money — after promotion its `session show` must stamp `paper_era`, never
/// the real-money allocation scope.
#[test]
fn a_pre_promotion_session_shows_as_paper_era() {
    let home = tempfile::tempdir().unwrap();
    plant_promoted_workspace_with_stopped_session(
        &home,
        "2026-01-01T00:00:00Z",
        "2026-01-01T12:00:00Z",
    );
    let out = super::common::kraken_in(&home)
        .args([
            "--workspace",
            "promoted",
            "session",
            "show",
            "--session",
            "s1",
            "-o",
            "json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["scope"], "paper_era");
    assert_eq!(json["mode"], "paper");
}

/// An aborted pre-promotion session (window never sealed) is still paper
/// history — promotion only refuses *recording* sessions, so it survives.
/// The listing's JSON names it in `paper_era` rather than letting the live
/// scope stamp claim its simulated fills as real-money evidence.
#[test]
fn session_list_names_an_aborted_pre_promotion_session_as_paper_era() {
    use kraken_session::{manifest::SessionWindow, timeline::fixtures};

    let home = tempfile::tempdir().unwrap();
    let ws = plant_promoted_workspace(&home);
    let init = serde_json::json!({
        "v": 1, "ts": "2026-01-01T00:00:00Z", "origin": "cli", "event": "initialized",
        "balance": "1000", "currency": "USD", "fee_rate": "0.0026", "slippage_rate": "0"
    });
    let start = serde_json::json!({
        "v": 1, "ts": "2026-01-01T06:00:00Z", "origin": "cli", "event": "command",
        "name": "session start", "outcome": {"result": "ok"}, "session": "s1"
    });
    let marker = serde_json::json!({
        "v": 1, "ts": "2026-01-02T00:00:00Z", "origin": "cli",
        "event": "fill_source_changed", "from": "simulated", "to": "venue"
    });
    let journal = [init, start, marker]
        .map(|line| format!("{line}\n"))
        .concat();
    std::fs::write(ws.join("journal.jsonl"), journal).unwrap();

    let mut session = fixtures::manifest(vec![fixtures::jsonl_recording()]);
    session.id = "s1".to_string();
    session.window = SessionWindow {
        session: "s1".to_string(),
        started_at: "2026-01-01T06:00:00Z".parse().unwrap(),
        opening_equity: rust_decimal_macros::dec!(1000),
        opening_complete: true,
        ended_at: None,
    };
    std::fs::create_dir_all(kraken_session::session::session_dir(&ws, 1)).unwrap();
    session
        .save(&kraken_session::session::session_file_path(&ws, 1))
        .unwrap();

    let out = super::common::kraken_in(&home)
        .args(["--workspace", "promoted", "session", "list", "-o", "json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["paper_era"], serde_json::json!(["s1"]));
    assert_eq!(json["mode"], "live", "the scope itself is still live");
}

/// A session sealed after promotion carried real venue fills and keeps the
/// live-allocation stamp.
#[test]
fn a_post_promotion_session_keeps_the_live_scope_stamp() {
    let home = tempfile::tempdir().unwrap();
    plant_promoted_workspace_with_stopped_session(
        &home,
        "2026-01-02T06:00:00Z",
        "2026-01-02T12:00:00Z",
    );
    let out = super::common::kraken_in(&home)
        .args([
            "--workspace",
            "promoted",
            "session",
            "show",
            "--session",
            "s1",
            "-o",
            "json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["scope"], "allocation");
    assert_eq!(json["mode"], "live");
}

/// The strategy stamp reads back through `session show` — the
/// knobs a driver was started with are no longer write-only.
#[test]
fn session_show_carries_the_strategy_stamp() {
    use kraken_session::{
        manifest::{SessionOutcome, SessionWindow, StrategyRef},
        timeline::fixtures,
    };

    let home = tempfile::tempdir().unwrap();
    let base = super::common::config_dir_in(&home);
    let mut manifest = fixtures::manifest(vec![fixtures::jsonl_recording()]);
    manifest.id = "s1".to_string();
    manifest.strategy = Some(StrategyRef {
        name: "recipe-playground-dca".to_string(),
        params: Some(serde_json::json!({"dollars_per_buy": 100})),
    });
    manifest.summary = Some(SessionOutcome {
        ended_at: "2026-01-01T00:30:00Z".parse().unwrap(),
        final_value: rust_decimal_macros::dec!(10_000),
        pnl: rust_decimal_macros::dec!(0),
    });
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

    let out = super::common::kraken_in(&home)
        .args(["session", "show", "--session", "s1", "-o", "json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["strategy"]["name"], "recipe-playground-dca");
    assert_eq!(json["strategy"]["params"]["dollars_per_buy"], 100);
}
