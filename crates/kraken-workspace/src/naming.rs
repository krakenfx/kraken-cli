//! The workspace name contract.

use crate::{Result, WorkspaceError};
use std::{fmt, str::FromStr};

/// The reserved implicit workspace: the flat layout under the config dir.
/// It cannot be created, and scoping to it is byte-identical to no scoping
/// at all.
pub const DEFAULT_WORKSPACE: &str = "default";

/// One validated path segment using the shared artifact-name grammar.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceName(String);

impl FromStr for WorkspaceName {
    type Err = WorkspaceError;

    fn from_str(s: &str) -> Result<Self> {
        if !kraken_core::name::is_safe_name_segment(s) {
            return Err(WorkspaceError::Spec(format!(
                "Invalid workspace name {s:?}: use only letters, digits, '.', '-', or '_' (no \
                 '/', spaces, or '.'/'..')."
            )));
        }
        Ok(Self(s.to_owned()))
    }
}

impl TryFrom<&str> for WorkspaceName {
    type Error = WorkspaceError;

    /// See [`WorkspaceName::from_str`].
    fn try_from(s: &str) -> Result<Self> {
        s.parse()
    }
}

impl WorkspaceName {
    /// True for the reserved implicit workspace (no scoping).
    pub fn is_default(&self) -> bool {
        self.0 == DEFAULT_WORKSPACE
    }
}

impl fmt::Display for WorkspaceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for WorkspaceName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_safe_segments_and_rejects_traversal_shapes() {
        for good in ["btc-momentum", "eth.dca_2", "default"] {
            assert!(good.parse::<WorkspaceName>().is_ok(), "{good}");
        }
        for bad in ["", ".", "..", "a/b", "a b", "ünïcode"] {
            assert!(bad.parse::<WorkspaceName>().is_err(), "{bad:?}");
        }
    }
}
