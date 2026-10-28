//! Scaffold CLI-client extensions. There is no plugin ABI: the CLI's

use clap::Subcommand;
use std::path::PathBuf;

/// The `plugin` family: scaffolding client extensions over this CLI's JSON
/// output. The CLI never executes an extension — the contract is the API.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PluginCommand {
    /// Scaffold a runnable extension: a stdlib-only entrypoint over `kraken ...
    /// -o json`, its manifest, a loadable `SKILL.md`, and a README.
    ///
    /// The JSON output is the whole extension API — there is no plugin ABI, and
    /// kraken never executes the code. Prints the absolute path and the
    /// symlink line that installs the skill. Refuses an unsafe name and a
    /// non-empty target directory.
    Init(Init),
    /// List scaffolded extensions.
    List(List),
}

/// Which language a scaffolded extension is written in.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    clap::ValueEnum,
    strum::Display,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "lowercase")]
pub enum Lang {
    Python,
    Bash,
}

/// What a scaffolded extension does with the CLI's output.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    clap::ValueEnum,
    strum::Display,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "lowercase")]
pub enum Kind {
    Indicator,
    Alert,
    Exporter,
    Driver,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Init {
    /// Name for the new extension (letters, digits, '.', '-', '_').
    pub name: String,
    /// Implementation language for the generated entrypoint.
    #[arg(long, value_enum)]
    pub lang: Lang,
    /// What the extension is for; tailors the generated example.
    #[arg(long, value_enum, default_value = "indicator")]
    #[serde(default = "indicator")]
    pub kind: Kind,
    /// Output directory (defaults to the plugin library, `<config
    /// dir>/plugins/<NAME>`).
    #[arg(long)]
    pub path: Option<PathBuf>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct List {}

/// The extension kind scaffolded when the caller names none.
fn indicator() -> Kind {
    Kind::Indicator
}

crate::tools! {
    /// The `kraken plugin` commands.
    fn plugin in ["plugin"] of PluginCommand {
        Init as "init" => Workspace, Local,    None, Tool
            example "kraken plugin init volume-profile --lang python --kind indicator -o json";
        List as "list" => Workspace, ReadOnly, None, Tool
            example "kraken plugin list -o json";
    }
}
