//! End-to-end CLI wiring tests for `kraken recipe create/list/show`.
//!
//! These run the real binary against sessions planted on disk inside a paper
//! workspace. The artifact shapes are pinned by kraken-recipe's own suites;
//! here the assertions are the command contract: refusals arrive as
//! validation envelopes, and a create leaves both `recipe.json` and the
//! draft `SKILL.md` behind in the global library.

use super::common::{config_dir_in, kraken_in};
use kraken_paper::{
    AccountEvent, CommandEntry,
    account::{AccountRecord, Origin, RECORD_VERSION},
};
use kraken_session::{
    decision::{Decision, DecisionKind},
    manifest::{SessionOutcome, SessionState, SessionWindow, StrategyRef},
    session::{session_decisions_path, session_dir, session_file_path},
    timeline::fixtures,
};
use predicates::prelude::*;
use rust_decimal_macros::dec;
use std::path::Path;

/// Creates the `w1` paper workspace through the binary and returns its scope
/// dir, so planted sessions live where `--workspace w1` resolves them.
fn provision_workspace(home: &tempfile::TempDir) -> std::path::PathBuf {
    kraken_in(home)
        .args([
            "workspace",
            "create",
            "w1",
            "--capital",
            "10000",
            "--mode",
            "paper",
            "-o",
            "json",
        ])
        .assert()
        .success();
    config_dir_in(home).join("workspaces").join("w1")
}

/// Plants session `s1` in `scope`: manifest, tape, journal window markers,
/// and two alert decisions. `stopped` controls whether the window closed;
/// `strategy` controls whether a strategy label was recorded.
fn plant_session(scope: &Path, stopped: bool, strategy: bool) {
    let mut manifest = fixtures::manifest(vec![kraken_session::manifest::RecordingRef {
        file: "tape.jsonl".to_string(),
        ..fixtures::jsonl_recording()
    }]);
    manifest.id = "s1".to_string();
    manifest.status = if stopped {
        SessionState::Stopped
    } else {
        SessionState::Running
    };
    if strategy {
        manifest.strategy = Some(StrategyRef {
            name: "recipe-playground-price-alert".to_string(),
            params: Some(serde_json::json!({"upper": 70000, "lower": 60000})),
        });
    }
    if stopped {
        manifest.summary = Some(SessionOutcome {
            ended_at: "2026-01-01T00:30:00Z".parse().unwrap(),
            final_value: dec!(10_000),
            pnl: dec!(0),
        });
    }
    manifest.window = SessionWindow {
        session: "s1".to_string(),
        started_at: "2026-01-01T00:00:00.500Z".parse().unwrap(),
        opening_equity: dec!(10_000),
        opening_complete: true,
        ended_at: stopped.then(|| "2026-01-01T00:30:00Z".parse().unwrap()),
    };
    manifest.save(&session_file_path(scope, 1)).unwrap();
    fixtures::write_run_tape(
        &session_dir(scope, 1),
        &["2026-01-01T00:00:01Z", "2026-01-01T00:00:03Z"],
    );

    let decisions: Vec<String> = ["2026-01-01T00:05:00Z", "2026-01-01T00:10:00Z"]
        .iter()
        .map(|ts| {
            serde_json::to_string(&Decision {
                timestamp: ts.parse().unwrap(),
                kind: DecisionKind::Alert,
                symbol: Some("BTCUSD".to_string()),
                reason: "level crossed".to_string(),
                order_id: None,
            })
            .unwrap()
        })
        .collect();
    std::fs::write(
        session_decisions_path(scope, 1),
        format!("{}\n", decisions.join("\n")),
    )
    .unwrap();

    // Append to the scope journal: workspace create already seeded the
    // funding epoch, so only a bare master scope needs an opening record.
    let journal = scope.join("journal.jsonl");
    let mut records = Vec::new();
    if !journal.exists() {
        records.push(fixtures::account_record("2026-01-01T00:00:00Z"));
    }
    records.push(marker_record(
        "session start",
        "s1",
        "2026-01-01T00:00:00.500Z",
    ));
    if stopped {
        records.push(marker_record("session stop", "s1", "2026-01-01T00:30:00Z"));
    }
    let mut lines = String::new();
    for record in &records {
        lines.push_str(&serde_json::to_string(record).unwrap());
        lines.push('\n');
    }
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&journal)
        .unwrap();
    file.write_all(lines.as_bytes()).unwrap();
}

