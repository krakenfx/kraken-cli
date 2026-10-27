//! Append-only rationale records for session actions.
//!
//! A short writer lock preserves timestamp order for later P&L attribution.

use crate::Result;
use chrono::{DateTime, Utc};
use kraken_recording::{JsonlSink, Sink};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::path::Path;

/// One entry in a session's decision log.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    /// Instant the decision was recorded.
    pub timestamp: DateTime<Utc>,
    /// What kind of decision this is.
    pub kind: DecisionKind,
    /// The trading pair the decision concerns, if any (a `skip`/`alert` may
    /// have none). Canonicalized on construction (uppercase, `/` stripped) so
    /// one instrument keys identically no matter which writer logged it.
    pub symbol: Option<String>,
    /// Free-text rationale ("the why").
    pub reason: String,
    /// A paper order this decision produced or references, if any.
    pub order_id: Option<String>,
}

/// The kind of a logged decision.
///
/// Matches `playground note --kind`: `buy`, `sell`, `skip`,
/// `alert`. The `clap::ValueEnum` derive rides the optional `clap` feature —
/// front-ends turn it on; the crate itself never needs clap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum DecisionKind {
    /// A decision to buy.
    Buy,
    /// A decision to sell.
    Sell,
    /// A decision to pull a resting order.
    Cancel,
    /// A deliberate no-op (considered and declined to act).
    Skip,
    /// A noteworthy observation with no trade.
    Alert,
    /// A newer build's kind. Never constructed by writers (nor offered as a
    /// `--kind` value); the timeline reader skips it.
    #[serde(other)]
    #[cfg_attr(feature = "clap", value(skip))]
    Unknown,
}

/// Uses the REST form because slash removal is deterministic but insertion is
/// not.
fn canonical_symbol(symbol: &str) -> String {
    symbol.replace('/', "").to_uppercase()
}

/// Appends under lock, stamping after acquisition to preserve timestamp order.
pub fn append(
    path: &Path,
    kind: DecisionKind,
    symbol: Option<String>,
    reason: String,
    order_id: Option<String>,
) -> Result<Decision> {
    let mut sink = JsonlSink::create(path.to_path_buf(), "session decision log")?;
    let decision = Decision {
        timestamp: Utc::now(),
        kind,
        symbol: symbol.as_deref().map(canonical_symbol),
        reason,
        order_id,
    };
    sink.record(std::slice::from_ref(&decision))?;
    Ok(decision)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_render_as_note_flag_values() {
        for (kind, expected) in [
            (DecisionKind::Buy, "\"buy\""),
            (DecisionKind::Sell, "\"sell\""),
            (DecisionKind::Cancel, "\"cancel\""),
            (DecisionKind::Skip, "\"skip\""),
            (DecisionKind::Alert, "\"alert\""),
        ] {
            assert_eq!(serde_json::to_string(&kind).unwrap(), expected);
        }
    }

    #[test]
    fn unknown_kind_is_the_deserialization_fallback() {
        // The timeline reader's skip depends on newer kinds parsing as Unknown.
        assert_eq!(
            serde_json::from_str::<DecisionKind>("\"hold\"").unwrap(),
            DecisionKind::Unknown
        );
    }

    #[test]
    fn busy_decision_log_rejects_the_append() {
        // The conflict must surface as an Err — `paper --reason` warns and
        // continues, `playground note` propagates; both need it reported.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("decisions.jsonl");
        let _held = kraken_recording::FileLock::acquire(&path, "test holder").unwrap();

        let err = append(&path, DecisionKind::Buy, None, "dip".into(), None).unwrap_err();
        // The lock refusal is the store's, surfaced transparently — the
        // binary's bridge lands `Recording(Rejected)` on `validation`.
        assert!(matches!(
            err,
            crate::SessionError::Recording(kraken_recording::Error::Rejected(_))
        ));
        assert!(err.to_string().contains("in use"), "got: {err}");
    }

    #[test]
    fn append_is_append_only_across_calls() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("decisions.jsonl");
        let first = append(
            &path,
            DecisionKind::Buy,
            Some("BTC/USD".into()),
            "dip".into(),
            None,
        )
        .unwrap();
        let second = append(
            &path,
            DecisionKind::Skip,
            None,
            "spread too wide".into(),
            None,
        )
        .unwrap();

        let lines: Vec<Decision> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines, [first, second]);
    }

    #[test]
    fn symbol_forms_converge_to_one_canonical_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("decisions.jsonl");
        for input in ["ETH/USD", "ETHUSD", "ethusd", "eth/usd"] {
            let d = append(
                &path,
                DecisionKind::Skip,
                Some(input.into()),
                "why".into(),
                None,
            )
            .unwrap();
            assert_eq!(d.symbol.as_deref(), Some("ETHUSD"), "input {input:?}");
        }
    }

    #[test]
    fn optional_refs_are_omitted_when_absent() {
        let skip = Decision {
            timestamp: "2026-07-02T10:00:00Z".parse().unwrap(),
            kind: DecisionKind::Skip,
            symbol: None,
            reason: "spread too wide".to_string(),
            order_id: None,
        };
        let json = serde_json::to_string(&skip).unwrap();
        assert!(!json.contains("symbol"));
        assert!(!json.contains("order_id"));
    }
}
