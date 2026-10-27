use super::active_key::{active_cell, offer_new_key, report_override};
use crate::{
    AppContext,
    commands::Execute,
    config,
    errors::{KrakenError, Result},
    output::CommandOutput,
};
use kraken_auth::{CredentialSource, KeyPair, KeySlot, SecretSource};
use kraken_contract::auth::Set;
use secrecy::{ExposeSecret, SecretString};

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

        // The global `--api-secret-stdin` / `-file` / `--api-secret` resolve
        // first; this subcommand's own `--api-secret` is the fallback.
        let spot_secret = match ctx.api_secret.clone() {
            Some(secret) => Some(secret),
            None => kraken_auth::read_secret(SecretSource {
                stdin: false,
                file: None,
                inline: api_secret.map(SecretString::from),
                inline_flag: "--api-secret",
            })?,
        };
        let spot_secret = spot_secret.map_or_else(|| prompt_secret("API Secret"), Ok)?;
        let spot = KeyPair::new(api_key, usable(spot_secret, "API secret")?);
        let futures = match futures_api_key {
            Some(key) => {
                let secret = kraken_auth::read_secret(SecretSource {
                    stdin: futures_api_secret_stdin,
                    file: futures_api_secret_file,
                    inline: futures_api_secret.map(SecretString::from),
                    inline_flag: "--futures-api-secret",
                })?;
                let secret = secret.map_or_else(|| prompt_secret("Futures API Secret"), Ok)?;
                Some(KeyPair::new(key, usable(secret, "Futures API secret")?))
            }
            None => None,
        };

        let store = config::credential_store()?;
        let (stored, ()) = store.update(|stored| {
            stored.set_own(spot);
            if let Some(futures) = futures {
                stored.set_futures(futures);
            }
            Ok(())
        })?;
        let active = offer_new_key(&store, stored, KeySlot::ApiKey);
        // This command's own `--api-key` is the flag tier, so only an env key
        // keeps the stored key unused afterwards.
        let overridden_by =
            report_override(active, ctx.spot_env_key.then_some(CredentialSource::Env));

        let message = "Credentials saved successfully.";
        let pairs = vec![
            ("Message".into(), message.to_string()),
            ("Active Spot Key".into(), active_cell(active)),
        ];
        let json = serde_json::json!({
            "message": message,
            "active_spot_key": active,
            "overridden_by": overridden_by,
        });
        Ok(CommandOutput::key_value(pairs, json))
    }
}

fn prompt_secret(prompt: &str) -> Result<SecretString> {
    dialoguer::Password::new()
        .with_prompt(prompt)
        .interact()
        .map(SecretString::from)
        .map_err(|e| KrakenError::Config(format!("Input error: {e}")))
}

/// The secret, unless it is empty or the request signer cannot decode it.
fn usable(secret: SecretString, what: &str) -> Result<SecretString> {
    if secret.expose_secret().is_empty() {
        return Err(KrakenError::Auth(format!("Cannot save an empty {what}.")));
    }
    if !super::is_decodable_secret(secret.expose_secret()) {
        return Err(KrakenError::Validation(format!(
            "Cannot save the {what}: it is not valid base64. Paste the Private Key exactly as \
             Kraken shows it."
        )));
    }
    Ok(secret)
}
