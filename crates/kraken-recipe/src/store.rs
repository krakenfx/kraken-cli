//! Filesystem and clock boundary behind [`crate::Recipes`].

use crate::{
    RecipeError, Result,
    app::CreateSpec,
    manifest::{RECIPE_FILE, RECIPE_VERSION, Recipe, SKILL_FILE, is_compatible},
};
use chrono::Utc;
use kraken_session::{decision::Decision, manifest::SessionManifest};
use std::{
    io::Write as _,
    path::{Path, PathBuf},
};

/// The library root: `<base>/recipes/`. Global, not workspace-scoped —
/// portability across accounts is the feature.
pub fn recipes_root(base: &Path) -> PathBuf {
    base.join("recipes")
}

/// One recipe's directory: `<base>/recipes/<name>/`.
pub fn recipe_dir(base: &Path, name: &str) -> PathBuf {
    recipes_root(base).join(name)
}

/// The contract file: `<base>/recipes/<name>/recipe.json`.
pub fn recipe_path(base: &Path, name: &str) -> PathBuf {
    recipe_dir(base, name).join(RECIPE_FILE)
}

/// The draft skill: `<base>/recipes/<name>/SKILL.md`.
pub fn skill_path(base: &Path, name: &str) -> PathBuf {
    recipe_dir(base, name).join(SKILL_FILE)
}

/// Derives and writes both artifacts, stamping storage-owned time.
///
/// Write order is deliberate and differs by case, because the pair is not
/// crash-atomic. Fresh create: the skill lands first — `recipe.json` is the
/// discovery key, so a listed recipe always has its skill beside it, and a
/// torn create leaves a hollow directory a re-create repairs. Overwrite: the
/// contract lands first — a torn overwrite then leaves a stale *draft*, never
/// a stale contract, and the draft is regenerable.
pub(crate) fn create(
    base: &Path,
    session: &SessionManifest,
    decisions: &[Decision],
    spec: &CreateSpec,
) -> Result<Recipe> {
    let recipe = crate::manifest::from_session(session, spec, Utc::now())?;
    let replacing = recipe_path(base, &recipe.name).exists();
    if replacing && !spec.overwrite {
        return Err(RecipeError::Exists(recipe.name.clone()));
    }
    let digest = crate::digest::digest(decisions);
    let skill = crate::skill::draft_skill(&recipe, &digest);
    if replacing {
        kraken_recording::write_json_atomic(&recipe_path(base, &recipe.name), &recipe)?;
        write_text_atomic(&skill_path(base, &recipe.name), &skill)?;
    } else {
        write_text_atomic(&skill_path(base, &recipe.name), &skill)?;
        kraken_recording::write_json_atomic(&recipe_path(base, &recipe.name), &recipe)?;
    }
    Ok(recipe)
}

/// Same tmp-beside-target-then-rename discipline as `write_json_atomic`,
/// for the non-JSON artifact. Parent-directory fsync is deliberately
/// skipped: the draft is regenerable, the contract write carries the full
/// guarantee.
fn write_text_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("md.tmp");
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Loads a contract, preserving distinct routing for absence, damage, and
/// version mismatch. The name is re-validated here so no caller can join an
/// unsafe path segment into the library.
pub(crate) fn load(base: &Path, name: &str) -> Result<Recipe> {
    if name.parse::<kraken_session::SessionName>().is_err() {
        return Err(RecipeError::NotFound(name.to_string()));
    }
    let path = recipe_path(base, name);
    let data = match std::fs::read_to_string(&path) {
        Ok(data) => data,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(RecipeError::NotFound(name.to_string()));
        }
        Err(err) => return Err(err.into()),
    };
    let raw: serde_json::Value = serde_json::from_str(&data)
        .map_err(|err| RecipeError::Damaged(format!("recipe '{name}' cannot be decoded: {err}")))?;
    // Gate the version before the strict decode: a newer MAJOR schema may
    // carry shapes this build cannot read (a new driver kind, say), and that
    // must report as `config`, not as damage.
    let found = raw
        .get("recipe_version")
        .and_then(|version| version.as_str())
        .unwrap_or(RECIPE_VERSION);
    if !is_compatible(found) {
        return Err(RecipeError::Incompatible {
            found: found.to_string(),
            expected: RECIPE_VERSION.to_string(),
        });
    }
    let recipe: Recipe = serde_json::from_value(raw)
        .map_err(|err| RecipeError::Damaged(format!("recipe '{name}' cannot be decoded: {err}")))?;
    if recipe.name != name {
        return Err(RecipeError::Damaged(format!(
            "recipe '{name}' names itself '{}' — a copied directory must be re-created, not \
             renamed",
            recipe.name
        )));
    }
    Ok(recipe)
}

