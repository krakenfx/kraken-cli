/// Utility commands: `setup` wizard.
use crate::config::{self, AuthConfig, KrakenConfig, SettingsConfig};
use crate::errors::Result;
use crate::output::CommandOutput;

/// Run the interactive setup wizard.
pub(crate) async fn setup(verbose: bool) -> Result<CommandOutput> {
    if verbose {
        crate::output::verbose("Starting setup wizard");
    }

    println!("Kraken CLI Setup");
    println!("================");
    println!();
    println!("This wizard will configure your Kraken API credentials.");
    let config_path_display = config::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "the kraken config file".into());
    println!("Your API secret will be stored in {config_path_display} (mode 0600).");
    println!();
    println!("SECURITY NOTE: Never share your API secret. For automation, consider using");
    println!("environment variables (KRAKEN_API_KEY, KRAKEN_API_SECRET) instead of storing");
    println!("credentials on disk.");
    println!();

    let api_key: String = dialoguer::Input::new()
        .with_prompt("Spot API Key")
        .interact_text()
        .map_err(|e| crate::errors::KrakenError::Config(format!("Input error: {e}")))?;

    let api_secret: String = dialoguer::Password::new()
        .with_prompt("Spot API Secret")
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

    let cfg = KrakenConfig {
        auth: AuthConfig {
            api_key: Some(api_key),
            api_secret: Some(api_secret),
            futures_api_key: futures_key,
            futures_api_secret: futures_secret,
        },
        settings: SettingsConfig {
            default_pair: Some(default_pair),
            output: Some("table".to_string()),
        },
    };

    config::save(&cfg)?;
    let msg = config::config_path()
        .map(|p| format!("Setup complete! Credentials saved to {}", p.display()))
        .unwrap_or_else(|_| "Setup complete! Credentials saved.".into());
    Ok(CommandOutput::message(&msg))
}
