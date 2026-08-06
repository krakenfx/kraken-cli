use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use uuid::Uuid;

pub(crate) const CLIENT_NAME: &str = "kraken-cli";
pub(crate) const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const BASE_USER_AGENT: &str = concat!("kraken-cli/", env!("CARGO_PKG_VERSION"));

pub(crate) const KORIGIN_REST: &str = "u003";
pub(crate) const KORIGIN_WS: &str = "u004";

const AGENT_CLIENT_ENV: &str = "KRAKEN_AGENT_CLIENT";
const INSTANCE_ID_ENV: &str = "KRAKEN_INSTANCE_ID";
const INSTANCE_ID_FILE: &str = "instance_id";

static AGENT_CLIENT: OnceLock<String> = OnceLock::new();
static INSTANCE_ID: OnceLock<String> = OnceLock::new();
static USER_AGENT: OnceLock<String> = OnceLock::new();

/// The shared client-identity headers, tagged with the transport's `x-korigin`. The one
/// list both transports build from, so adding an identity header cannot drift between
/// them; the WS handshake also carries an explicit `User-Agent` (its transports take
/// headers as data, while REST sets it on the client builder).
fn identity_headers(korigin: &'static str) -> [(&'static str, &'static str); 5] {
    [
        ("X-Kraken-Client", CLIENT_NAME),
        ("X-Kraken-Client-Version", CLIENT_VERSION),
        ("X-Kraken-Agent-Client", agent_client()),
        ("X-Kraken-Instance-Id", instance_id()),
        ("x-korigin", korigin),
    ]
}

/// Every WS handshake's client-identity headers — see [`identity_headers`].
pub(crate) fn ws_handshake_headers() -> Vec<(&'static str, &'static str)> {
    std::iter::once(("User-Agent", user_agent()))
        .chain(identity_headers(KORIGIN_WS))
        .collect()
}

/// The REST clients' default client-identity headers — see [`identity_headers`].
pub(crate) fn rest_identity_headers() -> [(&'static str, &'static str); 5] {
    identity_headers(KORIGIN_REST)
}

pub(crate) fn agent_client() -> &'static str {
    AGENT_CLIENT.get_or_init(resolve_agent_client).as_str()
}

pub(crate) fn instance_id() -> &'static str {
    INSTANCE_ID.get_or_init(resolve_instance_id).as_str()
}

/// Generate a fresh UUID v4 (used for feedback event ids and similar).
pub(crate) fn new_uuid_v4() -> String {
    Uuid::new_v4().to_string()
}

/// Structured User-Agent: `kraken-cli/0.2.0 (cursor)`.
/// Includes the base product token and the resolved agent client in parentheses,
/// following RFC 9110 product/comment convention.
pub(crate) fn user_agent() -> &'static str {
    USER_AGENT
        .get_or_init(|| build_structured_user_agent(agent_client()))
        .as_str()
}

fn build_structured_user_agent(agent: &str) -> String {
    format!("{BASE_USER_AGENT} ({agent})")
}

fn resolve_agent_client() -> String {
    if let Ok(raw) = std::env::var(AGENT_CLIENT_ENV) {
        return normalize_agent_client(&raw).to_string();
    }
    detect_from_environment()
}

fn normalize_agent_client(raw: &str) -> &'static str {
    let normalized = raw.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "cursor" | "cursor-ide" | "cursor-agent" => "cursor",
        "claude" | "claude-code" | "claude_code" | "claudecode" => "claude",
        "openclaw" | "open-claw" => "openclaw",
        "codex" | "openai-codex" => "codex",
        "goose" | "block-goose" => "goose",
        "gemini" | "gemini-cli" => "gemini",
        _ => "other",
    }
}

/// Detects the calling agent from well-known environment variables that
/// agent runtimes inject into their child processes.
///
/// Priority: agent-specific markers, then the vendor-neutral AGENT var
/// (see <https://github.com/agentsmd/agents.md/issues/136>), then IDE markers.
fn detect_from_environment() -> String {
    if env_present("CURSOR_AGENT") || env_present("CURSOR_TRACE_ID") {
        return "cursor".into();
    }
    if env_present("CLAUDECODE") {
        return "claude".into();
    }
    if env_present("OPENCLAW_SHELL") {
        return "openclaw".into();
    }
    if env_present("CODEX_SANDBOX") {
        return "codex".into();
    }
    if env_present("GOOSE_TERMINAL") {
        return "goose".into();
    }
    if env_present("GEMINI_CLI") {
        return "gemini".into();
    }

    if let Some(agent) = read_agent_env_var() {
        return agent;
    }

    if env_present("VSCODE_PID") || env_present("VSCODE_CLI") {
        return "vscode".into();
    }
    "direct".into()
}

/// Reads the vendor-neutral `AGENT` env var proposed by the agents.md
/// standardization effort. Sanitizes the value to ASCII alphanumeric
/// plus hyphens, lowercased, max 32 chars. Known agent names are
/// normalized to their canonical form via `normalize_agent_client`.
fn read_agent_env_var() -> Option<String> {
    let raw = std::env::var("AGENT").ok()?;
    let sanitized: String = raw
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(32)
        .collect::<String>()
        .to_ascii_lowercase();
    if sanitized.is_empty() {
        return None;
    }
    let normalized = normalize_agent_client(&sanitized);
    if normalized != "other" {
        Some(normalized.to_string())
    } else {
        Some(sanitized)
    }
}

