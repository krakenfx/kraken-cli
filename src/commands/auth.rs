/// `kraken auth` subcommands: set, show, test, reset.
use std::collections::HashMap;

use clap::Subcommand;

use crate::config::{self, mask_string};
use crate::errors::Result;
use crate::output::CommandOutput;
use crate::AppContext;

#[derive(Debug, Subcommand)]
pub(crate) enum AuthCommand {
    /// Save API credentials to the config file.
    ///
    /// The API secret is resolved via the global `--api-secret-stdin`,
    /// `--api-secret-file`, or `--api-secret` flags. If none are provided,
    /// an interactive prompt is shown. Pass `--api-secret` directly here
    /// as a last resort (it exposes the secret in process listings).
    Set {
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
    },
    /// Show configured API key (secret is masked).
    Show,
    /// Test authentication by calling the Balance endpoint.
    Test,
    /// Delete stored credentials.
    Reset,
}

pub(crate) async fn execute(cmd: &AuthCommand, ctx: &AppContext) -> Result<CommandOutput> {
    match cmd {
        AuthCommand::Set {
            api_key,
            api_secret,
            futures_api_key,
            futures_api_secret,
            futures_api_secret_stdin,
            futures_api_secret_file,
        } => {
            let secret = if let Some(s) = &ctx.api_secret {
                s.clone()
            } else if let Some(s) = api_secret {
                crate::output::warn(
                    "Passing --api-secret on the command line exposes it in process listings. \
                     Prefer --api-secret-stdin, --api-secret-file, or `kraken setup` for interactive entry.",
                );
                s.clone()
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
                input
            };

            if secret.is_empty() {
                return Err(crate::errors::KrakenError::Auth(
                    "Cannot save an empty API secret.".into(),
                ));
            }

            let mut cfg = config::load()?;
            cfg.auth.api_key = Some(api_key.clone());
            cfg.auth.api_secret = Some(secret);

            if let Some(fk) = futures_api_key {
                cfg.auth.futures_api_key = Some(fk.clone());

                let futures_secret = if *futures_api_secret_stdin {
                    if ctx.mcp_mode {
                        return Err(crate::errors::KrakenError::Validation(
                            "Cannot read Futures API secret from stdin in MCP mode \
                             (stdin is the JSON-RPC transport). Provide \
                             --futures-api-secret or --futures-api-secret-file instead."
                                .into(),
                        ));
                    }
                    config::read_secret_from_stdin()?.expose().to_string()
                } else if let Some(ref path) = futures_api_secret_file {
                    config::read_secret_from_file(path)?.expose().to_string()
                } else if let Some(fs) = futures_api_secret {
                    crate::output::warn(
                        "Passing --futures-api-secret on the command line exposes it in process listings. \
                         Prefer --futures-api-secret-stdin, --futures-api-secret-file, or `kraken setup` for interactive entry.",
                    );
                    fs.clone()
                } else if ctx.mcp_mode {
                    return Err(crate::errors::KrakenError::Validation(
                        "Futures API secret is required for auth set in non-interactive mode. \
                         Provide --futures-api-secret, --futures-api-secret-stdin, or --futures-api-secret-file."
                            .into(),
                    ));
                } else {
                    let input = dialoguer::Password::new()
                        .with_prompt("Futures API Secret")
                        .interact()
                        .map_err(|e| {
                            crate::errors::KrakenError::Config(format!("Input error: {e}"))
                        })?;
                    input
                };

                if futures_secret.is_empty() {
                    return Err(crate::errors::KrakenError::Auth(
                        "Cannot save an empty Futures API secret.".into(),
                    ));
                }
                cfg.auth.futures_api_secret = Some(futures_secret);
            }

            config::save(&cfg)?;

            Ok(CommandOutput::message("Credentials saved successfully."))
        }
        AuthCommand::Show => {
            let cfg = config::load()?;

            // Table output shows a last-4 mask of the api key so a user with
            // multiple keys on the same machine can tell them apart at a glance,
            // but never exposes secret material. Secrets render as a fixed
            // "[REDACTED]" token in both modes: there is no legitimate reason to
            // eyeball secret prefixes, and aligning both renderers reduces the
            // screen-share / paste-to-Slack leak surface that motivates this
            // command's output shape.
            let key_display = cfg
                .auth
                .api_key
                .as_deref()
                .map(mask_string)
                .unwrap_or_else(|| "(not set)".to_string());
            let futures_key_display = cfg
                .auth
                .futures_api_key
                .as_deref()
                .map(mask_string)
                .unwrap_or_else(|| "(not set)".to_string());

            let secret_display = if cfg.auth.api_secret.is_some() {
                "[REDACTED]".to_string()
            } else {
                "(not set)".to_string()
            };

            let pairs = vec![
                ("API Key".into(), key_display.clone()),
                ("API Secret".into(), secret_display),
                ("Futures API Key".into(), futures_key_display.clone()),
            ];
            // JSON output emits structured `{present, masked}` objects so
            // downstream scripts can branch on presence without parsing a
            // masked string, and so log-scrubbers keyed on the literal
            // "[REDACTED]" token still match. The `masked` field is present
            // only when the key exists and is safe to display; full key
            // material never appears in JSON output.
            let json = serde_json::json!({
                "api_key": match cfg.auth.api_key.as_deref() {
                    Some(k) => serde_json::json!({
                        "present": true,
                        "masked": mask_string(k),
                    }),
                    None => serde_json::json!({ "present": false }),
                },
                "api_secret": match cfg.auth.api_secret.as_deref() {
                    Some(_) => serde_json::json!({ "present": true, "masked": "[REDACTED]" }),
                    None => serde_json::json!({ "present": false }),
                },
                "futures_api_key": match cfg.auth.futures_api_key.as_deref() {
                    Some(k) => serde_json::json!({
                        "present": true,
                        "masked": mask_string(k),
                    }),
                    None => serde_json::json!({ "present": false }),
                },
            });
            Ok(CommandOutput::key_value(pairs, json))
        }
        AuthCommand::Test => {
            let creds = config::resolve_spot_credentials(
                ctx.api_key.as_deref(),
                ctx.api_secret.as_deref(),
            )?;
            let client = crate::client::SpotClient::new(ctx.api_url.as_deref())?;
            let result = client
                .private_post(
                    "Balance",
                    HashMap::new(),
                    &creds,
                    ctx.otp.as_deref(),
                    true,
                    ctx.verbose,
                )
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
        AuthCommand::Reset => {
            config::reset_auth()?;
            Ok(CommandOutput::message("Credentials deleted."))
        }
    }
}
