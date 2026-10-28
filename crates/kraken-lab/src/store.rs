//! The experiment store: the crate's single filesystem-and-clock seam.
//!
//! Everything the read-model pipeline and the [`experiment`]
//! domain compute is pure; this module is where that purity ends. It writes a
//! sealed spec, reads and verifies one back, and discovers an experiment's runs
//! on disk. All state is files, so a fresh agent context reproduces any verdict
//! from the store alone — there is no in-memory cache and no daemon to trust.

use crate::{
    LabError, Result,
    experiment::{self, Criteria, Experiment, SessionPlan},
};
use chrono::Utc;
use kraken_recording::write_json_atomic;
use kraken_session::{
    manifest::SessionManifest,
    session::{parse_ordinal, session_file_path, sessions_root},
};
use std::path::{Path, PathBuf};

/// One discovered run of an experiment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRef {
    pub ordinal: u32,
    /// The session's id, `s<n>` — its directory name under `runs/`.
    pub session: String,
}

/// The store all experiments live under: `<base>/lab/experiments/`. `base`
/// is the workspace-scoped dir in production and a temp dir in tests.
pub fn experiments_root(base: &Path) -> PathBuf {
    base.join("lab").join("experiments")
}

fn experiment_path(base: &Path, name: &str) -> PathBuf {
    experiments_root(base).join(format!("{name}.json"))
}

/// Freeze a new experiment: seal the spec (validating it and stamping the
/// clock here, at the I/O boundary), refuse an existing name, and write the
/// sealed file atomically.
pub(crate) fn freeze(
    base: &Path,
    name: &str,
    hypothesis: &str,
    strategy: Option<String>,
    criteria: Criteria,
    session_plan: Option<SessionPlan>,
) -> Result<Experiment> {
    let experiment = experiment::seal(
        name,
        hypothesis,
        strategy,
        criteria,
        session_plan,
        Utc::now(),
    )?;
    let path = experiment_path(base, name);
    if std::fs::exists(&path)? {
        return Err(LabError::ExperimentExists(name.to_string()));
    }
    write_json_atomic(&path, &experiment).map_err(|e| match e {
        kraken_recording::Error::Io(io) => LabError::Store(io),
        other => LabError::Damaged(other.to_string()),
    })?;
    Ok(experiment)
}

/// Load a frozen experiment and verify its seal. A missing file is
/// `ExperimentNotFound`; an undecodable or tampered file is `Damaged` (the
/// seal check lives in the domain — see [`experiment::verify`]).
/// Every sealed experiment name in this scope, sorted. A missing lab tree
/// is an empty list, never an error.
pub(crate) fn experiments(base: &Path) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(experiments_root(base)) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut names = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "json")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            names.push(stem.to_string());
        }
    }
    names.sort();
    Ok(names)
}

pub(crate) fn load(base: &Path, name: &str) -> Result<Experiment> {
    let path = experiment_path(base, name);
    if !std::fs::exists(&path)? {
        return Err(LabError::ExperimentNotFound(name.to_string()));
    }
    let data = std::fs::read_to_string(&path)?;
    let experiment: Experiment = serde_json::from_str(&data)
        .map_err(|e| LabError::Damaged(format!("experiment '{name}' cannot be decoded: {e}")))?;
    experiment::verify(&experiment, name)?;
    Ok(experiment)
}

