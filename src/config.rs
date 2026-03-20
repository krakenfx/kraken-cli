/// Configuration management for `~/.config/kraken/config.toml`.
///
/// Handles loading, saving, and resetting configuration with secure file
/// permissions (0600). Implements credential precedence: flag > env > config.
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct AuthConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) api_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) futures_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) futures_api_secret: Option<String>,
}

/// General settings section.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct SettingsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) default_pair: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) output: Option<String>,
}

impl std::fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthConfig")
            .field("api_key", &self.api_key.as_ref().map(|k| mask_string(k)))
            .field(
                "api_secret",
                &self.api_secret.as_ref().map(|_| "[REDACTED]".to_string()),
            )
            .field(
                "futures_api_key",
                &self.futures_api_key.as_ref().map(|k| mask_string(k)),
            )
            .field(
                "futures_api_secret",
                &self
                    .futures_api_secret
                    .as_ref()
                    .map(|_| "[REDACTED]".to_string()),
            )
            .finish()
    }
}

/// Wrapper that keeps secrets out of Debug, Display, and Serialize output.
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(val: String) -> Self {
        Self(val)
    }

    /// Provides read-only access to the secret. Callers must not log the result.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl std::fmt::Display for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Resolved credentials for Spot API calls.
#[derive(Debug)]
pub struct SpotCredentials {
    pub api_key: String,
    pub api_secret: SecretValue,
    pub source: CredentialSource,
}

/// Resolved credentials for Futures API calls.
#[derive(Debug)]
pub struct FuturesCredentials {
    pub api_key: String,
    pub api_secret: SecretValue,
    pub source: CredentialSource,
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

/// Returns the full path to the config file.
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

/// Save configuration to disk with 0600 permissions.
pub(crate) fn save(cfg: &KrakenConfig) -> Result<()> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir)?;
    let path = dir.join("config.toml");
    let contents = toml::to_string_pretty(cfg)
        .map_err(|e| KrakenError::Config(format!("TOML serialize error: {e}")))?;
    fs::write(&path, &contents)?;
    set_permissions_0600(&path)?;
    Ok(())
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

/// Resolve Spot credentials using precedence: flag > env > config.
///
/// At each tier, both key and secret must be present. If only one is provided,
/// a warning is emitted and resolution falls through to the next tier.
pub(crate) fn resolve_spot_credentials(
    flag_key: Option<&str>,
    flag_secret: Option<&str>,
) -> Result<SpotCredentials> {
    // Flags first (pair-level: both must be present)
    match (flag_key, flag_secret) {
        (Some(k), Some(s)) => {
            return Ok(SpotCredentials {
                api_key: k.to_string(),
                api_secret: SecretValue::new(s.to_string()),
                source: CredentialSource::Flag,
            });
        }
        (Some(_), None) => {
            crate::output::warn(
                "--api-key provided without --api-secret (or --api-secret-stdin/--api-secret-file). \
                 Flag credentials ignored, falling back to env/config.",
            );
        }
        (None, Some(_)) => {
            crate::output::warn(
                "--api-secret provided without --api-key. \
                 Flag credentials ignored, falling back to env/config.",
            );
        }
        (None, None) => {}
    }

    // Environment variables (pair-level: both must be present)
    let env_key = env::var("KRAKEN_API_KEY").ok();
    let env_secret = env::var("KRAKEN_API_SECRET").ok();
    match (&env_key, &env_secret) {
        (Some(k), Some(s)) => {
            return Ok(SpotCredentials {
                api_key: k.clone(),
                api_secret: SecretValue::new(s.clone()),
                source: CredentialSource::Env,
            });
        }
        (Some(_), None) => {
            crate::output::warn(
                "KRAKEN_API_KEY is set but KRAKEN_API_SECRET is missing — \
                 env credentials ignored, falling back to config.",
            );
        }
        (None, Some(_)) => {
            crate::output::warn(
                "KRAKEN_API_SECRET is set but KRAKEN_API_KEY is missing — \
                 env credentials ignored, falling back to config.",
            );
        }
        (None, None) => {}
    }

    // Config file
    let cfg = load()?;
    match (cfg.auth.api_key, cfg.auth.api_secret) {
        (Some(k), Some(s)) => Ok(SpotCredentials {
            api_key: k,
            api_secret: SecretValue::new(s),
            source: CredentialSource::Config,
        }),
        _ => Err(KrakenError::Auth(
            "No Spot API credentials found. Use `kraken auth set` or set KRAKEN_API_KEY / KRAKEN_API_SECRET env vars.".into(),
        )),
    }
}

