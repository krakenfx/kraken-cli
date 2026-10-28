#![doc = include_str!("../README.md")]

use std::{
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
};

pub mod decision;
pub mod manifest;
pub mod session;
pub mod timeline;

pub type Result<T, E = SessionError> = std::result::Result<T, E>;

/// Session failures aligned with recording categories for stable CLI mapping.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// A session that doesn't exist, is held by a writer, or needs a backend
    /// this build lacks; also an invalid name. → envelope `validation`.
    #[error("{0}")]
    Rejected(String),
    /// A manifest MAJOR this build can't read. → envelope `config`.
    #[error(
        "session manifest version {found} is incompatible with this build (expects {expected})"
    )]
    IncompatibleManifest { found: String, expected: String },
    /// A manifest the schema can't decode. → envelope `parse`.
    #[error("{0}")]
    Damaged(String),
    /// Journal-store failure; the envelope mapping delegates to the recording
    /// bridge (Rejected→validation, Damaged→parse, Io/Engine→io).
    #[error(transparent)]
    Recording(#[from] kraken_recording::Error),
    /// Filesystem failure reading the manifest. → envelope `io`.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The manifest schema version (SemVer; a differing MAJOR is incompatible).
///
/// Owned here, distinct from [`kraken_recording::schema::SCHEMA_VERSION`]: the
/// manifest and the recording format version independently.
pub const MANIFEST_VERSION: &str = "1.0";

/// Whether an on-disk manifest version is compatible with what this build
/// writes — true iff they share a MAJOR component.
///
/// Deliberately mirrors [`kraken_recording::schema::is_compatible`] rather than
/// sharing code: the two version domains evolve independently.
pub fn is_compatible(found: &str) -> bool {
    major(found) == major(MANIFEST_VERSION)
}

/// The MAJOR component of a `MAJOR.MINOR` version string.
fn major(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

/// One validated path segment using the shared artifact-name grammar.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionName(String);

impl FromStr for SessionName {
    type Err = SessionError;

    fn from_str(s: &str) -> Result<Self> {
        // Shared grammar prevents artifact namespaces from diverging.
        if !kraken_core::name::is_safe_name_segment(s) {
            return Err(SessionError::Rejected(format!(
                "Invalid session name {s:?}: use only letters, digits, '.', '-', or '_' (no '/', \
                 spaces, or '.'/'..')."
            )));
        }
        Ok(Self(s.to_owned()))
    }
}

impl TryFrom<&str> for SessionName {
    type Error = SessionError;

    /// See [`SessionName::from_str`].
    fn try_from(s: &str) -> Result<Self> {
        s.parse()
    }
}

impl fmt::Display for SessionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for SessionName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// The root under which all named sessions live: `<base>/sessions/`. `base`
/// is the config dir in production and a temp dir in tests.
pub fn sessions_root(base: &Path) -> PathBuf {
    base.join("sessions")
}

/// A session's directory: `<base>/sessions/<name>/`.
pub fn dir(base: &Path, name: &SessionName) -> PathBuf {
    sessions_root(base).join(name.as_ref())
}

/// The isolated paper account's event log:
/// `<base>/sessions/<name>/events.jsonl`.
pub fn events_path(base: &Path, name: &SessionName) -> PathBuf {
    dir(base, name).join("events.jsonl")
}

/// The session manifest: `<base>/sessions/<name>/manifest.json`.
pub fn manifest_path(base: &Path, name: &SessionName) -> PathBuf {
    dir(base, name).join("manifest.json")
}

/// The decision log: `<base>/sessions/<name>/decisions.jsonl`.
pub fn decisions_path(base: &Path, name: &SessionName) -> PathBuf {
    dir(base, name).join("decisions.jsonl")
}

/// Requires a manifest so decision writers cannot orphan rationale logs.
pub fn ensure_provisioned(base: &Path, name: &SessionName) -> Result<()> {
    if manifest_path(base, name).exists() {
        Ok(())
    } else {
        Err(SessionError::Rejected(format!(
            "session {name} not found; start one with 'kraken session start'"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn session_name_accepts_safe_segments() {
        // Note `..btc` is a literal segment, not the `..` traversal token.
        for ok in ["btc-dip", "s1", "ETH_USD.1", "a.b-c_d", "..btc"] {
            assert!(ok.parse::<SessionName>().is_ok(), "{ok:?} should parse");
        }
    }

    #[test]
    fn session_name_rejects_traversal_and_empty() {
        for bad in [
            "",
            ".",
            "..",
            "...",
            "/",
            "./",
            "../../etc/passwd",
            "a/b",
            "BTC/USD",
            "BTC USD",
            "a\0b",
        ] {
            assert!(bad.parse::<SessionName>().is_err(), "{bad:?} should reject");
        }
    }

    #[test]
    fn resolvers_stay_under_sessions_dir() {
        let base = Path::new("/cfg");
        let name: SessionName = "btc-dip".parse().unwrap();
        assert_eq!(dir(base, &name), base.join("sessions").join("btc-dip"));
        assert!(events_path(base, &name).ends_with("sessions/btc-dip/events.jsonl"));
        assert!(manifest_path(base, &name).ends_with("sessions/btc-dip/manifest.json"));
        assert!(decisions_path(base, &name).ends_with("sessions/btc-dip/decisions.jsonl"));
    }

    #[test]
    fn ensure_provisioned_requires_a_manifest() {
        // A bare `paper --session` account creates the directory without a manifest; a
        // decision must not be written there, so the guard keys off the
        // manifest, not the directory.
        let base = tempfile::tempdir().unwrap();
        let name: SessionName = "btc-dip".parse().unwrap();

        fs::create_dir_all(dir(base.path(), &name)).unwrap();
        assert!(ensure_provisioned(base.path(), &name).is_err());

        fs::write(manifest_path(base.path(), &name), "{}").unwrap();
        assert!(ensure_provisioned(base.path(), &name).is_ok());
    }

    #[test]
    fn version_compatibility_is_by_major() {
        assert!(is_compatible("1.0"));
        assert!(is_compatible("1.7")); // newer minor, same major
        assert!(!is_compatible("2.0"));
        assert!(!is_compatible("0.9"));
    }
}
