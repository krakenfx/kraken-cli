/// Configuration management for `~/.config/kraken/config.toml`.
///
/// Handles loading, saving, and resetting configuration with secure file
/// permissions (0600). Implements credential precedence: flag > env > config.
use std::fs;
use std::path::{Path, PathBuf};

use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::errors::{KrakenError, Result};

/// On-disk configuration format.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct KrakenConfig {
    #[serde(default)]
    pub(crate) auth: AuthConfig,
    #[serde(default)]
    pub(crate) settings: SettingsConfig,
}

/// Authentication section of the config file.
///
/// Every field is a [`SecretString`], so `Debug` redacts automatically and the
/// values are zeroized on drop. `secrecy` provides `Deserialize` out of the box
/// but deliberately omits `Serialize`; persisting to the (0600-mode) config
/// file is the one place we must expose them, via [`serialize_secret`].
#[skip_serializing_none]
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct AuthConfig {
    #[serde(serialize_with = "serialize_secret")]
    pub(crate) api_key: Option<SecretString>,
    #[serde(serialize_with = "serialize_secret")]
    pub(crate) api_secret: Option<SecretString>,
    #[serde(serialize_with = "serialize_secret")]
    pub(crate) futures_api_key: Option<SecretString>,
    #[serde(serialize_with = "serialize_secret")]
    pub(crate) futures_api_secret: Option<SecretString>,
}

/// Serialize an `Option<SecretString>` config field by exposing the secret.
///
/// `secrecy` intentionally does not implement `Serialize` for secrets to
/// prevent accidental exfiltration; the secrecy docs direct callers that must
/// persist a secret to opt in with `serialize_with`. This is invoked only when
/// the field is `Some` (the `None` case is skipped by `skip_serializing_if`).
fn serialize_secret<S>(
    secret: &Option<SecretString>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use secrecy::ExposeSecret;
    match secret {
        Some(s) => serializer.serialize_some(s.expose_secret()),
        None => serializer.serialize_none(),
    }
}

/// General settings section.
#[skip_serializing_none]
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct SettingsConfig {
    pub(crate) default_pair: Option<String>,
    pub(crate) output: Option<String>,
}

/// Resolved API credentials for one engine (Spot or Futures).
///
/// Spot and Futures share the same shape — a key/secret pair plus where they
/// were resolved from — and both feed `&SecretString` into HMAC signing, so a
/// single type serves both. Which engine a value belongs to is a property of
/// where it is stored ([`crate::cli::AppContext`]), not of the type. `api_key`
/// and `api_secret` are [`SecretString`]s, so the derived `Debug` redacts them,
/// there is no `Display`, and both are zeroized on drop.
#[derive(Debug, Clone)]
pub struct Credentials {
    pub api_key: SecretString,
    pub api_secret: SecretString,
    pub source: CredentialSource,
}

impl Credentials {
    /// Wrap a plaintext key/secret pair (from a flag or env var) as secrets.
    fn from_plaintext(api_key: String, api_secret: String, source: CredentialSource) -> Self {
        Self {
            api_key: api_key.into(),
            api_secret: api_secret.into(),
            source,
        }
    }
}

/// Which trading engine a set of credentials targets. Selects the env-var names,
/// config-file fields, and warning wording the otherwise-identical resolution
/// logic uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Spot,
    Futures,
}

impl Engine {
    /// Human-facing name used in "not configured" errors and warnings.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Engine::Spot => "Spot",
            Engine::Futures => "Futures",
        }
    }

    /// `(key, secret)` environment-variable names for this engine. clap performs
    /// the actual reads; these names are reused here only for warning text.
    pub(crate) fn env_names(self) -> (&'static str, &'static str) {
        match self {
            Engine::Spot => ("KRAKEN_API_KEY", "KRAKEN_API_SECRET"),
            Engine::Futures => ("KRAKEN_FUTURES_API_KEY", "KRAKEN_FUTURES_API_SECRET"),
        }
    }

    /// Pull this engine's `(key, secret)` config-file fields out of `AuthConfig`.
    fn config_pair(self, auth: AuthConfig) -> (Option<SecretString>, Option<SecretString>) {
        match self {
            Engine::Spot => (auth.api_key, auth.api_secret),
            Engine::Futures => (auth.futures_api_key, auth.futures_api_secret),
        }
    }
}

/// Where the credentials were resolved from.
#[derive(Debug, Clone, Copy)]
pub enum CredentialSource {
    Flag,
    Env,
    Config,
}

impl std::fmt::Display for CredentialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Flag => write!(f, "command-line flag"),
            Self::Env => write!(f, "environment variable"),
            Self::Config => write!(f, "config file"),
        }
    }
}