/// Resolve Futures credentials using precedence: flag > env > config.
///
/// At each tier, both key and secret must be present. If only one is provided,
/// a warning is emitted and resolution falls through to the next tier.
pub(crate) fn resolve_futures_credentials(
    flag_key: Option<&str>,
    flag_secret: Option<&str>,
) -> Result<FuturesCredentials> {
    match (flag_key, flag_secret) {
        (Some(k), Some(s)) => {
            return Ok(FuturesCredentials {
                api_key: k.to_string(),
                api_secret: SecretValue::new(s.to_string()),
                source: CredentialSource::Flag,
            });
        }
        (Some(_), None) => {
            crate::output::warn(
                "--api-key provided without --api-secret for Futures. \
                 Flag credentials ignored, falling back to env/config.",
            );
        }
        (None, Some(_)) => {
            crate::output::warn(
                "--api-secret provided without --api-key for Futures. \
                 Flag credentials ignored, falling back to env/config.",
            );
        }
        (None, None) => {}
    }

    let env_key = env::var("KRAKEN_FUTURES_API_KEY").ok();
    let env_secret = env::var("KRAKEN_FUTURES_API_SECRET").ok();
    match (&env_key, &env_secret) {
        (Some(k), Some(s)) => {
            return Ok(FuturesCredentials {
                api_key: k.clone(),
                api_secret: SecretValue::new(s.clone()),
                source: CredentialSource::Env,
            });
        }
        (Some(_), None) => {
            crate::output::warn(
                "KRAKEN_FUTURES_API_KEY is set but KRAKEN_FUTURES_API_SECRET is missing — \
                 env credentials ignored, falling back to config.",
            );
        }
        (None, Some(_)) => {
            crate::output::warn(
                "KRAKEN_FUTURES_API_SECRET is set but KRAKEN_FUTURES_API_KEY is missing — \
                 env credentials ignored, falling back to config.",
            );
        }
        (None, None) => {}
    }

    let cfg = load()?;
    match (cfg.auth.futures_api_key, cfg.auth.futures_api_secret) {
        (Some(k), Some(s)) => Ok(FuturesCredentials {
            api_key: k,
            api_secret: SecretValue::new(s),
            source: CredentialSource::Config,
        }),
        _ => Err(KrakenError::Auth(
            "No Futures API credentials found. Use `kraken setup` or set KRAKEN_FUTURES_API_KEY / KRAKEN_FUTURES_API_SECRET env vars.".into(),
        )),
    }
}

/// Read a secret from stdin (one line, trimmed). Returns an error on EOF/empty input.
pub fn read_secret_from_stdin() -> Result<SecretValue> {
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
    Ok(SecretValue::new(trimmed))
}

/// Read a secret from a file path. Returns an error if the file is empty.
pub fn read_secret_from_file(path: &Path) -> Result<SecretValue> {
    let contents = fs::read_to_string(path)?;
    let trimmed = contents.trim().to_string();
    if trimmed.is_empty() {
        return Err(KrakenError::Auth(format!(
            "Empty secret read from file: {}",
            path.display()
        )));
    }
    Ok(SecretValue::new(trimmed))
}

/// Mask a string for display, showing only the first 4 and last 4 characters.
pub(crate) fn mask_string(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 8 {
        return "****".to_string();
    }
    let prefix: String = chars[..4].iter().collect();
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    format!("{prefix}...{suffix}")
}

#[cfg(unix)]
fn set_permissions_0600(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_permissions_0600(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_value_debug_is_redacted() {
        let secret = SecretValue::new("my_actual_secret".to_string());
        let debug_output = format!("{:?}", secret);
        assert_eq!(debug_output, "[REDACTED]");
        assert!(!debug_output.contains("my_actual_secret"));
    }

    #[test]
    fn secret_value_display_is_redacted() {
        let secret = SecretValue::new("my_actual_secret".to_string());
        let display_output = format!("{}", secret);
        assert_eq!(display_output, "[REDACTED]");
    }

    #[test]
    fn mask_string_short() {
        assert_eq!(mask_string("abc"), "****");
    }

    #[test]
    fn mask_string_long() {
        assert_eq!(mask_string("abcdefghij"), "abcd...ghij");
    }

    #[test]
    fn auth_config_debug_masks_keys() {
        let auth = AuthConfig {
            api_key: Some("abcdefghijklmnop".into()),
            api_secret: Some("supersecret12345".into()),
            futures_api_key: Some("futureskey123456".into()),
            futures_api_secret: Some("futuresecret1234".into()),
        };
        let debug = format!("{:?}", auth);
        assert!(
            !debug.contains("abcdefghijklmnop"),
            "api_key leaked in Debug: {debug}"
        );
        assert!(
            !debug.contains("supersecret12345"),
            "api_secret leaked in Debug: {debug}"
        );
        assert!(
            !debug.contains("futureskey123456"),
            "futures_api_key leaked in Debug: {debug}"
        );
        assert!(
            !debug.contains("futuresecret1234"),
            "futures_api_secret leaked in Debug: {debug}"
        );
        assert!(
            debug.contains("abcd...mnop"),
            "api_key should be masked: {debug}"
        );
        assert!(
            debug.contains("[REDACTED]"),
            "secrets should be redacted: {debug}"
        );
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
        assert_eq!(deserialized.auth.api_key.as_deref(), Some("test_key"));
        assert_eq!(
            deserialized.settings.default_pair.as_deref(),
            Some("XBTUSD")
        );
    }
}
