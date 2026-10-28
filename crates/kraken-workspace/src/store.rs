//! Filesystem and clock boundary behind [`crate::Workspaces`].

use crate::{
    CreateSpec, Result, WorkspaceError,
    manifest::{WORKSPACE_FILE, WORKSPACE_VERSION, WorkspaceManifest, is_compatible},
    naming::WorkspaceName,
    workspace_dir, workspaces_root,
};
use chrono::Utc;
use std::path::{Path, PathBuf};

/// The contract file: `<base>/workspaces/<name>/workspace.json`.
pub fn manifest_path(base: &Path, name: &WorkspaceName) -> PathBuf {
    workspace_dir(base, name).join(WORKSPACE_FILE)
}

/// The account journal: `<base>/workspaces/<name>/journal.jsonl`.
pub fn journal_path(base: &Path, name: &WorkspaceName) -> PathBuf {
    workspace_dir(base, name).join(crate::manifest::JOURNAL_FILE)
}

/// Writes a contract once, stamping storage-owned time and version.
pub(crate) fn create(base: &Path, spec: &CreateSpec) -> Result<WorkspaceManifest> {
    let dir = workspace_dir(base, &spec.name);
    if dir.exists() {
        return Err(WorkspaceError::Exists(spec.name.to_string()));
    }
    let manifest = WorkspaceManifest {
        workspace_version: WORKSPACE_VERSION.to_string(),
        // Workspace-unified versions make this the CLI version.
        cli_version: env!("CARGO_PKG_VERSION").to_string(),
        name: spec.name.to_string(),
        capital: spec.capital,
        currency: spec.currency.clone(),
        mode: spec.mode,
        fee_rate: spec.fee_rate,
        slippage_rate: spec.slippage_rate,
        allowed_pairs: spec.allowed_pairs.clone(),
        created_at: Utc::now(),
    };
    kraken_recording::write_json_atomic(&manifest_path(base, &spec.name), &manifest)?;
    Ok(manifest)
}

/// Loads a contract, preserving distinct routing for absence, damage, and
/// version mismatch.
pub(crate) fn load(base: &Path, name: &WorkspaceName) -> Result<WorkspaceManifest> {
    let path = manifest_path(base, name);
    let data = match std::fs::read_to_string(&path) {
        Ok(data) => data,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(WorkspaceError::NotFound(name.to_string()));
        }
        Err(err) => return Err(err.into()),
    };
    let manifest: WorkspaceManifest =
        serde_json::from_str(&data).map_err(|err| WorkspaceError::Damaged {
            name: name.to_string(),
            what: err.to_string(),
        })?;
    if !is_compatible(&manifest.workspace_version) {
        return Err(WorkspaceError::Incompatible {
            found: manifest.workspace_version,
            expected: WORKSPACE_VERSION.to_string(),
        });
    }
    if manifest.name != name.as_ref() {
        return Err(WorkspaceError::Damaged {
            name: name.to_string(),
            what: format!("contract names workspace '{}'", manifest.name),
        });
    }
    Ok(manifest)
}

/// Remove a just-created contract after a failed funding step, so `create`
/// never leaves a workspace that exists but holds no account. Best-effort:
/// the funding error is the one the caller surfaces.
pub(crate) fn discard_created(base: &Path, name: &WorkspaceName) {
    let dir = workspace_dir(base, name);
    if let Err(err) = std::fs::remove_file(manifest_path(base, name)) {
        tracing::warn!(workspace = %name, %err, "could not discard the contract of a failed create");
    }
    if let Err(err) = std::fs::remove_dir(&dir) {
        tracing::warn!(workspace = %name, %err, "could not remove the dir of a failed create");
    }
}

/// The gate for scoping into a named workspace: a typo, a never-created name,
/// or a damaged contract must refuse rather than silently trade the wrong
/// account.
pub(crate) fn ensure_exists(base: &Path, name: &WorkspaceName) -> Result<()> {
    load(base, name).map(drop)
}

