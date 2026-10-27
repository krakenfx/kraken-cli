//! End-to-end CLI wiring tests for `kraken plugin init/list`.
//!
//! The scaffolds are exercised for real: the generated entrypoints run
//! against a stub `kraken` on PATH that answers with canned market JSON, so
//! the "runs unmodified and prints JSON" contract is pinned offline.

use super::common::{config_dir_in, kraken_in};
use predicates::prelude::*;
use std::path::Path;

/// A fake `kraken` ahead of PATH: any invocation prints the canned payload,
/// so a generated entrypoint's shell-out succeeds without the network.
fn stub_kraken(dir: &Path, payload: &str) {
    let stub = dir.join("kraken");
    std::fs::write(&stub, format!("#!/bin/sh\ncat <<'EOF'\n{payload}\nEOF\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

// The stubs answer with the CLI's POST-SCHEMA payloads — the named-field
// shapes `-o json` actually emits (pinned by src/commands/schema.rs tests) —
// never Kraken's raw wire arrays. A stub carrying the wire shape would pass
// a template the real binary breaks.
const OHLC_PAYLOAD: &str = r#"{"pair":"BTC/USD","candles":[{"time":1700000000,"open":100.0,"high":110.0,"low":90.0,"close":105.0,"vwap":104.0,"volume":12.5,"count":42},{"time":1700000300,"open":105.0,"high":112.0,"low":101.0,"close":108.0,"vwap":107.0,"volume":9.1,"count":30}],"last":1700000300}"#;
const TICKER_PAYLOAD: &str = r#"{"XXBTZUSD":{"ask_price":30300.1,"bid_price":30300.0,"last_price":30303.2,"last_volume":0.00067643,"open":30502.8}}"#;
const TRADES_PAYLOAD: &str = r#"{"pair":"BTC/USD","trades":[{"price":30243.4,"volume":0.34499092,"time":"1688669597.8277","side":"buy","order_type":"m","misc":"","trade_id":61044952}],"last":"1688671969993150842"}"#;

#[test]
fn plugin_init_scaffolds_the_four_artifacts() {
    let home = tempfile::tempdir().unwrap();
    let out = kraken_in(&home)
        .args([
            "plugin",
            "init",
            "volume-profile",
            "--lang",
            "python",
            "-o",
            "json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["extension"]["name"], "volume-profile");
    assert_eq!(json["extension"]["kind"], "indicator");
    assert_eq!(json["extension"]["language"], "python");
    assert_eq!(json["extension"]["entrypoint"], "main.py");
    assert!(
        json["install"]
            .as_str()
            .unwrap()
            .contains("~/.openclaw/skills/"),
        "{json}"
    );

    let dir = config_dir_in(&home).join("plugins").join("volume-profile");
    for file in ["extension.json", "main.py", "SKILL.md", "README.md"] {
        assert!(dir.join(file).exists(), "missing {file}");
    }
    assert!(
        !dir.join("plugin.json").exists(),
        "the manifest must not collide with the agent-host plugin.json"
    );
    let skill = std::fs::read_to_string(dir.join("SKILL.md")).unwrap();
    assert!(skill.contains("name: volume-profile"), "{skill}");
    assert!(skill.contains("bins: [\"kraken\"]"), "{skill}");
}

#[test]
fn plugin_init_refuses_unsafe_names_and_taken_directories() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args([
            "plugin",
            "init",
            "../escape",
            "--lang",
            "python",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("not a safe directory name"));

    kraken_in(&home)
        .args(["plugin", "init", "taken", "--lang", "bash", "-o", "json"])
        .assert()
        .success();
    kraken_in(&home)
        .args(["plugin", "init", "taken", "--lang", "python", "-o", "json"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"error\":\"validation\""))
        .stdout(predicate::str::contains("already holds files"));
}

#[test]
fn plugin_list_reports_name_kind_lang_and_entrypoint() {
    let home = tempfile::tempdir().unwrap();
    let empty = kraken_in(&home)
        .args(["plugin", "list", "-o", "json"])
        .output()
        .unwrap();
    assert!(empty.status.success(), "{empty:?}");
    let json: serde_json::Value = serde_json::from_slice(&empty.stdout).unwrap();
    assert_eq!(json["plugins"], serde_json::json!([]));

    kraken_in(&home)
        .args([
            "plugin", "init", "levels", "--lang", "bash", "--kind", "alert", "-o", "json",
        ])
        .assert()
        .success();
    kraken_in(&home)
        .args([
            "plugin", "init", "tape-csv", "--lang", "python", "--kind", "exporter",
        ])
        .assert()
        .success();
    // A hollow directory is a described row, never hidden.
    std::fs::create_dir_all(config_dir_in(&home).join("plugins").join("hollow")).unwrap();

    let out = kraken_in(&home)
        .args(["plugin", "list", "-o", "json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let plugins = json["plugins"].as_array().unwrap();
    assert_eq!(plugins.len(), 3, "{json}");
    assert_eq!(plugins[0]["name"], "hollow");
    assert!(
        plugins[0]["damaged"]
            .as_str()
            .unwrap()
            .contains("extension.json"),
        "{json}"
    );
    assert_eq!(plugins[1]["name"], "levels");
    assert_eq!(plugins[1]["kind"], "alert");
    assert_eq!(plugins[1]["language"], "bash");
    assert_eq!(plugins[1]["entrypoint"], "main.sh");
    assert_eq!(plugins[2]["name"], "tape-csv");
    assert_eq!(plugins[2]["kind"], "exporter");
}

/// Scaffolds `name` with the given lang/kind, then runs its entrypoint
/// against a stub `kraken` answering `payload`, returning captured stdout.
fn scaffold_and_run(
    home: &tempfile::TempDir,
    name: &str,
    lang: &str,
    kind: &str,
    payload: &str,
    extra_args: &[&str],
) -> Vec<u8> {
    kraken_in(home)
        .args(["plugin", "init", name, "--lang", lang, "--kind", kind])
        .assert()
        .success();
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    stub_kraken(&bin, payload);

    let entrypoint = config_dir_in(home)
        .join("plugins")
        .join(name)
        .join(if lang == "python" {
            "main.py"
        } else {
            "main.sh"
        });
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let runner = if lang == "python" { "python3" } else { "bash" };
    let out = std::process::Command::new(runner)
        .arg(&entrypoint)
        .arg("BTC/USD")
        .args(extra_args)
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{name}: {out:?}");
    out.stdout
}

/// The generated Python entrypoints run unmodified and print — pinned
/// against a stub `kraken` answering the CLI's post-schema payloads, so
/// every kind's body is exercised for real, offline.
#[test]
fn generated_python_entrypoint_runs_and_prints_json() {
    let home = tempfile::tempdir().unwrap();
    let stdout = scaffold_and_run(&home, "sma", "python", "indicator", OHLC_PAYLOAD, &[]);
    let json: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(json["pair"], "BTC/USD");
    assert_eq!(json["candles"], 2);
    assert_eq!(json["sma20"], 106.5);
}

#[test]
fn generated_python_alert_reads_the_named_ticker_fields() {
    let home = tempfile::tempdir().unwrap();
    let stdout = scaffold_and_run(
        &home,
        "levels",
        "python",
        "alert",
        TICKER_PAYLOAD,
        &["30000"],
    );
    let json: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(json["last"], 30303.2);
    assert_eq!(json["above_threshold"], true);
}

#[test]
fn generated_python_exporter_writes_trade_values_not_key_names() {
    let home = tempfile::tempdir().unwrap();
    let stdout = scaffold_and_run(&home, "tape-csv", "python", "exporter", TRADES_PAYLOAD, &[]);
    let csv = String::from_utf8(stdout).unwrap();
    let mut lines = csv.lines();
    assert_eq!(lines.next(), Some("price,volume,time,side"));
    assert_eq!(
        lines.next(),
        Some("30243.4,0.34499092,1688669597.8277,buy"),
        "{csv}"
    );
}

#[test]
fn generated_python_driver_emits_a_signal_never_an_order() {
    let home = tempfile::tempdir().unwrap();
    let stdout = scaffold_and_run(&home, "drv", "python", "driver", TICKER_PAYLOAD, &[]);
    let json: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(json["last"], 30303.2);
    assert_eq!(json["signal"], "hold");
}

/// The bash variant is a JSON passthrough over the same PATH contract.
#[test]
fn generated_bash_entrypoint_runs_and_prints_json() {
    let home = tempfile::tempdir().unwrap();
    let stdout = scaffold_and_run(&home, "raw", "bash", "indicator", OHLC_PAYLOAD, &[]);
    let json: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(json["pair"], "BTC/USD");
    assert!(json["candles"].is_array(), "{json}");
}

/// `--path` writes elsewhere and the library stays untouched.
#[test]
fn plugin_init_honors_a_path_override() {
    let home = tempfile::tempdir().unwrap();
    let target = home.path().join("elsewhere");
    kraken_in(&home)
        .args([
            "plugin",
            "init",
            "aside",
            "--lang",
            "bash",
            "--path",
            target.to_str().unwrap(),
            "-o",
            "json",
        ])
        .assert()
        .success();
    assert!(target.join("extension.json").exists());
    assert!(!config_dir_in(&home).join("plugins").join("aside").exists());
}
