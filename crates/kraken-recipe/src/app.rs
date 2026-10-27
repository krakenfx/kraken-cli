//! The `Recipes` facade — the one seam the binary dispatches to. The binary
//! resolves the library base and renders; derivation, layout, and refusal
//! policy live behind this type.

use crate::{Result, manifest::Recipe, store};
use chrono::{DateTime, Utc};
use kraken_session::{decision::Decision, manifest::SessionManifest};
use serde::Serialize;
use serde_with::skip_serializing_none;
use std::path::PathBuf;

/// Validated input for recipe creation; storage owns the timestamp.
#[derive(Debug, Clone)]
pub struct CreateSpec {
    pub name: String,
    pub description: Option<String>,
    /// Strategy label for a session that lacks one; an explicit flag wins
    /// over the session's own label.
    pub driver: Option<String>,
    /// Replace an existing recipe of the same name.
    pub overwrite: bool,
}

/// A recipe together with where it lives, so callers can point at the
/// artifacts without re-deriving library layout.
#[derive(Debug)]
pub struct StoredRecipe {
    pub recipe: Recipe,
    pub recipe_path: PathBuf,
    pub skill_path: PathBuf,
}

/// One `recipe list` row.
#[skip_serializing_none]
#[derive(Debug, Serialize)]
pub struct ListedRecipe {
    pub name: String,
    pub driver: Option<String>,
    pub symbols: Option<Vec<String>>,
    pub created_at: Option<DateTime<Utc>>,
    /// Why the artifact could not be read — and only then.
    pub damaged: Option<String>,
}

/// The recipe library over one base directory (the global config dir in
/// production, a tempdir in tests).
#[derive(Debug)]
pub struct Recipes {
    base: PathBuf,
}

impl Recipes {
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// Derive a recipe from a finished session and persist both artifacts.
    pub fn create(
        &self,
        session: &SessionManifest,
        decisions: &[Decision],
        spec: &CreateSpec,
    ) -> Result<StoredRecipe> {
        let recipe = store::create(&self.base, session, decisions, spec)?;
        Ok(self.located(recipe))
    }

    /// Every stored recipe as a display row; damage is described, not hidden.
    pub fn list(&self) -> Result<Vec<ListedRecipe>> {
        let rows = store::list(&self.base)?;
        Ok(rows
            .into_iter()
            .map(|(name, loaded)| match loaded {
                Ok(recipe) => ListedRecipe {
                    name,
                    driver: Some(recipe.driver.skill().to_string()),
                    symbols: Some(recipe.market.symbols),
                    created_at: Some(recipe.created_at),
                    damaged: None,
                },
                Err(err) => ListedRecipe {
                    name,
                    driver: None,
                    symbols: None,
                    created_at: None,
                    damaged: Some(err.to_string()),
                },
            })
            .collect())
    }

    pub fn show(&self, name: &str) -> Result<StoredRecipe> {
        let recipe = store::load(&self.base, name)?;
        Ok(self.located(recipe))
    }

    fn located(&self, recipe: Recipe) -> StoredRecipe {
        let recipe_path = store::recipe_path(&self.base, &recipe.name);
        let skill_path = store::skill_path(&self.base, &recipe.name);
        StoredRecipe {
            recipe,
            recipe_path,
            skill_path,
        }
    }
}
