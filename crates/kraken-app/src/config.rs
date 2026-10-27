//! The CLI config directory and the `[settings]` table of `config.toml`. The
//! `[auth]` table of the same file belongs to [`kraken_auth::CredentialStore`].
use crate::errors::{KrakenError, Result};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::path::PathBuf;

const SETTINGS_TABLE: &str = "settings";

/// General settings section.
#[skip_serializing_none]
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct SettingsConfig {
    pub(crate) default_pair: Option<String>,
    pub(crate) output: Option<String>,
}

/// Returns the config directory path: `~/.config/kraken/`.
pub fn config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir()
        .ok_or_else(|| KrakenError::Config("Cannot determine config directory".into()))?;
    Ok(base.join("kraken"))
}

pub(crate) fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

/// The stored API keys, in the same file as the settings.
pub(crate) fn credential_store() -> Result<kraken_auth::CredentialStore> {
    Ok(kraken_auth::CredentialStore::new(config_path()?))
}

/// The stored keys, read from `config.toml` on first use only, so a flag or env
/// key costs no read.
#[derive(Default)]
pub(crate) struct StoredKeys(std::cell::OnceCell<kraken_auth::StoredAuth>);

impl StoredKeys {
    pub(crate) fn credentials(
        &self,
        engine: kraken_auth::Engine,
    ) -> Option<kraken_auth::Credentials> {
        kraken_auth::resolve_stored(engine, self.0.get_or_init(load_stored_keys))
    }
}

/// Keys already in hand, so a test never reads the real config file.
impl From<kraken_auth::StoredAuth> for StoredKeys {
    fn from(stored: kraken_auth::StoredAuth) -> Self {
        Self(std::cell::OnceCell::from(stored))
    }
}

/// A config file that cannot be read gives a warning, not an error, so the
/// commands that need no key still run.
fn load_stored_keys() -> kraken_auth::StoredAuth {
    let loaded = credential_store().and_then(|store| store.load().map_err(Into::into));
    loaded.unwrap_or_else(|error| {
        tracing::warn!(%error, "could not read the stored API keys");
        kraken_auth::StoredAuth::default()
    })
}

/// Writes the `[settings]` table through the one writer of `config.toml`, so
/// the `[auth]` table and its 0600 mode stay intact.
pub(crate) fn save_settings(settings: &SettingsConfig) -> Result<()> {
    Ok(credential_store()?.replace_table(SETTINGS_TABLE, settings)?)
}

/// Env var name that allows non-Kraken hosts in URL overrides, surfaced in the
/// untrusted-host error message. The value is read by clap (see
/// [`crate::cli`]); this constant only names it for user guidance.
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Guard: all user-facing configuration env vars are read by clap via
    /// `#[arg(env = ...)]` in [`crate::cli`]. The only module that reads the
    /// environment directly is `telemetry.rs`, which *senses* the calling agent
    /// from third-party runtime markers (`CURSOR_AGENT`, `AGENT`, …) — not
    /// Kraken configuration. `config.rs` is allowlisted only because this
    /// guard's own assertion contains the `env::var` literal; it reads no
    /// env itself.
    #[test]
    fn env_reads_are_centralised() {
        use std::{fs, path::Path};

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