fn marker_record(name: &str, run: &str, ts: &str) -> AccountRecord {
    AccountRecord {
        v: RECORD_VERSION,
        ts: ts.parse().unwrap(),
        origin: Origin::Cli,
        event: AccountEvent::Command(CommandEntry {
            name: name.to_string(),
            session: Some(run.to_string()),
            ..CommandEntry::default()
        }),
    }
}

#[test]
fn recipe_create_from_a_stopped_session_writes_both_artifacts() {
    let home = tempfile::tempdir().unwrap();
    let scope = provision_workspace(&home);
    plant_session(&scope, true, true);

    let out = kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "recipe",
            "create",
            "btc-levels",
            "-o",
            "json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        json["recipe"]["driver"]["name"],
        "recipe-playground-price-alert"
    );
    assert_eq!(json["recipe"]["params"]["upper"], 70000);
    assert_eq!(json["recipe"]["origin"]["session"], "s1");

    // The library is global: artifacts land in the config dir, not the
    // workspace subtree.
    let recipe_dir = config_dir_in(&home).join("recipes").join("btc-levels");
    assert!(recipe_dir.join("recipe.json").exists());
    let skill = std::fs::read_to_string(recipe_dir.join("SKILL.md")).unwrap();
    assert!(skill.contains("UNRESOLVED RULES"), "{skill}");
    assert!(
        skill.contains("\"upper\": 70000"),
        "params verbatim: {skill}"
    );
    assert!(skill.contains("- alert: 2"), "decision counts: {skill}");
}

#[test]
fn recipe_create_refuses_the_reserved_default_name() {
    // `recipe run` provisions a workspace named after the recipe, and the
    // 'default' workspace is the real account scope — a recipe by that name
    // could be created but never run.
    let home = tempfile::tempdir().unwrap();

    kraken_in(&home)
        .args(["recipe", "create", "default", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("reserved"));
}

#[test]
fn recipe_create_outside_a_paper_workspace_is_refused() {
    let home = tempfile::tempdir().unwrap();
    plant_session(&config_dir_in(&home), true, true);

    kraken_in(&home)
        .args(["recipe", "create", "leaky", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("paper workspace"));
}

#[test]
fn recipe_create_without_a_strategy_label_needs_an_explicit_driver() {
    let home = tempfile::tempdir().unwrap();
    let scope = provision_workspace(&home);
    plant_session(&scope, true, false);

    kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "recipe",
            "create",
            "bare",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("--driver"));

    kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "recipe",
            "create",
            "bare",
            "--driver",
            "recipe-playground-price-alert",
            "-o",
            "json",
        ])
        .assert()
        .success();
}

#[test]
fn recipe_create_refuses_a_taken_name_without_overwrite() {
    let home = tempfile::tempdir().unwrap();
    let scope = provision_workspace(&home);
    plant_session(&scope, true, true);

    kraken_in(&home)
        .args(["--workspace", "w1", "recipe", "create", "btc", "-o", "json"])
        .assert()
        .success();
    kraken_in(&home)
        .args(["--workspace", "w1", "recipe", "create", "btc", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("--overwrite"));
    kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "recipe",
            "create",
            "btc",
            "--overwrite",
            "-o",
            "json",
        ])
        .assert()
        .success();
}

#[test]
fn recipe_create_refuses_a_still_recording_session() {
    let home = tempfile::tempdir().unwrap();
    let scope = provision_workspace(&home);
    plant_session(&scope, false, true);
    // Hold the tape's writer lock from this process: the CLI's liveness
    // probe now sees a live recorder, so the session is still recording.
    let _recorder = kraken_recording::FileLock::acquire(
        &session_dir(&scope, 1).join("tape.jsonl"),
        "test recorder",
    )
    .expect("lock");

    kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "recipe",
            "create",
            "early",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("still recording"));
}

