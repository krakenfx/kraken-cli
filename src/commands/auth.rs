/// `kraken auth` subcommands: set, show, test, reset.
use std::collections::HashMap;

use clap::Subcommand;
use enum_dispatch::enum_dispatch;
use secrecy::{ExposeSecret, SecretString};

use super::Execute;
use crate::AppContext;
use crate::config;
use crate::errors::Result;
use crate::output::CommandOutput;

#[enum_dispatch(Execute)]
#[derive(Debug, Subcommand)]
pub(crate) enum AuthCommand {
    /// Save API credentials to the config file.
    ///
    /// The API secret is resolved via the global `--api-secret-stdin`,
    /// `--api-secret-file`, or `--api-secret` flags. If none are provided,
    /// an interactive prompt is shown. Pass `--api-secret` directly here
    /// as a last resort (it exposes the secret in process listings).
    Set(Set),
    /// Show configured API key (secret is masked).
    Show(Show),
    /// Test authentication by calling the Balance endpoint.
    Test(Test),
    /// Delete stored credentials.
    Reset(Reset),
}

// The futures-secret sources cross-reference each other and the futures key
// via `requires` / `conflicts_with_all`, so all six fields must stay in this
// one struct for clap to resolve the identifiers.
#[derive(Debug, clap::Args)]
pub(crate) struct Set {
    /// Kraken API key.
    #[arg(long)]
    api_key: String,
    /// Kraken API secret (prefer global --api-secret-stdin for security).
    #[arg(long)]
    api_secret: Option<String>,
    /// Kraken Futures API key (optional).
    #[arg(long)]
    futures_api_key: Option<String>,
    /// Kraken Futures API secret (prefer --futures-api-secret-stdin for security).
    #[arg(long, requires = "futures_api_key", conflicts_with_all = ["futures_api_secret_stdin", "futures_api_secret_file"])]
    futures_api_secret: Option<String>,
    /// Read Futures API secret from stdin (mutually exclusive with --futures-api-secret and --futures-api-secret-file).
    #[arg(long, requires = "futures_api_key", conflicts_with_all = ["futures_api_secret", "futures_api_secret_file"])]
    futures_api_secret_stdin: bool,
    /// Path to file containing Futures API secret (mutually exclusive with --futures-api-secret and --futures-api-secret-stdin).
    #[arg(long, requires = "futures_api_key", conflicts_with_all = ["futures_api_secret", "futures_api_secret_stdin"])]
    futures_api_secret_file: Option<std::path::PathBuf>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Show {}

#[derive(Debug, clap::Args)]
pub(crate) struct Test {}

#[derive(Debug, clap::Args)]
pub(crate) struct Reset {}

/// Mask an API key to its last-4 tail for identification: `****jedl`.
///
/// Like the last four digits of an account number, the tail tells the operator
/// which key is configured without exposing usable material. Keys of four or
/// fewer characters are fully masked so a short key is never shown whole.
fn mask_tail(key: &SecretString) -> String {
    let count = key.expose_secret().chars().count();
    if count <= 4 {
        return "****".to_string();
    }
    let tail: String = key.expose_secret().chars().skip(count - 4).collect();
    format!("****{tail}")
}

impl Execute for Set {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            api_key,
            api_secret,
            futures_api_key,
            futures_api_secret,
            futures_api_secret_stdin,
            futures_api_secret_file,
        } = self;

        let secret: SecretString = if let Some(s) = &ctx.api_secret {
            s.clone()
        } else if let Some(s) = api_secret {
            tracing::warn!(
                "Passing --api-secret on the command line exposes it in process listings. \
                 Prefer --api-secret-stdin, --api-secret-file, or `kraken setup` for interactive entry."
            );
            SecretString::from(s)
        } else if ctx.mcp_mode {
            return Err(crate::errors::KrakenError::Validation(
                "API secret is required for auth set in non-interactive mode. \
                 Provide --api-secret, --api-secret-stdin, or --api-secret-file."
                    .into(),
            ));
        } else {
            let input = dialoguer::Password::new()
                .with_prompt("API Secret")
                .interact()
                .map_err(|e| crate::errors::KrakenError::Config(format!("Input error: {e}")))?;
            SecretString::from(input)
        };

