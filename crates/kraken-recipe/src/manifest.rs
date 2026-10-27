//! The durable recipe contract and its derivation from a finished session.

use crate::{RecipeError, Result, app::CreateSpec};
use chrono::{DateTime, Utc};
use kraken_session::manifest::SessionManifest;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::collections::BTreeSet;

/// Recipe schema version; compatibility is MAJOR-only (see [`is_compatible`]).
pub const RECIPE_VERSION: &str = "1.0";

/// The two files of a recipe directory.
pub(crate) const RECIPE_FILE: &str = "recipe.json";
pub(crate) const SKILL_FILE: &str = "SKILL.md";

fn default_recipe_version() -> String {
    RECIPE_VERSION.to_string()
}

pub fn is_compatible(found: &str) -> bool {
    fn major(version: &str) -> Option<&str> {
        version.split('.').next()
    }
    major(found) == major(RECIPE_VERSION)
}

/// A portable, editable description of how to start the next session.
/// No seal: tuning `params` between runs is the intended workflow, so an
/// edit must stay a normal edit, never a `Damaged` error.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    #[serde(default = "default_recipe_version")]
    pub recipe_version: String,
    pub cli_version: String,
    pub name: String,
    pub description: Option<String>,
    pub driver: Driver,
    /// Strategy knobs, verbatim from the source session.
    pub params: Option<serde_json::Value>,
    pub market: MarketSpec,
    pub account: AccountSpec,
    pub origin: Origin,
    pub created_at: DateTime<Utc>,
}

/// What executes the recipe. One variant today; a future executable driver
/// is an additive `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Driver {
    /// An agent skill drives the session — the strategy label.
    Skill { name: String },
}

impl Driver {
    pub fn skill(&self) -> &str {
        match self {
            Self::Skill { name } => name,
        }
    }
}

/// The market feeds the source session consumed, unioned across its
/// recordings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarketSpec {
    pub symbols: Vec<String>,
    pub channels: Vec<String>,
    /// Recording backends for `--to`: `duckdb`, `jsonl`.
    pub sinks: Vec<String>,
}

/// The paper account shape the source session ran on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountSpec {
    #[serde(with = "rust_decimal::serde::str")]
    pub capital: Decimal,
    pub currency: String,
}

/// Where the recipe came from — evidence pointers, not executable state.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    pub session: String,
    pub label: Option<String>,
    pub experiment: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub tape: Option<TapeOrigin>,
}

/// The replay source the session ran from, when it was a replay.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TapeOrigin {
    pub source: String,
    pub content_hash: Option<String>,
}

/// Derives the recipe a finished session implies. Pure: the caller owns the
/// clock, storage owns persistence. An explicit `--driver` wins over the
/// session's own strategy label; a session with neither is refused because
/// executable behavior cannot be recovered from prose decisions.
pub fn from_session(
    session: &SessionManifest,
    spec: &CreateSpec,
    created_at: DateTime<Utc>,
) -> Result<Recipe> {
    if session.window.ended_at.is_none() {
        return Err(RecipeError::Spec(format!(
            "session '{}' has not stopped — only a closed window can seed a recipe",
            session.id
        )));
    }
    if spec.name.parse::<kraken_session::SessionName>().is_err() {
        return Err(RecipeError::Spec(format!(
            "recipe name '{}' is not a safe directory name — use letters, digits, '.', '-', '_'",
            spec.name
        )));
    }
    let skill = match (&spec.driver, &session.strategy) {
        (Some(driver), _) => driver.clone(),
        (None, Some(strategy)) => strategy.name.clone(),
        (None, None) => {
            return Err(RecipeError::Spec(format!(
                "session '{}' has no strategy label, so there is nothing executable to package — \
                 pass --driver <skill> to name the skill that drove it",
                session.id
            )));
        }
    };
    // The resolved label lands verbatim in SKILL.md frontmatter and a shell
    // command; only the safe name grammar may pass, whichever source it
    // came from.
    if skill.parse::<kraken_session::SessionName>().is_err() {
        return Err(RecipeError::Spec(format!(
            "strategy label '{skill}' is not a safe skill name — use letters, digits, '.', '-', \
             '_'"
        )));
    }
    if session.recordings.is_empty() {
        return Err(RecipeError::Spec(format!(
            "session '{}' has no market recordings — a recipe needs feeds to resubscribe",
            session.id
        )));
    }
    Ok(Recipe {
        recipe_version: RECIPE_VERSION.to_string(),
        cli_version: env!("CARGO_PKG_VERSION").to_string(),
        name: spec.name.clone(),
        description: spec.description.clone(),
        driver: Driver::Skill { name: skill },
        params: session.strategy.as_ref().and_then(|s| s.params.clone()),
        market: market_union(session),
        account: AccountSpec {
            capital: session.paper.starting_balance,
            currency: session.paper.currency.clone(),
        },
        origin: Origin {
            session: session.id.clone(),
            label: session.label.clone(),
            experiment: session.experiment.clone(),
            started_at: session.window.started_at,
            ended_at: session.window.ended_at,
            tape: session.source.as_ref().map(|source| TapeOrigin {
                source: source.tape.clone(),
                content_hash: source.content_hash.clone(),
            }),
        },
        created_at,
    })
}