#[test]
fn recipe_list_and_show_report_the_library() {
    let home = tempfile::tempdir().unwrap();
    let scope = provision_workspace(&home);
    plant_session(&scope, true, true);

    // A fresh library is never empty: the built-in demo recipe fills it
    // until an on-disk recipe of the same name shadows it.
    let fresh = kraken_in(&home)
        .args(["recipe", "list", "-o", "json"])
        .output()
        .unwrap();
    assert!(fresh.status.success(), "{fresh:?}");
    let json: serde_json::Value = serde_json::from_slice(&fresh.stdout).unwrap();
    assert_eq!(json["recipes"].as_array().unwrap().len(), 1);
    assert_eq!(json["recipes"][0]["name"], "playground");
    assert_eq!(json["recipes"][0]["builtin"], true);

    kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "recipe",
            "create",
            "btc-levels",
            "-o",
            "json",
        ])
        .assert()
        .success();

    // list/show read the global library and need no workspace scope.
    kraken_in(&home)
        .args(["recipe", "list", "-o", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\":\"btc-levels\""))
        .stdout(predicate::str::contains("recipe-playground-price-alert"));

    let shown = kraken_in(&home)
        .args(["recipe", "show", "btc-levels", "-o", "json"])
        .output()
        .unwrap();
    assert!(shown.status.success(), "{shown:?}");
    let json: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(json["recipe"]["driver"]["kind"], "skill");
    assert_eq!(json["recipe"]["params"]["lower"], 60000);
    assert!(
        json["skill_path"].as_str().unwrap().ends_with("SKILL.md"),
        "{json}"
    );

    kraken_in(&home)
        .args(["recipe", "show", "ghost", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("kraken recipe list"));
}

/// Spawns `kraken` against a dead WS endpoint and reads stderr until every
/// needle appears (or a generous deadline passes), then kills the child.
/// Waiting on the output itself instead of racing a fixed timeout keeps
/// these tests honest under full-suite load — the earlier fixed windows
/// flaked when a saturated box delayed the spawn past them.
fn run_streaming_until_stderr(home: &tempfile::TempDir, args: &[&str], needles: &[&str]) -> String {
    use std::io::BufRead as _;

    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("kraken"))
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env_remove("KRAKEN_SESSION")
        .env_remove("KRAKEN_WORKSPACE")
        .env("KRAKEN_WS_PUBLIC_URL", "wss://127.0.0.1:1")
        .env("KRAKEN_DANGER_ALLOW_ANY_URL_HOST", "1")
        .env("KRAKEN_WS_RECONNECT_BASE_MS", "1000")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut buf = String::new();
    while !needles.iter().all(|needle| buf.contains(needle)) {
        let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) else {
            break;
        };
        match rx.recv_timeout(left) {
            Ok(line) => {
                buf.push_str(&line);
                buf.push('\n');
            }
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    drop(rx);
    let _ = reader.join();
    buf
}

/// `recipe run` provisions the recipe's workspace exactly once
/// (never re-funding it) and hands off on stderr. A dead WS endpoint keeps
/// the test offline: everything asserted prints before the stream connects.
#[test]
fn recipe_run_provisions_the_workspace_once_and_reuses_it() {
    let home = tempfile::tempdir().unwrap();
    let scope = provision_workspace(&home);
    plant_session(&scope, true, true);
    kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "recipe",
            "create",
            "btc-levels",
            "-o",
            "json",
        ])
        .assert()
        .success();

    let first = run_streaming_until_stderr(
        &home,
        &["recipe", "run", "btc-levels", "-o", "json"],
        &["btc-levels workspace created", "recipe:     btc-levels"],
    );
    assert!(first.contains("btc-levels workspace created"), "{first}");
    assert!(first.contains("recipe:     btc-levels"), "{first}");
    assert!(
        config_dir_in(&home)
            .join("workspaces")
            .join("btc-levels")
            .join("workspace.json")
            .exists(),
        "the run provisions a workspace named after the recipe"
    );

    // The handoff line prints after the provisioning line would have, so
    // seeing it proves the created-line's absence is real, not a race.
    let second = run_streaming_until_stderr(
        &home,
        &["recipe", "run", "btc-levels", "-o", "json"],
        &["recipe:     btc-levels"],
    );
    assert!(
        !second.contains("workspace created"),
        "reuse never re-funds: {second}"
    );
}

