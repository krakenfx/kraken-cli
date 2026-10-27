//! Durable workspace account contracts.
//!
//! Additive fields remain forward-compatible; major layout changes do not.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// Manifest schema version; compatibility is MAJOR-only (see
/// [`is_compatible`]).
pub const WORKSPACE_VERSION: &str = "1.0";
/// Named explicitly to distinguish the workspace contract from `session.json`.
pub const WORKSPACE_FILE: &str = "workspace.json";
/// One journal spans both modes, so its name remains mode-neutral.
pub const JOURNAL_FILE: &str = "journal.jsonl";

/// The workspace decision log: `--reason` rationales keyed by order id.
pub const DECISIONS_FILE: &str = "decisions.jsonl";

/// Which backend the account's trading verbs execute against.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case", ascii_case_insensitive)]
pub enum WorkspaceMode {
    /// Simulated fills from the event-sourced engine, live market data.
    Paper,
    /// Real orders attributed to this workspace through the configured account.
    Live,
}

/// The durable description of a workspace account.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    /// Contract schema version; a differing MAJOR is incompatible.
    #[serde(default = "default_workspace_version")]
    pub workspace_version: String,
    /// CLI version that wrote the contract.
    pub cli_version: String,
    /// The workspace name (its directory under `workspaces/`).
    pub name: String,
    /// Starting capital. In live mode this is the allocation: the CLI refuses
    /// spending beyond what the allocation ledger holds (never the order that
    /// pays the allocated asset back in), but the venue enforces none of it.
    /// Digit-exact on the wire.
    #[serde(with = "rust_decimal::serde::str")]
    pub capital: Decimal,
    /// Quote currency the capital is denominated in.
    pub currency: String,
    pub mode: WorkspaceMode,
    #[serde(with = "rust_decimal::serde::str")]
    pub fee_rate: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub slippage_rate: Decimal,
    /// Account permission policy: `None` = unrestricted, empty = deny-all
    /// (the paper analogue of live API-key pair permissions).
    pub allowed_pairs: Option<Vec<String>>,
    /// Instant the account was created and funded.
    pub created_at: DateTime<Utc>,
}

fn default_workspace_version() -> String {
    WORKSPACE_VERSION.to_string()
}

/// MAJOR-only compatibility: additive fields never bump it; a layout change
/// this build cannot read does.
pub fn is_compatible(found: &str) -> bool {
    fn major(version: &str) -> Option<&str> {
        version.split('.').next()
    }
    major(found) == major(WORKSPACE_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_is_major_only() {
        assert!(is_compatible("1.0"));
        assert!(is_compatible("1.7"));
        assert!(!is_compatible("2.0"));
    }
}
