//! End-to-end CLI wiring tests for `kraken lab score` and `kraken lab compare`.
//!
//! These run the real binary and assert the session contract: an unknown session ref
//! and no runs at all are validation errors in the JSON envelope, and the
//! ref defaults to `latest`. The scorecard math is pinned by kraken-lab's
//! own suites; the compare tests pin the ticket ACs — cells byte-equal to
//! `lab score`, failures as named error columns — over real fixture runs on
//! disk.

use std::path::Path;

use kraken_paper::account::{AccountRecord, Origin, RECORD_VERSION};
use kraken_paper::{AccountEvent, CommandEntry};
use kraken_session::manifest::SessionOutcome;
use kraken_session::manifest::SessionWindow;
use kraken_session::session::{session_dir, session_file_path};
use kraken_session::timeline::fixtures;
use predicates::prelude::*;
use rust_decimal_macros::dec;

use super::common::{config_dir_in, kraken, kraken_in};

#[test]
fn lab_score_with_no_runs_names_what_exists() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["lab", "score", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("session 'latest' not found"));
}

#[test]
fn lab_score_of_an_unknown_run_is_a_validation_error() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["lab", "score", "--session", "ghost", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("not found"));
}

#[test]
fn lab_new_freezes_and_show_verifies_the_seal() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "lab",
            "new",
            "momentum-1",
            "--hypothesis",
            "buying strength beats the tape",
            "--min-return-pct",
            "1",
            "--min-fills",
            "2",
            "-o",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"frozen\":\"sha256:"));

    kraken_in(&home)
        .args(["lab", "show", "momentum-1", "-o", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\":\"momentum-1\""))
        .stdout(predicate::str::contains("\"min_return_pct\":1"));
}

#[test]
fn lab_new_refuses_a_second_freeze_under_the_same_name() {
    let home = tempfile::tempdir().unwrap();
    let freeze = |hypothesis: &str| {
        kraken_in(&home)
            .args([
                "lab",
                "new",
                "m1",
                "--hypothesis",
                hypothesis,
                "--min-fills",
                "1",
                "-o",
                "json",
            ])
            .assert()
    };
    freeze("first").success();
    freeze("second")
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("immutable"));
}

#[test]
fn lab_new_without_criteria_is_a_validation_error() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["lab", "new", "m1", "--hypothesis", "h", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("criterion"));
}

#[test]
fn lab_show_of_a_missing_experiment_points_at_lab_new() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["lab", "show", "ghost", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("kraken lab new ghost"));
}

