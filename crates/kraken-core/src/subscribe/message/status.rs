//! The `status` channel: the engine's identity/health announcement, pushed
//! unsolicited on every (re)connect.
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/status>.

use serde::{Deserialize, Serialize};
use strum::Display;

/// One `status` entry.
///
/// No `deny_unknown_fields`: an extensible inbound payload — a field Kraken
/// adds later is ignored rather than failing the decode, which would drop every
/// frame on the channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusData {
    pub api_version: String,
    pub connection_id: u64,
    pub system: SystemStatus,
    pub version: String,
    /// Wire fields outside the pinned schema, preserved verbatim (see
    /// [`ExtraFields`](super::ExtraFields)).
    #[serde(flatten)]
    pub extra: super::ExtraFields,
}

impl StatusData {
    /// Whether any channel-local enum field decoded through its `Unknown`
    /// fallback.
    pub(crate) fn has_unknown_vocabulary(&self) -> bool {
        self.system == SystemStatus::Unknown
    }
}

/// The trading engine's operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[non_exhaustive]
pub enum SystemStatus {
    Online,
    Maintenance,
    CancelOnly,
    PostOnly,
    LimitOnly,
    /// Vocabulary Kraken adds later; re-serializes as `"unknown"`, not the
    /// original token.
    #[serde(other)]
    Unknown,
}