/// The shipped `playground` recipe runs like any stored one,
/// provisioning the demo workspace.
#[test]
fn recipe_run_of_the_builtin_provisions_the_demo_workspace() {
    let home = tempfile::tempdir().unwrap();
    let buf = run_streaming_until_stderr(
        &home,
        &["recipe", "run", "playground", "-o", "json"],
        &["playground workspace created", "built-in"],
    );
    assert!(buf.contains("playground workspace created"), "{buf}");
    assert!(buf.contains("built-in"), "{buf}");
}

#[test]
fn recipe_run_of_a_missing_recipe_is_refused() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["recipe", "run", "ghost", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("kraken recipe list"));
}

/// `--real-account` conflicts with `--from` at the parser, and
/// without a TTY it refuses unless `--yes` opts in.
#[test]
fn recipe_run_real_account_conflicts_with_replay_and_needs_a_tty_or_yes() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "recipe",
            "run",
            "playground",
            "--real-account",
            "--from",
            "tape:x",
        ])
        .assert()
        .failure()
        .code(2);

    kraken_in(&home)
        .args([
            "recipe",
            "run",
            "playground",
            "--real-account",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("confirmation needed"));
}

/// The replay wiring reaches the tape resolver inside the paper
/// workspace — a missing tape is the tape's error, not a scope refusal.
#[test]
fn recipe_run_replay_resolves_the_tape_in_the_paper_workspace() {
    let home = tempfile::tempdir().unwrap();
    // No per-test timeout: the missing tape makes this exit on its own in ~3s,
    // so a harness deadline only ever fires when a saturated run makes the spawn
    // slow. Hangs are bounded globally by nextest's slow-timeout instead.
    kraken_in(&home)
        .args([
            "recipe",
            "run",
            "playground",
            "--from",
            "tape:ghost",
            "--speed",
            "1000",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("ghost"))
        .stdout(predicate::str::contains("settles fills from recorded prices").not());
}

/// `session.json` carries the recipe's strategy stamp and the
/// `--experiment` override — written before the stream opens, and the stamp
/// is what `lab compare` discovers runs by.
#[test]
fn recipe_run_stamps_strategy_params_and_experiment() {
    let home = tempfile::tempdir().unwrap();
    let scope = provision_workspace(&home);
    plant_session(&scope, true, true);
    kraken_in(&home)
        .args([
            "--workspace",
            "w1",
            "recipe",
            "create",
            "btc-levels",
            "-o",
            "json",
        ])
        .assert()
        .success();

    let manifest_path = config_dir_in(&home)
        .join("workspaces")
        .join("btc-levels")
        .join("sessions")
        .join("s1")
        .join("session.json");
    // session.json lands before the stream connects; the handoff line
    // precedes it, so wait for the file itself rather than a fixed window.
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("kraken"))
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env_remove("KRAKEN_SESSION")
        .env_remove("KRAKEN_WORKSPACE")
        .env("KRAKEN_WS_PUBLIC_URL", "wss://127.0.0.1:1")
        .env("KRAKEN_DANGER_ALLOW_ANY_URL_HOST", "1")
        .env("KRAKEN_WS_RECONNECT_BASE_MS", "1000")
        .args([
            "recipe",
            "run",
            "btc-levels",
            "--experiment",
            "e1",
            "-o",
            "json",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !manifest_path.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();

    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["experiment"], "e1");
    assert_eq!(
        manifest["strategy"]["name"],
        "recipe-playground-price-alert"
    );
    assert_eq!(manifest["strategy"]["params"]["upper"], 70000);
}