/// Deduplicated, ordered union across recordings: two backends recording the
/// same feed must not double a symbol in the regenerated start command.
fn market_union(session: &SessionManifest) -> MarketSpec {
    let mut symbols = BTreeSet::new();
    let mut channels = BTreeSet::new();
    let mut sinks = BTreeSet::new();
    for recording in &session.recordings {
        symbols.extend(recording.symbols.iter().cloned());
        channels.extend(recording.channels.iter().cloned());
        sinks.insert(recording.backend.to_string());
    }
    MarketSpec {
        symbols: symbols.into_iter().collect(),
        channels: channels.into_iter().collect(),
        sinks: sinks.into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_session::{manifest::StrategyRef, timeline::fixtures};

    fn spec(name: &str) -> CreateSpec {
        CreateSpec {
            name: name.to_string(),
            description: None,
            driver: None,
            overwrite: false,
        }
    }

    fn stopped_session() -> SessionManifest {
        let mut session = fixtures::manifest(vec![fixtures::jsonl_recording()]);
        session.window.ended_at = Some("2026-01-01T00:30:00Z".parse().expect("valid instant"));
        session.strategy = Some(StrategyRef {
            name: "recipe-playground-price-alert".to_string(),
            params: Some(serde_json::json!({"upper": 70000, "lower": 60000})),
        });
        session
    }

    fn created_at() -> DateTime<Utc> {
        "2026-02-01T12:00:00Z".parse().expect("valid instant")
    }

    #[test]
    fn an_open_window_is_refused() {
        let mut session = stopped_session();
        session.window.ended_at = None;
        let err = from_session(&session, &spec("open"), created_at()).expect_err("must refuse");
        assert_eq!(err.category(), "validation");
        assert!(err.to_string().contains("has not stopped"), "{err}");
    }

    #[test]
    fn a_session_without_strategy_label_needs_an_explicit_driver() {
        let mut session = stopped_session();
        session.strategy = None;
        let err = from_session(&session, &spec("bare"), created_at()).expect_err("must refuse");
        assert!(err.to_string().contains("--driver"), "{err}");

        let mut with_driver = spec("bare");
        with_driver.driver = Some("my-skill".to_string());
        let recipe = from_session(&session, &with_driver, created_at()).expect("derive");
        assert_eq!(recipe.driver.skill(), "my-skill");
        assert_eq!(recipe.params, None);
    }

    #[test]
    fn an_explicit_driver_wins_over_the_session_label_and_keeps_params() {
        let session = stopped_session();
        let mut override_spec = spec("overridden");
        override_spec.driver = Some("other-skill".to_string());
        let recipe = from_session(&session, &override_spec, created_at()).expect("derive");
        assert_eq!(recipe.driver.skill(), "other-skill");
        assert!(recipe.params.is_some(), "params stay with the run");
    }

    #[test]
    fn a_session_without_recordings_is_refused() {
        let mut session = stopped_session();
        session.recordings.clear();
        let err = from_session(&session, &spec("deaf"), created_at()).expect_err("must refuse");
        assert!(err.to_string().contains("no market recordings"), "{err}");
    }

    #[test]
    fn an_unsafe_recipe_name_is_refused() {
        let session = stopped_session();
        let err =
            from_session(&session, &spec("../escape"), created_at()).expect_err("must refuse");
        assert_eq!(err.category(), "validation");
    }

    #[test]
    fn an_unsafe_strategy_label_is_refused_from_either_source() {
        // The label lands verbatim in SKILL.md frontmatter and a shell line,
        // so both the --driver override and the session's own label are held
        // to the safe grammar.
        let session = stopped_session();
        let mut injected = spec("inject");
        injected.driver = Some("evil\"]\nrun: rm -rf /".to_string());
        let err = from_session(&session, &injected, created_at()).expect_err("must refuse");
        assert_eq!(err.category(), "validation");

        let mut session = stopped_session();
        if let Some(strategy) = &mut session.strategy {
            strategy.name = "not a skill name".to_string();
        }
        let err = from_session(&session, &spec("inject"), created_at()).expect_err("must refuse");
        assert!(err.to_string().contains("not a safe skill name"), "{err}");
    }

    #[test]
    fn recordings_union_dedupes_symbols_and_channels() {
        let mut session = stopped_session();
        session.recordings = vec![fixtures::jsonl_recording(), fixtures::jsonl_recording()];
        let recipe = from_session(&session, &spec("dedup"), created_at()).expect("derive");
        assert_eq!(recipe.market.symbols, vec!["BTC/USD"]);
        assert_eq!(recipe.market.sinks, vec!["jsonl"]);
    }
}
