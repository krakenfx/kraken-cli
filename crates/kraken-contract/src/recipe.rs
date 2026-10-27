//! `kraken recipe`: arguments for packaging and rerunning a finished session.

use clap::Subcommand;

/// The `recipe` family: packaging a finished session's setup so the next run
/// reproduces it.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RecipeCommand {
    /// Derive a reusable recipe from a stopped session: an editable
    /// `recipe.json` plus a draft `SKILL.md` an agent can follow.
    ///
    /// The recipe carries the driver skill, frozen strategy params, symbols,
    /// channels, sinks and provenance. Needs a paper workspace scope — a
    /// real-account session would bake real equity into a portable
    /// artifact. Refuses a still-recording session, a session with no
    /// strategy label (unless `--driver` names one), and an existing recipe
    /// name (unless `--overwrite`). The recipe library is global, not
    /// workspace-scoped.
    Create(Create),
    /// List the local recipe library.
    List(List),
    /// Show one recipe's contract and where its artifacts live.
    Show(Show),
    /// Run a recipe: provision (or reuse) its paper workspace and start the
    /// session it records.
    Run(Run),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Create {
    /// Name for the new recipe (letters, digits, '.', '-', '_').
    pub name: String,
    /// Source session: `latest`, an ordinal (`s3`), or a label.
    #[arg(long, default_value = "latest")]
    #[serde(default = "crate::value_types::latest_session")]
    pub from_session: String,
    /// Strategy skill for a session that recorded no strategy label.
    #[arg(long)]
    pub driver: Option<String>,
    /// One-line description stored on the recipe.
    #[arg(long)]
    pub description: Option<String>,
    /// Replace an existing recipe of the same name.
    #[arg(long)]
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct List {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Show {
    /// Recipe name.
    pub name: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Run {
    /// Recipe name (see `kraken recipe list`).
    pub name: String,
    /// Override the recipe's recorded symbols (comma-separated).
    #[arg(long, value_delimiter = ',', conflicts_with = "from")]
    pub symbols: Option<Vec<String>>,
    /// Auto-stop after this wall-clock window (e.g. 20s, 5m, 1h); without it
    /// the session records until `kraken session stop`.
    #[arg(long = "for")]
    pub window: Option<String>,
    /// Replay a finalized tape (`tape:<name>` or `session:s<n>`) instead of
    /// the live market.
    #[arg(long, conflicts_with = "real_account")]
    pub from: Option<String>,
    /// Playback speed for --from: tape seconds per wall second.
    #[arg(long, default_value_t = 1.0, requires = "from")]
    #[serde(default = "crate::value_types::real_time")]
    pub speed: f64,
    /// Lab experiment to stamp on the session — what `lab compare` discovers
    /// runs by.
    #[arg(long)]
    pub experiment: Option<String>,
    /// Run against the REAL Kraken account instead of the recipe's paper
    /// workspace. Asks for confirmation; the session's evidence carries the
    /// whole-portfolio caveats printed at start.
    #[arg(long)]
    #[serde(default)]
    pub real_account: bool,
}

crate::tools! {
    /// The `kraken recipe` commands.
    fn recipe in ["recipe"] of RecipeCommand {
        Create as "create" => Workspace, Local,       None, Tool
            example "kraken recipe create btc-levels --from-session latest -o json";
        List   as "list"   => Workspace, ReadOnly,    None, Tool
            example "kraken recipe list -o json";
        Show   as "show"   => Workspace, ReadOnly,    None, Tool
            example "kraken recipe show btc-levels -o json";
        Run    as "run"    => Workspace, LocalOrLive, None, Stream
            example "kraken recipe run btc-levels --for 15m";
    }
}