/// All of `experiment`'s runs under `base`, ordinal-ascending. Membership is
/// the `session.json` experiment stamp alone — labels are display handles,
/// never a discovery key. A damaged `session.json` cannot be attributed to any
/// experiment, so the session is skipped with a warning. This stays the single
/// discovery seam — every consumer routes through it.
pub(crate) fn discover_sessions(base: &Path, experiment: &str) -> Result<Vec<SessionRef>> {
    let entries = match std::fs::read_dir(sessions_root(base)) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut runs = Vec::new();
    for entry in entries {
        let entry = entry?;
        // A racing delete between readdir and stat is "not a session directory
        // anymore", not a reason to fail the whole discovery.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        // Run ids are ASCII by grammar, so a non-UTF-8 entry cannot be one.
        let Some(ordinal) = entry.file_name().to_str().and_then(parse_ordinal) else {
            continue;
        };
        match SessionManifest::load(&session_file_path(base, ordinal)) {
            Ok(run) if run.experiment.as_deref() == Some(experiment) => {
                runs.push(SessionRef {
                    ordinal,
                    session: format!("s{ordinal}"),
                });
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(run = format!("s{ordinal}"), error = %err,
                    "session.json unreadable; skipping — the session cannot be attributed to an experiment");
            }
        }
    }
    // Canonical ordinals are unique per scope, so this order is total.
    runs.sort_unstable_by_key(|run| run.ordinal);
    Ok(runs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_session::{
        manifest::{PaperRef, SessionManifest, SessionWindow},
        session::session_dir,
    };
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    fn criteria() -> Criteria {
        Criteria {
            min_return_pct: Some(dec!(1.0)),
            max_drawdown_pct: Some(dec!(5.0)),
            min_fills: Some(2),
        }
    }

    #[test]
    fn freeze_then_load_round_trips_the_sealed_spec() {
        let base = tempfile::tempdir().unwrap();
        let frozen = freeze(
            base.path(),
            "momentum-1",
            "buying strength beats the tape",
            Some("recipe-momentum".into()),
            criteria(),
            None,
        )
        .unwrap();
        assert!(frozen.frozen.starts_with("sha256:"));

        let loaded = load(base.path(), "momentum-1").unwrap();
        assert_eq!(loaded, frozen);
    }

    #[test]
    fn freezing_an_existing_name_is_refused() {
        let base = tempfile::tempdir().unwrap();
        freeze(base.path(), "m1", "h", None, criteria(), None).unwrap();
        let err = freeze(base.path(), "m1", "h2", None, criteria(), None).unwrap_err();
        assert!(matches!(err, LabError::ExperimentExists(_)), "got: {err}");
        assert!(err.to_string().contains("already exists"), "got: {err}");
    }

    #[test]
    fn loading_a_missing_experiment_names_the_new_command() {
        let base = tempfile::tempdir().unwrap();
        let err = load(base.path(), "ghost").unwrap_err();
        assert!(matches!(err, LabError::ExperimentNotFound(_)));
        assert_eq!(
            err.to_string(),
            "experiment 'ghost' not found; run 'kraken lab new ghost' first"
        );
    }

    #[test]
    fn tampering_after_freeze_reads_as_damage_not_rejection() {
        let base = tempfile::tempdir().unwrap();
        freeze(base.path(), "m1", "h", None, criteria(), None).unwrap();

        // Move the goalpost by hand: min_return_pct 1.0 → 0.0, through a
        // JSON round-trip so the tamper is independent of file formatting.
        let path = experiments_root(base.path()).join("m1.json");
        let mut doctored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        doctored["criteria"]["min_return_pct"] = serde_json::json!(0.0);
        std::fs::write(&path, serde_json::to_string(&doctored).unwrap()).unwrap();

        let err = load(base.path(), "m1").unwrap_err();
        assert!(matches!(err, LabError::Damaged(_)), "got: {err}");
        assert!(err.to_string().contains("modified after"), "got: {err}");
    }

    /// A rename leaves the content — and thus the seal — intact, so only the
    /// filename↔sealed-name binding catches it. This also stands in for the
    /// in-band case-insensitive-filesystem route (`compare M1` resolving to
    /// `m1.json`): both hand `load` a file whose sealed name is not the one
    /// requested, and both must fail loud so discovery never keys off it.
    #[test]
    fn a_renamed_spec_is_refused_though_its_seal_still_verifies() {
        let base = tempfile::tempdir().unwrap();
        freeze(base.path(), "m1", "h", None, criteria(), None).unwrap();
        let root = experiments_root(base.path());
        std::fs::rename(root.join("m1.json"), root.join("m2.json")).unwrap();

        let err = load(base.path(), "m2").unwrap_err();
        assert!(matches!(err, LabError::Damaged(_)), "got: {err}");
        assert!(
            err.to_string().contains("sealed under the name 'm1'"),
            "got: {err}"
        );
    }

    fn write_run(base: &Path, ordinal: u32, experiment: Option<&str>) {
        let run = format!("s{ordinal}");
        let manifest = SessionManifest::new(
            run.clone(),
            "0.0.0-test",
            0,
            Vec::new(),
            PaperRef {
                starting_balance: Decimal::ONE,
                currency: "USD".to_string(),
            },
            SessionWindow {
                session: run,
                started_at: "2026-01-01T00:00:00Z".parse().unwrap(),
                opening_equity: Decimal::ONE,
                opening_complete: true,
                ended_at: None,
            },
        )
        .with_experiment(experiment.map(str::to_string));
        manifest.save(&session_file_path(base, ordinal)).unwrap();
    }

    #[test]
    fn discover_runs_orders_ordinals_numerically_not_lexicographically() {
        let base = tempfile::tempdir().unwrap();
        for ordinal in [10, 2, 1] {
            write_run(base.path(), ordinal, Some("m1"));
        }
        let ordinals: Vec<u32> = discover_sessions(base.path(), "m1")
            .unwrap()
            .into_iter()
            .map(|run| run.ordinal)
            .collect();
        assert_eq!(ordinals, [1, 2, 10]);
    }

    #[test]
    fn discover_runs_keys_on_the_experiment_stamp_alone() {
        let base = tempfile::tempdir().unwrap();
        // Stamped for m1: the only membership rule.
        write_run(base.path(), 1, Some("m1"));
        // Stamped for another experiment: not a session of m1.
        write_run(base.path(), 2, Some("m2"));
        // No stamp at all: not an experiment run.
        write_run(base.path(), 3, None);

        let runs = discover_sessions(base.path(), "m1").unwrap();
        assert_eq!(
            runs,
            [SessionRef {
                ordinal: 1,
                session: "s1".to_string(),
            }]
        );
    }

    #[test]
    fn discover_runs_skips_non_run_entries() {
        let base = tempfile::tempdir().unwrap();
        write_run(base.path(), 1, Some("m1"));
        let root = sessions_root(base.path());
        // A leading-zero alias is not a session directory (see parse_ordinal).
        std::fs::create_dir_all(root.join("r01")).unwrap();
        // Nor is a directory outside the s<n> grammar.
        std::fs::create_dir_all(root.join("btc-dip")).unwrap();
        // A stray file named like a session is not a session directory.
        std::fs::write(root.join("s2"), "").unwrap();
        // A session directory without a session.json cannot be attributed.
        std::fs::create_dir_all(root.join("s3")).unwrap();

        let runs = discover_sessions(base.path(), "m1").unwrap();
        assert_eq!(
            runs,
            [SessionRef {
                ordinal: 1,
                session: "s1".to_string(),
            }]
        );
    }

    #[test]
    fn discover_runs_without_a_runs_tree_is_empty_not_an_error() {
        let base = tempfile::tempdir().unwrap();
        assert_eq!(discover_sessions(base.path(), "m1").unwrap(), []);

        std::fs::create_dir_all(sessions_root(base.path())).unwrap();
        assert_eq!(discover_sessions(base.path(), "m1").unwrap(), []);
    }

    /// A session.json the schema cannot decode carries no readable experiment
    /// stamp, so the session cannot be attributed — skipped (with a warning),
    /// never surfaced under the wrong experiment.
    #[test]
    fn a_damaged_run_json_is_skipped_not_attributed() {
        let base = tempfile::tempdir().unwrap();
        write_run(base.path(), 1, Some("m1"));
        std::fs::create_dir_all(session_dir(base.path(), 2)).unwrap();
        std::fs::write(session_file_path(base.path(), 2), "not json").unwrap();

        let runs = discover_sessions(base.path(), "m1").unwrap();
        assert_eq!(
            runs,
            [SessionRef {
                ordinal: 1,
                session: "s1".to_string(),
            }]
        );
    }
}