/// Returns the config directory path: `~/.config/kraken/`.
pub(crate) fn config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir()
        .ok_or_else(|| KrakenError::Config("Cannot determine config directory".into()))?;
    Ok(base.join("kraken"))
}

pub(crate) fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

/// Load configuration from disk. Returns default if file does not exist.
pub(crate) fn load() -> Result<KrakenConfig> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(KrakenConfig::default());
    }
    let contents = fs::read_to_string(&path)?;
    let cfg: KrakenConfig = toml::from_str(&contents)?;
    Ok(cfg)
}

/// Save configuration to disk atomically with 0600 permissions.
///
/// On Unix the file is written to a temporary path with mode 0600 from
/// creation, then renamed into place. This eliminates the window where
/// credentials could be read by other local users.
pub(crate) fn save(cfg: &KrakenConfig) -> Result<()> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir)?;
    let path = dir.join("config.toml");
    let contents = toml::to_string_pretty(cfg)
        .map_err(|e| KrakenError::Config(format!("TOML serialize error: {e}")))?;
    atomic_write_restricted(&path, contents.as_bytes())?;
    Ok(())
}

/// Treat an empty string as absent (`None`).
///
/// clap owns the credential environment reads (see [`crate::cli`]); an unset var
/// arrives as `None`, but plugin hosts such as Claude Code pass an *empty
/// string* when a `userConfig` field is left blank. Without this, resolution
/// would treat `""` as a real (but invalid) credential instead of falling
/// through to the next tier.
fn normalize_env(value: Option<&str>) -> Option<&str> {
    value.filter(|s| !s.is_empty())
}

/// Env var name that allows non-Kraken hosts in URL overrides, surfaced in the
/// untrusted-host error message. The value is read by clap (see [`crate::cli`]);
/// this constant only names it for user guidance.
pub(crate) const DANGER_ALLOW_ANY_URL_HOST_ENV: &str = "KRAKEN_DANGER_ALLOW_ANY_URL_HOST";

/// Whether the [`NO_COLOR`](https://no-color.org/) convention is active: the
/// variable is present and non-empty. Read here so the logging layer
/// ([`crate::logging`]) never touches the environment directly.
pub(crate) fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

/// Env var that overrides the default `kraken feedback` ingest URL.
pub(crate) const FEEDBACK_ENDPOINT_ENV: &str = "KRAKEN_FEEDBACK_ENDPOINT";

/// Optional override for the feedback ingest endpoint. Empty / unset → `None`.
pub(crate) fn feedback_endpoint_override() -> Option<String> {
    std::env::var(FEEDBACK_ENDPOINT_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Clear stored credentials while preserving the `[settings]` section.
pub(crate) fn reset_auth() -> Result<()> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(());
    }
    let mut cfg = load()?;
    cfg.auth = AuthConfig::default();
    save(&cfg)?;
    Ok(())
}

/// Take a complete `(key, secret)` pair from a single resolution tier.
///
/// Both halves must be present to use a tier. A half-configured tier (one half
/// without the other) is almost always a misconfiguration, so we warn — with
/// the tier-specific message produced lazily by `orphan_msg` — and return
/// `None`, letting resolution fall through to the next tier.
fn complete_pair(
    key: Option<&str>,
    secret: Option<&str>,
    orphan_msg: impl Fn(Half) -> String,
) -> Option<(String, String)> {
    match (key, secret) {
        (Some(k), Some(s)) => Some((k.to_string(), s.to_string())),
        (Some(_), None) => {
            tracing::warn!("{}", orphan_msg(Half::SecretMissing));
            None
        }
        (None, Some(_)) => {
            tracing::warn!("{}", orphan_msg(Half::KeyMissing));
            None
        }
        (None, None) => None,
    }
}

/// Which half of a credential pair is missing, for orphan warnings.
#[derive(Clone, Copy)]
enum Half {
    KeyMissing,
    SecretMissing,
}

