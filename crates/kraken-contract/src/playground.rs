//! One-command demo composed from ordinary workspaces and sessions.

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct PlaygroundCommand {
    /// Comma-separated trading pairs to record and demo against.
    #[arg(long, value_delimiter = ',', default_value = "BTC/USD")]
    #[serde(default = "btc_usd")]
    pub symbols: Vec<String>,
    /// The template (recipe skill) driving the demo; recorded as the session's
    /// strategy label so scoring can attribute it.
    #[arg(long, default_value = "recipe-playground-dca")]
    #[serde(default = "dca_template")]
    pub template: String,
    /// How long the demo session records before closing itself.
    #[arg(long = "for", default_value = "1h")]
    #[serde(default = "one_hour")]
    pub window: String,
}

/// The pair the playground trades when the caller names none.
fn btc_usd() -> Vec<String> {
    vec!["BTC/USD".to_owned()]
}

/// The recipe the playground runs when the caller names none.
fn dca_template() -> String {
    "recipe-playground-dca".to_owned()
}

/// How long the playground runs when the caller names none.
fn one_hour() -> String {
    "1h".to_owned()
}
