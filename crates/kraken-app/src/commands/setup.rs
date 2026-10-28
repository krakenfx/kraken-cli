//! First-run setup wizard: prompts for API credentials and writes the config
//! file.
use crate::{
    config::{self, SettingsConfig},
    errors::Result,
    output::CommandOutput,
};
use kraken_auth::{KeyPair, KeySlot, StoredAuth};
use std::io::IsTerminal;

pub(crate) async fn run() -> Result<CommandOutput> {
    // dialoguer reads stdin and draws on stderr, so an agent or pipe gets the fix
    // here instead of its raw "IO error: not a terminal".
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        return Err(crate::errors::KrakenError::Validation(
            "kraken setup needs a terminal to read your API keys — run it in a terminal window, \
             or set KRAKEN_API_KEY and KRAKEN_API_SECRET instead"
                .into(),
        ));
    }
    tracing::info!("setup wizard started");
    // Read before the prompts, so a broken file fails before anything is typed.
    let store = config::credential_store()?;
    store.load()?;

    // The banner shares stderr with the prompts, so stdout stays one JSON value.
    eprintln!("Kraken CLI Setup");
    eprintln!("================");
    eprintln!();
    eprintln!("This wizard will configure your Kraken API credentials.");
    let config_path_display = config::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "the kraken config file".into());
    eprintln!("Your API secret will be stored in {config_path_display} (mode 0600).");
    eprintln!();
    eprintln!("SECURITY NOTE: Never share your API secret. For automation, consider using");
    eprintln!("environment variables (KRAKEN_API_KEY, KRAKEN_API_SECRET) instead of storing");
    eprintln!("credentials on disk.");
    eprintln!();

    let api_key: String = dialoguer::Input::new()
        .with_prompt("Spot API Key")
        .interact_text()
        .map_err(|e| crate::errors::KrakenError::Config(format!("Input error: {e}")))?;

    let api_secret: String = dialoguer::Password::new()
        .with_prompt("Spot API Secret")
        .validate_with(|secret: &String| check_secret(secret))
        .interact()
        .map_err(|e| crate::errors::KrakenError::Config(format!("Input error: {e}")))?;

    let setup_futures = dialoguer::Confirm::new()
        .with_prompt("Configure Futures API credentials?")
        .default(false)
        .interact()
        .map_err(|e| crate::errors::KrakenError::Config(format!("Input error: {e}")))?;

    let (futures_key, futures_secret) = if setup_futures {
        let fk: String = dialoguer::Input::new()
            .with_prompt("Futures API Key")
            .interact_text()
            .map_err(|e| crate::errors::KrakenError::Config(format!("Input error: {e}")))?;
        let fs: String = dialoguer::Password::new()
            .with_prompt("Futures API Secret")
            .validate_with(|secret: &String| check_secret(secret))
            .interact()
            .map_err(|e| crate::errors::KrakenError::Config(format!("Input error: {e}")))?;
        (Some(fk), Some(fs))
    } else {
        (None, None)
    };

    let default_pair: String = dialoguer::Input::new()
        .with_prompt("Default trading pair")
        .default("XBTUSD".to_string())
        .interact_text()
        .map_err(|e| crate::errors::KrakenError::Config(format!("Input error: {e}")))?;

    let futures = futures_key
        .zip(futures_secret)
        .map(|(key, secret)| KeyPair::new(key, secret));
    let (stored, ()) = store.update(|stored| {
        replace_keys(stored, KeyPair::new(api_key, api_secret), futures);
        Ok(())
    })?;
    config::save_settings(&SettingsConfig {
        default_pair: Some(default_pair),
        output: Some("table".to_string()),
    })?;
    crate::commands::auth::offer_new_key(&store, stored, KeySlot::ApiKey);
    if let Ok(path) = config::config_path() {
        tracing::debug!(path = %path.display(), "credentials saved");
    }
    let msg = config::config_path()
        .map(|p| format!("Setup complete! Credentials saved to {}", p.display()))
        .unwrap_or_else(|_| "Setup complete! Credentials saved.".into());
    Ok(CommandOutput::message(&msg))
}

/// Setup replaces the credentials, so declining Futures drops an older Futures
/// key rather than leave it signing unseen. The sign-in key stays.
fn replace_keys(stored: &mut StoredAuth, spot: KeyPair, futures: Option<KeyPair>) {
    stored.set_own(spot);
    match futures {
        Some(pair) => stored.set_futures(pair),
        None => stored.clear_futures(),
    }
}

const NOT_A_SECRET: &str =
    "This is not a valid API secret. Paste the Private Key exactly as Kraken shows it.";

/// A prompt validator: dialoguer shows the error and asks again.
fn check_secret(secret: &str) -> std::result::Result<(), &'static str> {
    if crate::commands::auth::is_decodable_secret(secret) {
        Ok(())
    } else {
        Err(NOT_A_SECRET)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declining_futures_drops_the_old_futures_key() {
        let mut stored = StoredAuth::default();
        stored.set_futures(KeyPair::new("old-futures", "b2xk"));
        replace_keys(&mut stored, KeyPair::new("spot", "c3BvdA=="), None);
        assert!(stored.futures().is_none());
        assert!(stored.own().is_some());
    }
}