/// Resolve credentials for `engine` using precedence: flag > env > config.
///
/// At each tier, both key and secret must be present. If only one is provided,
/// a warning is emitted and resolution falls through to the next tier. Returns
/// `None` when no complete pair is found; callers turn that into the
/// user-facing "not configured" error.
///
/// Spot and Futures differ only in env-var names, config fields, and warning
/// wording — all selected by `engine`, so the precedence logic lives once here.
pub fn resolve_credentials(
    engine: Engine,
    flag_key: Option<&str>,
    flag_secret: Option<&str>,
    env_key: Option<&str>,
    env_secret: Option<&str>,
) -> Option<Credentials> {
    // Tier 1: command-line flags. The shared `--api-key`/`--api-secret` pair is
    // disambiguated only by `engine` in the orphan warning.
    let suffix = match engine {
        Engine::Spot => "",
        Engine::Futures => " for Futures",
    };
    if let Some((k, s)) = complete_pair(flag_key, flag_secret, |half| match half {
        Half::SecretMissing => format!(
            "--api-key provided without --api-secret{suffix}. \
             Flag credentials ignored, falling back to env/config."
        ),
        Half::KeyMissing => format!(
            "--api-secret provided without --api-key{suffix}. \
             Flag credentials ignored, falling back to env/config."
        ),
    }) {
        return Some(Credentials::from_plaintext(k, s, CredentialSource::Flag));
    }

    // Tier 2: environment variables. Empty-string values are treated as unset so
    // plugin hosts that pass blank user input through env vars fall through.
    let (k_env, s_env) = engine.env_names();
    if let Some((k, s)) =
        complete_pair(
            normalize_env(env_key),
            normalize_env(env_secret),
            |half| match half {
                Half::SecretMissing => format!(
                    "{k_env} is set but {s_env} is missing — \
                 env credentials ignored, falling back to config."
                ),
                Half::KeyMissing => format!(
                    "{s_env} is set but {k_env} is missing — \
                 env credentials ignored, falling back to config."
                ),
            },
        )
    {
        return Some(Credentials::from_plaintext(k, s, CredentialSource::Env));
    }

    // Tier 3: config file. Both fields must be present (no warning — a
    // half-filled config file is the user's own persisted state).
    match engine.config_pair(load().ok()?.auth) {
        (Some(api_key), Some(api_secret)) => Some(Credentials {
            api_key,
            api_secret,
            source: CredentialSource::Config,
        }),
        _ => None,
    }
}

/// Read a secret from stdin (one line, trimmed). Returns an error on EOF/empty input.
pub fn read_secret_from_stdin() -> Result<SecretString> {
    let mut buf = String::new();
    let n = std::io::stdin()
        .read_line(&mut buf)
        .map_err(KrakenError::Io)?;
    let trimmed = buf.trim().to_string();
    if n == 0 || trimmed.is_empty() {
        return Err(KrakenError::Auth(
            "Empty secret received from stdin — did you forget to pipe input?".into(),
        ));
    }
    Ok(SecretString::from(trimmed))
}

/// Read a secret from a file path. Returns an error if the file is empty.
pub fn read_secret_from_file(path: &Path) -> Result<SecretString> {
    let contents = fs::read_to_string(path)?;
    let trimmed = contents.trim().to_string();
    if trimmed.is_empty() {
        return Err(KrakenError::Auth(format!(
            "Empty secret read from file: {}",
            path.display()
        )));
    }
    Ok(SecretString::from(trimmed))
}

