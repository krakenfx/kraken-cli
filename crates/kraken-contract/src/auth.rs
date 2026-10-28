//! `kraken auth`.

use clap::Subcommand;

/// The `auth` family: the stored API credentials this CLI signs with.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AuthCommand {
    /// Sign in with your Kraken account in the browser.
    ///
    /// Creates a restricted API key for this machine and saves it: no
    /// withdrawal permissions, and an expiry date. A new sign-in replaces the
    /// key that the last sign-in created. If you also set a key yourself, you
    /// choose which key is active.
    Login(Login),
    /// Save API credentials to the config file.
    ///
    /// The API secret is resolved via the global `--api-secret-stdin`,
    /// `--api-secret-file`, or `--api-secret` flags. If none are provided,
    /// an interactive prompt is shown. Pass `--api-secret` directly here
    /// as a last resort (it exposes the secret in process listings).
    Set(Set),
    /// Show the stored API keys, the active key, and the key in use (secrets
    /// are masked).
    #[command(visible_alias = "status")]
    Show(Show),
    /// Choose which stored Spot key is active: the API key from `kraken auth
    /// set` or the sign-in key from `kraken auth login`.
    ///
    /// A flag or an env var still overrides the active key. Without an
    /// argument, a prompt asks which key to use.
    Switch(Switch),
    /// Test authentication by calling the Balance endpoint.
    Test(Test),
    /// Delete stored credentials.
    Reset(Reset),
}

// The futures-secret sources cross-reference each other and the futures key
// via `requires` / `conflicts_with_all`, so all six fields must stay in this
// one struct for clap to resolve the identifiers.
#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Set {
    /// Kraken API key.
    #[arg(long)]
    #[serde(skip)]
    pub api_key: String,
    /// Kraken API secret (prefer global --api-secret-stdin for security).
    #[arg(long)]
    #[serde(skip)]
    pub api_secret: Option<String>,
    /// Kraken Futures API key (optional).
    #[arg(long)]
    #[serde(skip)]
    pub futures_api_key: Option<String>,
    /// Kraken Futures API secret (prefer --futures-api-secret-stdin for
    /// security).
    #[arg(long, requires = "futures_api_key", conflicts_with_all = ["futures_api_secret_stdin", "futures_api_secret_file"])]
    #[serde(skip)]
    pub futures_api_secret: Option<String>,
    /// Read Futures API secret from stdin (mutually exclusive with
    /// --futures-api-secret and --futures-api-secret-file).
    #[arg(long, requires = "futures_api_key", conflicts_with_all = ["futures_api_secret", "futures_api_secret_file"])]
    #[serde(default)]
    pub futures_api_secret_stdin: bool,
    /// Path to file containing Futures API secret (mutually exclusive with
    /// --futures-api-secret and --futures-api-secret-stdin).
    #[arg(long, requires = "futures_api_key", conflicts_with_all = ["futures_api_secret", "futures_api_secret_stdin"])]
    pub futures_api_secret_file: Option<std::path::PathBuf>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Login {
    /// Create a read-only key: balances, orders, trades and ledger, but it
    /// cannot trade.
    #[arg(long)]
    #[serde(default)]
    pub read_only: bool,
    /// How long the key stays valid (for example 30d or 90d). Default: 90d.
    #[arg(long)]
    pub valid_for: Option<String>,
    /// Print the sign-in URL, but do not open a browser. Over SSH, forward the
    /// redirect ports 27461 to 27463 first (`ssh -L 27461:localhost:27461 -L
    /// 27462:localhost:27462 -L 27463:localhost:27463`): the listener takes the
    /// first free one. Then open the URL on your own machine.
    #[arg(long)]
    #[serde(default)]
    pub no_browser: bool,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Show {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Switch {
    /// The key to activate. Omit it to choose in a prompt.
    pub key: Option<SpotKey>,
}

/// One of the two stored Spot keys.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum SpotKey {
    /// The API key that you set with `kraken auth set`.
    ApiKey,
    /// The sign-in key that `kraken auth login` created.
    SignIn,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Test {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Reset {}

crate::tools! {
    /// The `kraken auth` commands.
    fn auth in ["auth"] of AuthCommand {
        Login  as "login"  => Auth, Dangerous, None, Interactive
            example "kraken auth login";
        Set    as "set"    => Auth, Local,     None, Interactive
            example "kraken auth set --api-key <KEY> --api-secret-stdin";
        Show   as "show"   => Auth, ReadOnly,  None, Tool
            example "kraken auth show";
        Switch as "switch" => Auth, Dangerous, None, Withheld
            example "kraken auth switch sign-in";
        Test   as "test"   => Auth, ReadOnly,  Spot, Tool
            example "kraken auth test";
        Reset  as "reset"  => Auth, Dangerous, None, Withheld
            example "kraken auth reset";
    }
}