fn env_present(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

fn resolve_instance_id() -> String {
    if let Ok(value) = std::env::var(INSTANCE_ID_ENV) {
        let trimmed = value.trim();
        if is_uuid_like(trimmed) {
            return trimmed.to_string();
        }
    }

    if let Some(path) = instance_id_path() {
        if let Some(existing) = read_instance_id(&path) {
            return existing;
        }

        let generated = Uuid::new_v4().to_string();
        let _ = write_instance_id(&path, &generated);
        return generated;
    }

    Uuid::new_v4().to_string()
}

fn instance_id_path() -> Option<PathBuf> {
    crate::config::config_dir()
        .ok()
        .map(|dir| dir.join(INSTANCE_ID_FILE))
}

fn read_instance_id(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let trimmed = content.trim();
    if is_uuid_like(trimmed) {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn write_instance_id(path: &Path, value: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    atomic_write_restricted(path, value.as_bytes())?;
    Ok(())
}

fn is_uuid_like(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (idx, byte) in bytes.iter().enumerate() {
        match idx {
            8 | 13 | 18 | 23 => {
                if *byte != b'-' {
                    return false;
                }
            }
            _ => {
                if !byte.is_ascii_hexdigit() {
                    return false;
                }
            }
        }
    }
    true
}

#[cfg(unix)]
fn atomic_write_restricted(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let dir = path.parent().unwrap_or(path);
    let tmp_path = dir.join(".instance_id.tmp");

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp_path)?;
    file.write_all(data)?;
    file.sync_all()?;

    fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(not(unix))]
fn atomic_write_restricted(path: &Path, data: &[u8]) -> std::io::Result<()> {
    fs::write(path, data)?;
    Ok(())
}

// The only unsafe in the crate: Rust 2024 env mutation, justified on ENV_LOCK.
#[allow(unsafe_code)]
#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// SAFETY justification for every `unsafe` env access below: mutating the
    /// process environment is only sound while no other thread touches it, and
    /// all env mutation in this test binary lives in this module, serialized
    /// behind this lock; the reads under test run on the same thread while it
    /// is held.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const ALL_AGENT_VARS: &[&str] = &[
        "CURSOR_AGENT",
        "CURSOR_TRACE_ID",
        "CLAUDECODE",
        "OPENCLAW_SHELL",
        "CODEX_SANDBOX",
        "GOOSE_TERMINAL",
        "GEMINI_CLI",
        "AGENT",
        "VSCODE_PID",
        "VSCODE_CLI",
    ];

    fn clear_agent_vars() {
        for var in ALL_AGENT_VARS {
            unsafe { std::env::remove_var(var) };
        }
    }

    /// Pins the generator/validator agreement: whatever we mint must pass the
    /// gate that guards env- and file-sourced ids.
    #[test]
    fn generated_instance_id_is_uuid() {
        let generated = Uuid::new_v4().to_string();
        assert!(is_uuid_like(&generated));
    }

    #[test]
    fn normalize_agent_client_values() {
        assert_eq!(normalize_agent_client("cursor"), "cursor");
        assert_eq!(normalize_agent_client("cursor-ide"), "cursor");
        assert_eq!(normalize_agent_client("cursor-agent"), "cursor");
        assert_eq!(normalize_agent_client("claude"), "claude");
        assert_eq!(normalize_agent_client("claude-code"), "claude");
        assert_eq!(normalize_agent_client("claude_code"), "claude");
        assert_eq!(normalize_agent_client("claudecode"), "claude");
        assert_eq!(normalize_agent_client("openclaw"), "openclaw");
        assert_eq!(normalize_agent_client("open-claw"), "openclaw");
        assert_eq!(normalize_agent_client("codex"), "codex");
        assert_eq!(normalize_agent_client("openai-codex"), "codex");
        assert_eq!(normalize_agent_client("goose"), "goose");
        assert_eq!(normalize_agent_client("block-goose"), "goose");
        assert_eq!(normalize_agent_client("gemini"), "gemini");
        assert_eq!(normalize_agent_client("gemini-cli"), "gemini");
        assert_eq!(normalize_agent_client("unknown"), "other");
        assert_eq!(normalize_agent_client("  Cursor  "), "cursor");
    }

    #[test]
    fn detect_cursor_from_cursor_agent_var() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("CURSOR_AGENT", "1") };
        assert_eq!(detect_from_environment(), "cursor");
        clear_agent_vars();
    }

    #[test]
    fn detect_cursor_from_trace_id_var() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("CURSOR_TRACE_ID", "abc123") };
        assert_eq!(detect_from_environment(), "cursor");
        clear_agent_vars();
    }

    #[test]
    fn detect_claude_from_claudecode_var() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("CLAUDECODE", "1") };
        assert_eq!(detect_from_environment(), "claude");
        clear_agent_vars();
    }

    #[test]
    fn detect_openclaw_from_shell_var() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("OPENCLAW_SHELL", "exec") };
        assert_eq!(detect_from_environment(), "openclaw");
        clear_agent_vars();
    }

    #[test]
    fn detect_codex_from_sandbox_var() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("CODEX_SANDBOX", "seatbelt") };
        assert_eq!(detect_from_environment(), "codex");
        clear_agent_vars();
    }

    #[test]
    fn detect_goose_from_terminal_var() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("GOOSE_TERMINAL", "1") };
        assert_eq!(detect_from_environment(), "goose");
        clear_agent_vars();
    }

    #[test]
    fn detect_gemini_from_cli_var() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("GEMINI_CLI", "1") };
        assert_eq!(detect_from_environment(), "gemini");
        clear_agent_vars();
    }

    #[test]
    fn agent_env_var_known_value_normalized() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("AGENT", "claude-code") };
        assert_eq!(detect_from_environment(), "claude");
        clear_agent_vars();
    }

    #[test]
    fn agent_env_var_unknown_value_passed_through() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("AGENT", "some-new-agent") };
        assert_eq!(detect_from_environment(), "some-new-agent");
        clear_agent_vars();
    }

    #[test]
    fn agent_env_var_sanitized_and_lowercased() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("AGENT", "  My-Custom-Agent!@#  ") };
        assert_eq!(detect_from_environment(), "my-custom-agent");
        clear_agent_vars();
    }

    #[test]
    fn agent_env_var_truncated_to_32_chars() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("AGENT", "a]bcdefghijklmnopqrstuvwxyz-123456789") };
        let result = detect_from_environment();
        assert!(result.len() <= 32);
        assert_eq!(result, "abcdefghijklmnopqrstuvwxyz-12345");
        clear_agent_vars();
    }

    #[test]
    fn empty_agent_env_var_falls_through() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("AGENT", "") };
        assert_eq!(detect_from_environment(), "direct");
        clear_agent_vars();
    }

    #[test]
    fn detect_vscode_without_agent_vars() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("VSCODE_PID", "12345") };
        assert_eq!(detect_from_environment(), "vscode");
        clear_agent_vars();
    }

    #[test]
    fn cursor_takes_priority_over_vscode() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("CURSOR_AGENT", "1") };
        unsafe { std::env::set_var("VSCODE_PID", "12345") };
        assert_eq!(detect_from_environment(), "cursor");
        clear_agent_vars();
    }

    #[test]
    fn agent_specific_var_takes_priority_over_agent_env() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("CLAUDECODE", "1") };
        unsafe { std::env::set_var("AGENT", "some-other-agent") };
        assert_eq!(detect_from_environment(), "claude");
        clear_agent_vars();
    }

    #[test]
    fn agent_env_takes_priority_over_vscode() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("AGENT", "aider") };
        unsafe { std::env::set_var("VSCODE_PID", "12345") };
        assert_eq!(detect_from_environment(), "aider");
        clear_agent_vars();
    }

    #[test]
    fn detect_direct_when_no_signals() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        assert_eq!(detect_from_environment(), "direct");
    }

    #[test]
    fn empty_env_var_is_not_present() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_agent_vars();
        unsafe { std::env::set_var("CURSOR_AGENT", "") };
        assert_eq!(detect_from_environment(), "direct");
        clear_agent_vars();
    }

    #[test]
    fn structured_user_agent_includes_agent_in_parens() {
        let ua = build_structured_user_agent("cursor");
        assert!(ua.ends_with(" (cursor)"));
    }

    #[test]
    fn structured_user_agent_starts_with_base_product_token() {
        let ua = build_structured_user_agent("direct");
        assert!(ua.starts_with("kraken-cli/"));
    }

    #[test]
    fn structured_user_agent_matches_expected_format() {
        let ua = build_structured_user_agent("gemini");
        let expected = format!("kraken-cli/{} (gemini)", env!("CARGO_PKG_VERSION"));
        assert_eq!(ua, expected);
    }

    #[test]
    fn structured_user_agent_direct_caller() {
        let ua = build_structured_user_agent("direct");
        let expected = format!("kraken-cli/{} (direct)", env!("CARGO_PKG_VERSION"));
        assert_eq!(ua, expected);
    }

    #[test]
    fn structured_user_agent_is_valid_http_header() {
        use reqwest::header::HeaderValue;
        for agent in &[
            "cursor", "claude", "codex", "goose", "gemini", "direct", "vscode", "other",
        ] {
            let ua = build_structured_user_agent(agent);
            assert!(
                HeaderValue::from_str(&ua).is_ok(),
                "invalid header for agent '{agent}': {ua}"
            );
        }
    }

    #[test]
    fn structured_user_agent_passthrough_agent_is_valid_http_header() {
        use reqwest::header::HeaderValue;
        let ua = build_structured_user_agent("some-new-agent");
        assert!(HeaderValue::from_str(&ua).is_ok());
        assert!(ua.ends_with(" (some-new-agent)"));
    }
}