#[cfg(unix)]
fn atomic_write_restricted(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let dir = path
        .parent()
        .ok_or_else(|| KrakenError::Config("config path has no parent directory".into()))?;
    let tmp_path = dir.join(".config.tmp");

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
fn atomic_write_restricted(path: &Path, data: &[u8]) -> Result<()> {
    fs::write(path, data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    #[test]
    fn secret_string_debug_is_redacted() {
        // secrecy renders `SecretBox<str>([REDACTED])` — the exact wrapper
        // text is an implementation detail, so assert the invariant that
        // matters: the secret never appears and the output is marked redacted.
        let secret = SecretString::from("my_actual_secret".to_string());
        let debug_output = format!("{:?}", secret);
        assert!(
            !debug_output.contains("my_actual_secret"),
            "secret leaked in Debug output: {debug_output}"
        );
        assert!(
            debug_output.contains("[REDACTED]"),
            "Debug output should be marked redacted: {debug_output}"
        );
    }

    // Note: `SecretString` deliberately has no `Display` impl, so `format!("{}", secret)`
    // does not compile. That is a stronger guarantee than the previous custom
    // `Display => [REDACTED]`, so there is no runtime Display test to keep.

    #[test]
    fn credentials_debug_redacts_key_and_secret() {
        // Both fields are SecretStrings, so Debug fully redacts them (no last-4
        // mask — that view lives only in `auth show`).
        let creds = Credentials {
            api_key: SecretString::from("abcdefghijklmnop"),
            api_secret: SecretString::from("supersecret12345"),
            source: CredentialSource::Env,
        };
        let debug = format!("{:?}", creds);
        assert!(
            !debug.contains("abcdefghijklmnop"),
            "api_key must not appear in Debug output: {debug}"
        );
        assert!(
            !debug.contains("supersecret12345"),
            "api_secret must not appear in Debug output: {debug}"
        );
        // No partial key material either — not even the last-4 tail.
        assert!(
            !debug.contains("mnop"),
            "api_key tail leaked in Debug: {debug}"
        );
        assert!(
            debug.contains("[REDACTED]"),
            "key and secret should be redacted: {debug}"
        );
    }

    #[test]
    fn auth_config_debug_redacts_all_credentials() {
        // AuthConfig Debug fully redacts every field — including the keys,
        // which are no longer last-4 masked here. The last-4 mask is reserved
        // for the explicit `auth show` command (see commands/auth.rs).
        let auth = AuthConfig {
            api_key: Some("abcdefghijklmnop".into()),
            api_secret: Some("supersecret12345".into()),
            futures_api_key: Some("futureskey123456".into()),
            futures_api_secret: Some("futuresecret1234".into()),
        };
        let debug = format!("{:?}", auth);
        for plaintext in [
            "abcdefghijklmnop",
            "supersecret12345",
            "futureskey123456",
            "futuresecret1234",
        ] {
            assert!(
                !debug.contains(plaintext),
                "credential leaked in Debug: {debug}"
            );
        }
        // Not even key tails should appear now that masking is gone here.
        assert!(
            !debug.contains("mnop") && !debug.contains("3456"),
            "key tail leaked in Debug: {debug}"
        );
        assert!(
            debug.contains("[REDACTED]"),
            "all credentials should be redacted: {debug}"
        );
    }

    #[test]
    fn normalize_env_treats_unset_as_none() {
        assert_eq!(normalize_env(None), None);
    }

    #[test]
    fn normalize_env_treats_empty_string_as_none() {
        assert_eq!(normalize_env(Some("")), None);
    }

    #[test]
    fn normalize_env_preserves_set_value() {
        assert_eq!(normalize_env(Some("abc123")), Some("abc123"));
    }

    #[test]
    fn normalize_env_preserves_whitespace() {
        // Whitespace is deliberately preserved; only the literal empty string
        // is treated as absent. Whitespace in a credential is almost certainly
        // a user mistake, and we want the Kraken API call to surface that.
        assert_eq!(normalize_env(Some("   ")), Some("   "));
    }

    #[test]
    fn config_path_resolves_to_config_toml() {
        let path = config_path().expect("config_path() should succeed on any desktop OS");
        assert!(
            path.ends_with("config.toml"),
            "config path should end with config.toml, got: {}",
            path.display()
        );
        let parent = path.parent().expect("config path should have a parent dir");
        assert!(
            parent.ends_with("kraken"),
            "config dir should end with 'kraken', got: {}",
            parent.display()
        );
    }

    #[test]
    fn config_path_display_is_valid_string() {
        let display = config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "the kraken config file".into());
        assert!(!display.is_empty(), "display string should not be empty");
        assert!(
            display.contains("kraken"),
            "display string should contain 'kraken', got: {display}"
        );
    }

    #[test]
    fn config_roundtrip() {
        let cfg = KrakenConfig {
            auth: AuthConfig {
                api_key: Some("test_key".into()),
                api_secret: Some("test_secret".into()),
                ..Default::default()
            },
            settings: SettingsConfig {
                default_pair: Some("XBTUSD".into()),
                ..Default::default()
            },
        };
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        let deserialized: KrakenConfig = toml::from_str(&serialized).unwrap();
        // Secrets roundtrip through the `serialize_with` exposure and the
        // `SecretString` `Deserialize` impl.
        assert_eq!(
            deserialized
                .auth
                .api_key
                .as_ref()
                .map(|s| s.expose_secret()),
            Some("test_key")
        );
        assert_eq!(
            deserialized
                .auth
                .api_secret
                .as_ref()
                .map(|s| s.expose_secret()),
            Some("test_secret")
        );
        assert_eq!(
            deserialized.settings.default_pair.as_deref(),
            Some("XBTUSD")
        );
    }

    /// Guard: all user-facing configuration env vars are read by clap via
    /// `#[arg(env = ...)]` in [`crate::cli`]. The only module that reads the
    /// environment directly is `telemetry.rs`, which *senses* the calling agent
    /// from third-party runtime markers (`CURSOR_AGENT`, `AGENT`, …) — not Kraken
    /// configuration. `config.rs` is allowlisted only because this guard's own
    /// assertion contains the `env::var` literal; it reads no env itself.
    #[test]
    fn env_reads_are_centralised() {
        use std::fs;
        use std::path::Path;

        const ALLOWED: &[&str] = &["config.rs", "telemetry.rs"];

        fn check_dir(dir: &Path, allowed: &[&str]) {
            for entry in fs::read_dir(dir).expect("read src dir") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    check_dir(&path, allowed);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if allowed.contains(&name) {
                        continue;
                    }
                    let src = fs::read_to_string(&path).expect("read source file");
                    assert!(
                        !src.contains("env::var"),
                        "{} reads an environment variable directly; route it through the \
                         accessors in config.rs (or clap's #[arg(env = ...)]) instead",
                        path.display()
                    );
                }
            }
        }

        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        check_dir(&src_dir, ALLOWED);
    }
}