/// Every stored recipe, sorted by name. A damaged artifact is a described
/// row — one broken recipe cannot hide the others.
pub(crate) fn list(base: &Path) -> Result<Vec<(String, Result<Recipe>)>> {
    let entries = match std::fs::read_dir(recipes_root(base)) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut rows = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.path().is_dir() {
            continue;
        }
        // Only safe names can be addressed by the commands; foreign dirs are
        // not recipes.
        let raw = entry.file_name();
        let name = raw.to_string_lossy().to_string();
        if name.parse::<kraken_session::SessionName>().is_err() {
            continue;
        }
        // A directory that exists but holds no contract must not describe
        // itself with the lookup's "not found" hint — list just found it.
        let recipe = load(base, &name).map_err(|err| match err {
            RecipeError::NotFound(_) => RecipeError::Damaged(format!(
                "directory holds no {RECIPE_FILE} — re-create the recipe or remove it"
            )),
            other => other,
        });
        rows.push((name, recipe));
    }
    rows.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    Ok(rows)
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
            params: Some(serde_json::json!({"upper": 70000})),
        });
        session
    }

    #[test]
    fn create_writes_both_artifacts_and_load_round_trips() {
        let base = tempfile::tempdir().expect("tempdir");
        let recipe =
            create(base.path(), &stopped_session(), &[], &spec("btc-levels")).expect("create");
        assert!(skill_path(base.path(), "btc-levels").exists());
        let loaded = load(base.path(), "btc-levels").expect("load");
        assert_eq!(loaded, recipe);
        assert_eq!(loaded.account.capital.to_string(), "10000.0");
    }

    #[test]
    fn create_refuses_a_taken_name_without_overwrite() {
        let base = tempfile::tempdir().expect("tempdir");
        create(base.path(), &stopped_session(), &[], &spec("btc")).expect("first create");
        let err =
            create(base.path(), &stopped_session(), &[], &spec("btc")).expect_err("must refuse");
        assert!(matches!(err, RecipeError::Exists(_)), "{err}");
        assert!(err.to_string().contains("--overwrite"), "{err}");
    }

    #[test]
    fn overwrite_replaces_in_place() {
        let base = tempfile::tempdir().expect("tempdir");
        create(base.path(), &stopped_session(), &[], &spec("btc")).expect("first create");
        let mut forced = spec("btc");
        forced.overwrite = true;
        forced.description = Some("second take".to_string());
        let recipe = create(base.path(), &stopped_session(), &[], &forced).expect("overwrite");
        assert_eq!(recipe.description.as_deref(), Some("second take"));
        let loaded = load(base.path(), "btc").expect("load");
        assert_eq!(loaded.description.as_deref(), Some("second take"));
    }

    #[test]
    fn unknown_recipe_is_not_found_with_the_list_hint() {
        let base = tempfile::tempdir().expect("tempdir");
        let err = load(base.path(), "ghost").expect_err("must refuse");
        assert_eq!(err.category(), "validation");
        assert!(err.to_string().contains("kraken recipe list"), "{err}");
    }

    #[test]
    fn an_unsafe_name_never_touches_the_filesystem() {
        let base = tempfile::tempdir().expect("tempdir");
        let err = load(base.path(), "../escape").expect_err("must refuse");
        assert!(matches!(err, RecipeError::NotFound(_)), "{err}");
    }

    #[test]
    fn a_damaged_artifact_is_a_parse_failure() {
        let base = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(recipe_dir(base.path(), "broken")).expect("mkdir");
        std::fs::write(recipe_path(base.path(), "broken"), "not json").expect("write");
        let err = load(base.path(), "broken").expect_err("must refuse");
        assert_eq!(err.category(), "parse");
    }

    #[test]
    fn a_newer_major_schema_is_incompatible() {
        let base = tempfile::tempdir().expect("tempdir");
        let recipe = create(base.path(), &stopped_session(), &[], &spec("future")).expect("create");
        let mut raw = serde_json::to_value(&recipe).expect("to json");
        raw["recipe_version"] = "2.0".into();
        kraken_recording::write_json_atomic(&recipe_path(base.path(), "future"), &raw)
            .expect("rewrite");
        let err = load(base.path(), "future").expect_err("must refuse");
        assert_eq!(err.category(), "config");
    }

    #[test]
    fn a_newer_schema_with_an_unreadable_shape_is_still_config_not_parse() {
        // An additive driver kind from a future MAJOR must report as a
        // version problem, not as damage — the version gate runs before the
        // strict decode.
        let base = tempfile::tempdir().expect("tempdir");
        let raw = serde_json::json!({
            "recipe_version": "2.0",
            "name": "future",
            "driver": { "kind": "binary", "path": "/usr/local/bin/strategy" },
        });
        kraken_recording::write_json_atomic(&recipe_path(base.path(), "future"), &raw)
            .expect("write");
        let err = load(base.path(), "future").expect_err("must refuse");
        assert_eq!(err.category(), "config");
    }

    #[test]
    fn an_artifact_naming_another_recipe_is_damaged() {
        let base = tempfile::tempdir().expect("tempdir");
        create(base.path(), &stopped_session(), &[], &spec("real")).expect("create");
        std::fs::create_dir_all(recipe_dir(base.path(), "copied")).expect("mkdir");
        std::fs::copy(
            recipe_path(base.path(), "real"),
            recipe_path(base.path(), "copied"),
        )
        .expect("copy");
        let err = load(base.path(), "copied").expect_err("must refuse");
        assert_eq!(err.category(), "parse");
    }

    #[test]
    fn unknown_artifact_keys_are_tolerated() {
        let base = tempfile::tempdir().expect("tempdir");
        let recipe = create(base.path(), &stopped_session(), &[], &spec("fwd")).expect("create");
        let mut raw = serde_json::to_value(&recipe).expect("to json");
        raw["runner"] = "future-field".into();
        kraken_recording::write_json_atomic(&recipe_path(base.path(), "fwd"), &raw)
            .expect("rewrite");
        assert!(load(base.path(), "fwd").is_ok());
    }

    #[test]
    fn a_missing_library_lists_empty() {
        let base = tempfile::tempdir().expect("tempdir");
        assert!(list(base.path()).expect("list").is_empty());
    }

    #[test]
    fn one_damaged_recipe_never_hides_the_others() {
        let base = tempfile::tempdir().expect("tempdir");
        create(base.path(), &stopped_session(), &[], &spec("healthy")).expect("create");
        std::fs::create_dir_all(recipe_dir(base.path(), "broken")).expect("mkdir");
        std::fs::write(recipe_path(base.path(), "broken"), "not json").expect("write");
        let rows = list(base.path()).expect("list");
        assert_eq!(rows.len(), 2);
        assert!(rows[0].1.is_err(), "broken sorts first and is described");
        assert!(rows[1].1.is_ok());
    }

    #[test]
    fn a_contract_less_directory_lists_as_described_damage_not_as_not_found() {
        let base = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(recipe_dir(base.path(), "hollow")).expect("mkdir");
        let rows = list(base.path()).expect("list");
        assert_eq!(rows.len(), 1);
        let err = rows[0].1.as_ref().expect_err("hollow dir is described");
        assert!(
            err.to_string().contains("holds no recipe.json"),
            "no circular 'not found' hint: {err}"
        );
    }
}