/// Every named workspace, sorted by name. A damaged contract is a described
/// row — one broken workspace cannot hide the others.
pub(crate) fn list(base: &Path) -> Result<Vec<(WorkspaceName, Result<WorkspaceManifest>)>> {
    let entries = match std::fs::read_dir(workspaces_root(base)) {
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
        // Only valid names can be addressed by --workspace; foreign dirs are
        // not workspaces.
        let raw = entry.file_name();
        let Ok(name) = raw.to_string_lossy().parse::<WorkspaceName>() else {
            continue;
        };
        let manifest = load(base, &name);
        rows.push((name, manifest));
    }
    rows.sort_unstable_by(|a, b| a.0.as_ref().cmp(b.0.as_ref()));
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::WorkspaceMode;
    use rust_decimal_macros::dec;

    fn spec(name: &str) -> CreateSpec {
        CreateSpec {
            name: name.parse().expect("valid name"),
            capital: dec!(10000),
            currency: "USD".to_string(),
            mode: WorkspaceMode::Paper,
            fee_rate: dec!(0.0026),
            slippage_rate: dec!(0),
            allowed_pairs: None,
        }
    }

    #[test]
    fn unknown_workspace_is_not_found_with_the_create_hint() {
        let base = tempfile::tempdir().expect("tempdir");
        let name: WorkspaceName = "ghost".parse().expect("valid name");
        let err = ensure_exists(base.path(), &name).expect_err("must refuse");
        assert_eq!(err.category(), "validation");
        assert!(
            err.to_string()
                .contains("kraken workspace create ghost --capital"),
            "{err}"
        );
    }

    #[test]
    fn create_refuses_a_second_workspace_of_the_same_name() {
        let base = tempfile::tempdir().expect("tempdir");
        create(base.path(), &spec("btc")).expect("first create");
        let err = create(base.path(), &spec("btc")).expect_err("must refuse");
        assert!(matches!(err, WorkspaceError::Exists(_)), "{err}");
    }

    #[test]
    fn damaged_manifest_fails_the_existence_gate_loud() {
        let base = tempfile::tempdir().expect("tempdir");
        let name: WorkspaceName = "broken".parse().expect("valid name");
        std::fs::create_dir_all(workspace_dir(base.path(), &name)).expect("mkdir");
        std::fs::write(manifest_path(base.path(), &name), "not json").expect("write");
        let err = ensure_exists(base.path(), &name).expect_err("must refuse");
        assert_eq!(err.category(), "parse");
    }

    #[test]
    fn newer_major_manifest_is_incompatible() {
        let base = tempfile::tempdir().expect("tempdir");
        let name: WorkspaceName = "future".parse().expect("valid name");
        let manifest = create(base.path(), &spec("future")).expect("create");
        let mut raw = serde_json::to_value(&manifest).expect("to json");
        raw["workspace_version"] = "2.0".into();
        kraken_recording::write_json_atomic(&manifest_path(base.path(), &name), &raw)
            .expect("rewrite");
        let err = load(base.path(), &name).expect_err("must refuse");
        assert_eq!(err.category(), "config");
    }

    #[test]
    fn contract_naming_another_workspace_is_damaged() {
        let base = tempfile::tempdir().expect("tempdir");
        create(base.path(), &spec("real")).expect("create");
        let real: WorkspaceName = "real".parse().expect("valid name");
        let copied: WorkspaceName = "copied".parse().expect("valid name");
        std::fs::create_dir_all(workspace_dir(base.path(), &copied)).expect("mkdir");
        std::fs::copy(
            manifest_path(base.path(), &real),
            manifest_path(base.path(), &copied),
        )
        .expect("copy");
        let err = load(base.path(), &copied).expect_err("must refuse");
        assert_eq!(err.category(), "parse");
    }

    #[test]
    fn unknown_manifest_keys_are_tolerated() {
        let base = tempfile::tempdir().expect("tempdir");
        let name: WorkspaceName = "fwd".parse().expect("valid name");
        let manifest = create(base.path(), &spec("fwd")).expect("create");
        let mut raw = serde_json::to_value(&manifest).expect("to json");
        raw["credential_profile"] = "future-profile".into();
        kraken_recording::write_json_atomic(&manifest_path(base.path(), &name), &raw)
            .expect("rewrite");
        assert!(load(base.path(), &name).is_ok());
    }

    #[test]
    fn one_damaged_workspace_never_hides_the_others() {
        let base = tempfile::tempdir().expect("tempdir");
        create(base.path(), &spec("healthy")).expect("create");
        let broken: WorkspaceName = "broken".parse().expect("valid name");
        std::fs::create_dir_all(workspace_dir(base.path(), &broken)).expect("mkdir");
        std::fs::write(manifest_path(base.path(), &broken), "not json").expect("write");

        let rows = list(base.path()).expect("list");
        assert_eq!(rows.len(), 2);
        assert!(rows[0].1.is_err(), "broken sorts first and is described");
        assert!(rows[1].1.is_ok());
    }
}
