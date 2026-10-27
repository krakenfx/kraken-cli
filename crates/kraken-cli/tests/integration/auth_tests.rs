use super::common::{config_dir_in, kraken_in, kraken_mcp_process};

const OWN_KEY: &str = r#"
[auth]
api_key = "manual-key"
api_secret = "bWFudWFsLXNlY3JldA=="
"#;

const LOGIN_CHOSEN: &str = "active = \"sign-in\"\n";

const FUTURE: &str = "2099-01-01T00:00:00Z";
const PAST: &str = "2020-01-01T00:00:00Z";

fn login_key(valid_until: &str) -> String {
    format!(
        r#"
[auth.sign-in]
api_key = "login-key"
api_secret = "bG9naW4tc2VjcmV0"
name = "kraken-cli-0000beef"
valid_until = "{valid_until}"
"#
    )
}

fn home_with_config(toml: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let dir = config_dir_in(&home);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), toml).unwrap();
    home
}

/// Set in a developer's shell, these would change which key a test sees.
const AMBIENT: [&str; 7] = [
    "KRAKEN_API_KEY",
    "KRAKEN_API_SECRET",
    "KRAKEN_OAUTH_CLIENT_ID",
    "KRAKEN_OAUTH_AUTHORIZE_URL",
    "KRAKEN_SPOT_URL",
    "KRAKEN_DANGER_ALLOW_ANY_URL_HOST",
    "KRAKEN_DANGER_ACCEPT_INVALID_CERTS",
];

fn kraken(home: &tempfile::TempDir) -> assert_cmd::Command {
    let mut cmd = kraken_in(home);
    for var in AMBIENT {
        cmd.env_remove(var);
    }
    cmd
}

fn show(cmd: &mut assert_cmd::Command) -> serde_json::Value {
    let output = cmd.args(["auth", "show", "-o", "json"]).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn spot_source(home: &tempfile::TempDir) -> serde_json::Value {
    show(&mut kraken(home))["active_spot_source"].clone()
}

fn error_category(output: &std::process::Output) -> serde_json::Value {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["error"].clone()
}

#[test]
fn without_a_choice_your_own_key_is_active() {
    let home = home_with_config(&format!("{OWN_KEY}{}", login_key(FUTURE)));
    let status = show(&mut kraken(&home));
    assert_eq!(status["active_spot_key"], "api-key");
    assert_eq!(status["active_spot_source"], "config file");
}

#[test]
fn a_chosen_login_key_is_used_over_your_own_key() {
    let home = home_with_config(&format!("{OWN_KEY}{LOGIN_CHOSEN}{}", login_key(FUTURE)));
    assert_eq!(spot_source(&home), "browser sign-in");
}

#[test]
fn an_env_key_overrides_the_active_key() {
    let home = home_with_config(&format!("{OWN_KEY}{LOGIN_CHOSEN}{}", login_key(FUTURE)));
    let mut cmd = kraken(&home);
    cmd.env("KRAKEN_API_KEY", "env-key")
        .env("KRAKEN_API_SECRET", "ZW52LXNlY3JldA==");
    let status = show(&mut cmd);
    assert_eq!(status["active_spot_key"], "sign-in");
    assert_eq!(status["active_spot_source"], "environment variable");
    assert_eq!(status["overridden"], true);
}

#[test]
fn a_flag_key_overrides_the_active_key() {
    let home = home_with_config(&format!("{OWN_KEY}{LOGIN_CHOSEN}{}", login_key(FUTURE)));
    let mut cmd = kraken(&home);
    cmd.args(["--api-key", "flag-key", "--api-secret", "ZmxhZy1zZWNyZXQ="]);
    let status = show(&mut cmd);
    assert_eq!(status["active_spot_key"], "sign-in");
    assert_eq!(status["active_spot_source"], "command-line flag");
    assert_eq!(status["overridden"], true);
}

#[test]
fn the_login_key_is_used_when_no_other_key_exists() {
    let home = home_with_config(&login_key(FUTURE));
    assert_eq!(spot_source(&home), "browser sign-in");
}

#[test]
fn an_expired_active_login_key_never_falls_back_to_your_own_key() {
    let home = home_with_config(&format!("{OWN_KEY}{LOGIN_CHOSEN}{}", login_key(PAST)));
    assert!(spot_source(&home).is_null());
}

#[test]
fn an_expired_active_login_key_says_how_to_recover() {
    let home = home_with_config(&format!("{OWN_KEY}{LOGIN_CHOSEN}{}", login_key(PAST)));
    let output = kraken(&home)
        .args(["auth", "test", "-o", "json"])
        .output()
        .unwrap();
    assert_eq!(error_category(&output), "auth");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["message"]
            .as_str()
            .unwrap()
            .contains("sign-in key expired"),
        "{envelope}"
    );
}