        if secret.expose_secret().is_empty() {
            return Err(crate::errors::KrakenError::Auth(
                "Cannot save an empty API secret.".into(),
            ));
        }

        let mut cfg = config::load()?;
        cfg.auth.api_key = Some(SecretString::from(api_key));
        cfg.auth.api_secret = Some(secret);

        if let Some(fk) = futures_api_key {
            cfg.auth.futures_api_key = Some(SecretString::from(fk));

            let futures_secret = if futures_api_secret_stdin {
                if ctx.mcp_mode {
                    return Err(crate::errors::KrakenError::Validation(
                        "Cannot read Futures API secret from stdin in MCP mode \
                         (stdin is the JSON-RPC transport). Provide \
                         --futures-api-secret or --futures-api-secret-file instead."
                            .into(),
                    ));
                }
                config::read_secret_from_stdin()?
                    .expose_secret()
                    .to_string()
            } else if let Some(path) = futures_api_secret_file {
                config::read_secret_from_file(&path)?
                    .expose_secret()
                    .to_string()
            } else if let Some(fs) = futures_api_secret {
                tracing::warn!(
                    "Passing --futures-api-secret on the command line exposes it in process listings. \
                     Prefer --futures-api-secret-stdin, --futures-api-secret-file, or `kraken setup` for interactive entry."
                );
                fs
            } else if ctx.mcp_mode {
                return Err(crate::errors::KrakenError::Validation(
                    "Futures API secret is required for auth set in non-interactive mode. \
                     Provide --futures-api-secret, --futures-api-secret-stdin, or --futures-api-secret-file."
                        .into(),
                ));
            } else {
                dialoguer::Password::new()
                    .with_prompt("Futures API Secret")
                    .interact()
                    .map_err(|e| crate::errors::KrakenError::Config(format!("Input error: {e}")))?
            };

            if futures_secret.is_empty() {
                return Err(crate::errors::KrakenError::Auth(
                    "Cannot save an empty Futures API secret.".into(),
                ));
            }
            cfg.auth.futures_api_secret = Some(SecretString::from(futures_secret));
        }

        config::save(&cfg)?;

        Ok(CommandOutput::message("Credentials saved successfully."))
    }
}

impl Execute for Show {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        let cfg = config::load()?;

        // Render each field's masked string once, then drive both outputs
        // from it. Keys are identifiers and show a last-4 tail (`****jedl`)
        // so the operator can tell which key is configured; the secret is
        // never shown even partially and collapses to "[REDACTED]". `None`
        // means the field is not configured.
        let api_key = cfg.auth.api_key.as_ref().map(mask_tail);
        let api_secret = cfg
            .auth
            .api_secret
            .as_ref()
            .map(|_| "[REDACTED]".to_string());
        let futures_api_key = cfg.auth.futures_api_key.as_ref().map(mask_tail);

        let cell = |m: &Option<String>| m.as_deref().unwrap_or("(not set)").to_string();
        let pairs = vec![
            ("API Key".into(), cell(&api_key)),
            ("API Secret".into(), cell(&api_secret)),
            ("Futures API Key".into(), cell(&futures_api_key)),
        ];

        // JSON mirrors the table as `{present, masked}` so scripts can branch
        // on presence without parsing a string.
        let field = |m: &Option<String>| match m {
            Some(m) => serde_json::json!({ "present": true, "masked": m }),
            None => serde_json::json!({ "present": false }),
        };
        let json = serde_json::json!({
            "api_key": field(&api_key),
            "api_secret": field(&api_secret),
            "futures_api_key": field(&futures_api_key),
        });
        Ok(CommandOutput::key_value(pairs, json))
    }
}

impl Execute for Test {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds, otp) = ctx.spot_authed()?;
        let result = client
            .private_post("Balance", HashMap::new(), creds, otp, true)
            .await?;

        let pairs = vec![
            ("Status".into(), "Authentication successful".to_string()),
            ("Source".into(), creds.source.to_string()),
        ];
        let json = serde_json::json!({
            "status": "success",
            "source": creds.source.to_string(),
            "balances": result,
        });
        Ok(CommandOutput::key_value(pairs, json))
    }
}

impl Execute for Reset {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        config::reset_auth()?;
        Ok(CommandOutput::message("Credentials deleted."))
    }
}