#[test]
fn lab_score_against_a_missing_experiment_fails_before_scoring() {
    let home = tempfile::tempdir().unwrap();
    write_run(&config_dir_in(&home), "ghost", 1, true);
    kraken_in(&home)
        .args([
            "lab",
            "score",
            "--session",
            "s1",
            "--experiment",
            "ghost",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("experiment 'ghost' not found"));
}

#[test]
fn lab_score_help_names_the_run_ref() {
    kraken()
        .args(["lab", "score", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--session"))
        .stdout(predicate::str::contains("latest"));
}

#[test]
fn lab_compare_of_a_missing_experiment_is_a_validation_error() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["lab", "compare", "ghost", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("experiment 'ghost' not found"));
}

#[test]
fn lab_compare_of_a_spec_whose_filename_diverges_from_its_seal_fails_loud() {
    let home = tempfile::tempdir().unwrap();
    let base = config_dir_in(&home);
    freeze(&home, "orig");
    // Rename the sealed file: its internal name stays "orig", so it still
    // hash-verifies — only the filename diverges (the same shape a case-only
    // mismatch takes on a case-insensitive filesystem). Without the load guard
    // this silently discovered zero runs and reported an empty comparison; it
    // must instead fail loud so discovery never keys off a foreign identity.
    let experiments = base.join("lab").join("experiments");
    std::fs::rename(
        experiments.join("orig.json"),
        experiments.join("aliased.json"),
    )
    .unwrap();

    kraken_in(&home)
        .args(["lab", "compare", "aliased", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"parse\""))
        .stdout(predicate::str::contains("sealed under the name 'orig'"));
}

#[test]
fn lab_compare_with_no_runs_is_an_empty_comparison_not_an_error() {
    let home = tempfile::tempdir().unwrap();
    freeze(&home, "cmp-empty");
    let out = kraken_in(&home)
        .args(["lab", "compare", "cmp-empty", "-o", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let data: serde_json::Value = serde_json::from_slice(&out).unwrap();
    // The top-level keys are the comparison's pinned JSON contract.
    assert_eq!(data["group"], "lab");
    assert_eq!(data["type"], "comparison");
    assert_eq!(data["experiment"], "cmp-empty");
    assert!(
        data["frozen"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
    assert_eq!(data["sessions"], serde_json::json!([]));
    assert_eq!(data["pass_count"], 0);
    assert_eq!(data["total"], 0);
    assert!(data["caveats"].as_array().is_some_and(|c| !c.is_empty()));
}

#[test]
fn lab_compare_cells_are_byte_equal_to_lab_score_and_failures_are_named_columns() {
    let home = tempfile::tempdir().unwrap();
    let base = config_dir_in(&home);
    freeze(&home, "cmp-1");
    write_run(&base, "cmp-1", 1, true);
    write_run(&base, "cmp-1", 2, false); // never stopped: no summary

    let score_out = kraken_in(&home)
        .args([
            "lab",
            "score",
            "--session",
            "s1",
            "--experiment",
            "cmp-1",
            "-o",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let score: serde_json::Value = serde_json::from_slice(&score_out).unwrap();

    let compare_out = kraken_in(&home)
        .args(["lab", "compare", "cmp-1", "-o", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let compare: serde_json::Value = serde_json::from_slice(&compare_out).unwrap();

    // Ticket AC: every scored cell re-serializes byte-equal to the session's own
    // `lab score` output — the comparison presents, never recomputes.
    let run = &compare["sessions"][0];
    assert_eq!(run["session"], "s1");
    assert_eq!(run["n"], 1);
    for key in ["metrics", "trades", "verdict", "caveats"] {
        // A rename on both sides would compare null == null; guard the pin.
        assert!(!run[key].is_null(), "{key} missing from the comparison run");
        assert_eq!(
            serde_json::to_vec(&run[key]).unwrap(),
            serde_json::to_vec(&score[key]).unwrap(),
            "{key} must be byte-equal to the source scorecard's"
        );
    }
    assert_eq!(run["verdict"]["pass"], true);

    // Ticket AC: an unscoreable run is a named error column, never an
    // omission — and it never counts toward pass_count.
    let failed = &compare["sessions"][1];
    assert_eq!(failed["session"], "s2");
    assert!(
        failed["error"].as_str().is_some_and(
            |error| error.starts_with("validation:") && error.contains("no final summary")
        ),
        "got: {failed}"
    );
    assert!(failed.get("verdict").is_none());
    assert_eq!(compare["pass_count"], 1);
    assert_eq!(compare["total"], 2);
}

/// Freeze `name` with one criterion the r1 fixture passes (one fill ≥ 1).
fn freeze(home: &tempfile::TempDir, name: &str) {
    kraken_in(home)
        .args([
            "lab",
            "new",
            name,
            "--hypothesis",
            "fixture runs are comparable",
            "--min-fills",
            "1",
            "-o",
            "json",
        ])
        .assert()
        .success();
}

/// A fixture run on disk: `sessions/s<n>/session.json` stamped with the
/// experiment, a finalized run tape, and window markers on the ONE shared
/// scope journal — `stopped` controls whether the window closed (and a stop
/// summary anchors the score). A stopped session carries one in-window fill; an
/// open run carries only its start marker.
fn write_run(base: &Path, experiment: &str, ordinal: u32, stopped: bool) {
    let session_id = format!("s{ordinal}");
    let mut manifest = fixtures::manifest(vec![kraken_session::manifest::RecordingRef {
        file: "tape.jsonl".to_string(),
        ..fixtures::jsonl_recording()
    }]);
    manifest.id = session_id.clone();
    manifest.experiment = Some(experiment.to_string());
    if stopped {
        // 10_000 − 1.0 cost − 0.0026 fee + 1 BTC marked at 1.0.
        manifest.summary = Some(SessionOutcome {
            ended_at: "2026-01-01T00:30:00Z".parse().unwrap(),
            final_value: dec!(9_999.9974),
            pnl: dec!(-0.0026),
        });
    }
    let mut run = manifest;
    run.label = Some(format!("{experiment}-{session_id}"));
    run.window = SessionWindow {
        session: session_id.clone(),
        started_at: "2026-01-01T00:00:00.500Z".parse().unwrap(),
        opening_equity: dec!(10_000),
        opening_complete: true,
        ended_at: stopped.then(|| "2026-01-01T00:30:00Z".parse().unwrap()),
    };
    run.save(&session_file_path(base, ordinal)).unwrap();
    fixtures::write_run_tape(
        &session_dir(base, ordinal),
        &["2026-01-01T00:00:01Z", "2026-01-01T00:00:03Z"],
    );

    // The shared journal: seed the account epoch once, then this session's
    // window — markers cut it, the fill sits inside it.
    let journal = base.join("journal.jsonl");
    let mut records = Vec::new();
    if !journal.exists() {
        records.push(fixtures::account_record("2026-01-01T00:00:00Z"));
    }
    records.push(marker_record(
        "session start",
        &session_id,
        "2026-01-01T00:00:00.500Z",
    ));
    if stopped {
        records.push(fixtures::fill_record("2026-01-01T00:00:02Z"));
        records.push(marker_record(
            "session stop",
            &session_id,
            "2026-01-01T00:30:00Z",
        ));
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

/// The journal marker a `run start`/`run stop` appends.
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
fn lab_next_without_a_plan_names_the_run_flag() {
    let home = tempfile::tempdir().unwrap();
    freeze(&home, "planless");
    kraken_in(&home)
        .args(["lab", "next", "planless", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("--session"));
}

#[test]
fn lab_next_walks_a_live_plan_from_start_to_complete() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "lab",
            "new",
            "walk",
            "--hypothesis",
            "plan walking is mechanical",
            "--min-return-pct=-100",
            "--session",
            "live:1h",
            "-o",
            "json",
        ])
        .assert()
        .success();

    let out = kraken_in(&home)
        .args(["lab", "next", "walk", "-o", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["state"], "start");
    assert_eq!(json["session"], "walk-s1");
    let command = json["command"].as_str().unwrap();
    assert!(
        command.contains("kraken session start --label walk-s1"),
        "{command}"
    );
    assert!(command.contains("--experiment walk"), "{command}");
    assert!(command.contains("--for '1h'"), "{command}");

    // A completed run consumes the entry: fabricate a finalized r1.
    write_run(&config_dir_in(&home), "walk", 1, true);
    let out = kraken_in(&home)
        .args(["lab", "next", "walk", "-o", "json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["state"], "plan_complete");
    assert!(
        json["reason"]
            .as_str()
            .unwrap()
            .contains("lab compare walk"),
        "{}",
        json["reason"]
    );
}

#[test]
fn lab_new_refuses_to_seal_an_unfinalized_tape() {
    let home = tempfile::tempdir().unwrap();
    super::tape_tests::write_tape(&home, "hot", false);
    kraken_in(&home)
        .args([
            "lab",
            "new",
            "sealed-1",
            "--hypothesis",
            "unfinalized tapes cannot be sealed",
            "--min-return-pct=-100",
            "--session",
            "replay:tape:hot",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("not finalized"));
}

#[test]
fn lab_next_refuses_a_re_recorded_sealed_tape() {
    let home = tempfile::tempdir().unwrap();
    super::tape_tests::write_tape(&home, "tape", true);
    // A sealed replay leg needs a non-empty tape, so give it one frame; the
    // re-record below then only has to change the bytes to drift the hash.
    let tape = config_dir_in(&home).join("tapes").join("tape.jsonl");
    std::fs::write(
        &tape,
        concat!(
            r#"{"channel":"ticker","type":"update","data":[{"symbol":"BTC/USD","bid":50000.0,"bid_qty":1.0,"ask":50000.0,"ask_qty":1.0,"last":50000.0,"volume":100.0,"vwap":50000.0,"low":50000.0,"high":50000.0,"change":0.0,"change_pct":0.0,"timestamp":"2026-01-01T00:00:00Z"}]}"#,
            "\n"
        ),
    )
    .unwrap();
    kraken_in(&home)
        .args([
            "lab",
            "new",
            "sealed-2",
            "--hypothesis",
            "a re-recorded tape must not satisfy the plan",
            "--min-return-pct=-100",
            "--session",
            "replay:tape:tape",
            "-o",
            "json",
        ])
        .assert()
        .success();

    let mut bytes = std::fs::read(&tape).unwrap();
    bytes.push(b'\n');
    std::fs::write(&tape, bytes).unwrap();

    let out = kraken_in(&home)
        .args(["lab", "next", "sealed-2", "-o", "json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("no longer matches the frozen plan"),
        "{stdout}"
    );
    // The refusal names both hashes cleanly — no collapsed line-wrap runs.
    assert!(stdout.contains(", current sha256:"), "{stdout}");
}