#[test]
fn a_command_that_needs_no_key_stays_quiet_about_an_expired_login_key() {
    let home = home_with_config(&format!("{OWN_KEY}{LOGIN_CHOSEN}{}", login_key(PAST)));
    let output = kraken(&home)
        .args(["auth", "show", "-o", "json"])
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&output.stderr).contains("expired"));
}

#[test]
fn auth_switch_changes_the_active_key() {
    let home = home_with_config(&format!("{OWN_KEY}{}", login_key(FUTURE)));
    kraken(&home)
        .args(["auth", "switch", "sign-in"])
        .assert()
        .success();
    assert_eq!(spot_source(&home), "browser sign-in");
}

#[test]
fn auth_switch_to_a_key_that_is_not_stored_is_refused() {
    let home = home_with_config(OWN_KEY);
    let output = kraken(&home)
        .args(["auth", "switch", "sign-in", "-o", "json"])
        .output()
        .unwrap();
    assert_eq!(error_category(&output), "validation");
    assert_eq!(spot_source(&home), "config file");
}

#[test]
fn auth_switch_without_a_key_or_a_terminal_is_refused() {
    let home = home_with_config(&format!("{OWN_KEY}{}", login_key(FUTURE)));
    let output = kraken(&home)
        .args(["auth", "switch", "-o", "json"])
        .output()
        .unwrap();
    assert_eq!(error_category(&output), "validation");
}

#[test]
fn auth_set_without_a_terminal_keeps_the_active_login_key() {
    let home = home_with_config(&login_key(FUTURE));
    kraken(&home)
        .args([
            "auth",
            "set",
            "--api-key",
            "manual-key",
            "--api-secret",
            "c2VjcmV0",
        ])
        .assert()
        .success();
    let status = show(&mut kraken(&home));
    assert_eq!(status["api_key"]["present"], true);
    assert_eq!(status["active_spot_source"], "browser sign-in");
}

