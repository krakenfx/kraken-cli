//! Session-derived recipes: the artifact that records what to run again.
//!
//! A session records what happened; a [`Recipe`] records how to start the
//! next one — driver skill, frozen params, market feeds, and provenance — as
//! one portable directory holding `recipe.json` plus a draft `SKILL.md`.
//! Recipes are editable inputs (tuning `params` is the intended workflow),
//! so the artifact carries no integrity seal; sealing stays with the lab's
//! experiments, where goalposts must not move after evidence exists.

mod app;
mod digest;
mod manifest;
mod skill;
mod store;

pub use app::{CreateSpec, ListedRecipe, Recipes, StoredRecipe};
pub use digest::{DecisionDigest, digest};
pub use manifest::{
    AccountSpec, Driver, MarketSpec, Origin, RECIPE_VERSION, Recipe, TapeOrigin, from_session,
    is_compatible,
};
pub use skill::draft_skill;
pub use store::{recipe_dir, recipe_path, recipes_root, skill_path};

pub type Result<T, E = RecipeError> = std::result::Result<T, E>;

/// The recipe layer's failure modes. The binary maps each variant onto the
/// JSON envelope exactly once, in its `errors` module.
#[derive(Debug, thiserror::Error)]
pub enum RecipeError {
    /// A recipe that doesn't exist. → envelope `validation`.
    #[error("recipe '{0}' not found; list local recipes with 'kraken recipe list'")]
    NotFound(String),
    /// A create under a taken name — an existing recipe is edited or
    /// replaced deliberately, never silently. → envelope `validation`.
    #[error("recipe '{0}' already exists — pass --overwrite to replace it")]
    Exists(String),
    /// A session the recipe grammar refuses to package (no strategy label,
    /// no recordings, an open window, a bad name). → envelope `validation`.
    #[error("{0}")]
    Spec(String),
    /// A recipe written by a newer MAJOR schema. → envelope `config`.
    #[error(
        "recipe schema {found} is newer than this build reads ({expected}) — upgrade kraken-cli"
    )]
    Incompatible { found: String, expected: String },
    /// A stored recipe the schema can't decode. → envelope `parse`.
    #[error("{0}")]
    Damaged(String),
    /// Filesystem failure around the recipe store. → envelope `io`.
    #[error(transparent)]
    Store(#[from] std::io::Error),
    /// Writing an artifact through the recording store failed; the envelope
    /// mapping delegates to the recording bridge.
    #[error(transparent)]
    Recording(#[from] kraken_recording::Error),
}

impl RecipeError {
    /// The stable envelope category this failure maps to. Kept in sync with
    /// the binary's error bridge (pinned there by
    /// `recipe_error_categories_are_pinned`) so crate tests can assert
    /// routing without a round-trip through the CLI error type.
    pub fn category(&self) -> &'static str {
        match self {
            Self::NotFound(_) | Self::Exists(_) | Self::Spec(_) => "validation",
            Self::Incompatible { .. } => "config",
            Self::Damaged(_) => "parse",
            Self::Store(_) => "io",
            Self::Recording(e) => match e {
                kraken_recording::Error::Rejected(_) => "validation",
                kraken_recording::Error::Damaged(_) => "parse",
                _ => "io",
            },
        }
    }
}