#[test]
fn auth_set_says_when_an_env_key_overrides_the_active_key() {
    let home = tempfile::tempdir().unwrap();
    let output = kraken(&home)
        .env("KRAKEN_API_KEY", "env-key")
        .env("KRAKEN_API_SECRET", "ZW52LXNlY3JldA==")
        .args([
            "auth",
            "set",
            "--api-key",
            "manual-key",
            "--api-secret",
            "c2VjcmV0",
            "-o",
            "json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let saved: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(saved["overridden_by"].is_string(), "{saved}");
}

#[test]
fn auth_status_is_an_alias_of_auth_show() {
    let home = home_with_config(OWN_KEY);
    let output = kraken(&home)
        .args(["auth", "status", "-o", "json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn auth_reset_deletes_every_key_and_keeps_the_settings() {
    let home = home_with_config(&format!(
        "{OWN_KEY}{}\n[settings]\ndefault_pair = \"XBTUSD\"\n",
        login_key(FUTURE)
    ));
    kraken(&home).args(["auth", "reset"]).assert().success();
    let saved = std::fs::read_to_string(config_dir_in(&home).join("config.toml")).unwrap();
    assert!(!saved.contains("[auth"), "{saved}");
    assert!(saved.contains("default_pair"), "{saved}");
}

/// `auth login` with a client ID and `extra`. It must fail validation before
/// the browser opens; returns the envelope's message.
fn login_refused(extra: &[(&str, &str)], args: &[&str]) -> String {
    let home = tempfile::tempdir().unwrap();
    let mut cmd = kraken(&home);
    cmd.env("KRAKEN_OAUTH_CLIENT_ID", "testclient");
    for (name, value) in extra {
        cmd.env(name, value);
    }
    let output = cmd
        .args(["auth", "login", "--no-browser", "-o", "json"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("Sign in to Kraken"),
        "{output:?}"
    );
    assert_eq!(error_category(&output), "validation");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    envelope["message"].as_str().unwrap().to_owned()
}

/// Not zero: the window parser refuses that before the lifetime check runs.
#[test]
fn auth_login_refuses_a_lifetime_under_a_minute() {
    assert!(login_refused(&[], &["--valid-for", "30s"]).contains("between 1m"));
}

#[test]
fn auth_login_refuses_a_lifetime_over_90_days() {
    assert!(login_refused(&[], &["--valid-for", "91d"]).contains("90d"));
}

#[test]
fn auth_login_without_a_terminal_is_refused() {
    assert!(login_refused(&[], &[]).contains("needs a terminal"));
}

#[test]
fn a_production_sign_in_never_sends_its_code_off_kraken() {
    let message = login_refused(
        &[
            ("KRAKEN_DANGER_ALLOW_ANY_URL_HOST", "1"),
            ("KRAKEN_SPOT_URL", "https://api.example.com"),
        ],
        &[],
    );
    assert!(message.contains("api.kraken.com only"), "{message}");
}

#[test]
fn a_production_sign_in_needs_verified_tls() {
    let message = login_refused(&[("KRAKEN_DANGER_ACCEPT_INVALID_CERTS", "1")], &[]);
    assert!(message.contains("verified TLS"), "{message}");
}

#[test]
fn an_empty_secret_from_stdin_keeps_its_published_message() {
    let home = tempfile::tempdir().unwrap();
    let output = kraken(&home)
        .args([
            "auth",
            "set",
            "--api-key",
            "k",
            "--api-secret-stdin",
            "-o",
            "json",
        ])
        .write_stdin("\n")
        .output()
        .unwrap();
    assert_eq!(error_category(&output), "auth");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["message"]
            .as_str()
            .unwrap()
            .contains("Empty secret received from stdin")
    );
}

#[test]
fn a_broken_config_file_never_prints_its_secret() {
    let home = home_with_config("[auth]\napi_key = \"k\"\napi_secret = \"c2VjcmV0\nbroken\n");
    let output = kraken(&home)
        .args(["auth", "show", "-o", "json"])
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&output.stdout).contains("c2VjcmV0"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("c2VjcmV0"));
}

#[test]
fn auth_reset_repairs_a_malformed_auth_table() {
    let home = home_with_config("[auth]\napi_key = 5\n\n[settings]\noutput = \"table\"\n");
    // The table must fail to load, or the reset never takes its repair path.
    kraken(&home)
        .args(["auth", "show", "-o", "json"])
        .assert()
        .failure();
    kraken(&home)
        .args(["auth", "reset", "--yes"])
        .assert()
        .success();
    let saved = std::fs::read_to_string(config_dir_in(&home).join("config.toml")).unwrap();
    assert!(!saved.contains("[auth"), "{saved}");
    assert!(saved.contains("output"), "{saved}");
}

#[test]
fn a_bad_authorize_url_only_breaks_auth_login() {
    let home = home_with_config(OWN_KEY);
    let untrusted = "https://sign-in.example.com/oauth/authorize";
    kraken(&home)
        .env("KRAKEN_OAUTH_AUTHORIZE_URL", untrusted)
        .args(["auth", "show", "-o", "json"])
        .assert()
        .success();
    let output = kraken(&home)
        .env("KRAKEN_OAUTH_AUTHORIZE_URL", untrusted)
        .env("KRAKEN_OAUTH_CLIENT_ID", "testclient")
        .args(["auth", "login", "--no-browser", "-o", "json"])
        .output()
        .unwrap();
    // Without a terminal, `auth login` fails validation anyway; the message
    // shows that the allowlist refused it first.
    assert_eq!(error_category(&output), "validation");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let message = envelope["message"].as_str().unwrap();
    assert!(message.contains("Untrusted URL host"), "{message}");
}

/// Without a terminal the refusal comes after the sign-in config resolved, so
/// reaching it proves the client ID did; no sign-in URL is printed either way.
fn login_reaches_the_terminal_check(client_id: Option<&str>) {
    let home = tempfile::tempdir().unwrap();
    let mut cmd = kraken(&home);
    if let Some(id) = client_id {
        cmd.env("KRAKEN_OAUTH_CLIENT_ID", id);
    }
    let output = cmd
        .args(["auth", "login", "--no-browser", "-o", "json"])
        .output()
        .unwrap();
    assert_eq!(error_category(&output), "validation", "{output:?}");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let message = envelope["message"].as_str().unwrap();
    assert!(message.contains("needs a terminal"), "{message}");
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("Sign in to Kraken"),
        "{output:?}"
    );
}

#[test]
fn auth_login_signs_in_with_the_built_in_client_when_none_is_set() {
    login_reaches_the_terminal_check(None);
}

#[test]
fn an_empty_client_id_counts_as_unset() {
    login_reaches_the_terminal_check(Some(""));
}

#[test]
fn a_sign_in_page_override_needs_its_own_client_id() {
    let home = tempfile::tempdir().unwrap();
    let output = kraken(&home)
        .env("KRAKEN_DANGER_ALLOW_ANY_URL_HOST", "1")
        .env(
            "KRAKEN_OAUTH_AUTHORIZE_URL",
            "https://sign-in.example.com/oauth/authorize",
        )
        .args(["auth", "login", "--no-browser", "-o", "json"])
        .output()
        .unwrap();
    assert_eq!(error_category(&output), "config", "{output:?}");
}

/// One `tools/call` of `kraken_auth_show` on a running server.
fn mcp_auth_show(
    stdin: &mut std::process::ChildStdin,
    stdout: &mut impl std::io::BufRead,
    id: u32,
) -> serde_json::Value {
    use std::io::Write;
    let request = serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": {
            "name": "kraken_auth_show", "arguments": {},
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    });
    writeln!(stdin, "{request}").unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let response: serde_json::Value = serde_json::from_str(&line).unwrap();
    response["result"]["structuredContent"].clone()
}

#[test]
fn a_running_mcp_server_follows_a_switch_of_the_active_key() {
    let home = home_with_config(&format!("{OWN_KEY}{}", login_key(FUTURE)));
    let mut server = kraken_mcp_process(&home);
    for var in AMBIENT {
        server.env_remove(var);
    }
    let mut server = server
        .args(["-s", "auth"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = server.stdin.take().unwrap();
    let mut stdout = std::io::BufReader::new(server.stdout.take().unwrap());

    let before = mcp_auth_show(&mut stdin, &mut stdout, 1);
    kraken(&home)
        .args(["auth", "switch", "sign-in"])
        .assert()
        .success();
    let after = mcp_auth_show(&mut stdin, &mut stdout, 2);
    drop(stdin);
    server.wait().unwrap();

    assert_eq!(before["active_spot_source"], "config file", "{before}");
    assert_eq!(after["active_spot_source"], "browser sign-in", "{after}");
}
