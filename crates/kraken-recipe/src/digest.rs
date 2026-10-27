//! Decision-track summary for a recipe draft: counts and cadence only —
//! the free-text reasons are never interpreted.

use chrono::{DateTime, Utc};
use kraken_session::decision::Decision;
use serde::Serialize;
use std::collections::BTreeMap;

/// What the decision track shows without reading minds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DecisionDigest {
    pub total: usize,
    /// Count per decision kind, keyed by wire name (`buy`, `alert`, …).
    pub counts: BTreeMap<String, usize>,
    /// Median gap between consecutive decisions in whole seconds (an even
    /// count averages the two middle gaps); `None` below two decisions.
    pub median_gap_secs: Option<i64>,
}

pub fn digest(decisions: &[Decision]) -> DecisionDigest {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for decision in decisions {
        *counts.entry(decision.kind.to_string()).or_default() += 1;
    }
    DecisionDigest {
        total: decisions.len(),
        counts,
        median_gap_secs: median_gap(decisions),
    }
}

fn median_gap(decisions: &[Decision]) -> Option<i64> {
    // Decision logs are append-ordered, but a digest must not trust that:
    // a clock step backwards would otherwise produce negative gaps.
    let mut instants: Vec<DateTime<Utc>> = decisions.iter().map(|d| d.timestamp).collect();
    instants.sort_unstable();
    let mut gaps: Vec<i64> = instants
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).num_seconds())
        .collect();
    if gaps.is_empty() {
        return None;
    }
    gaps.sort_unstable();
    let mid = gaps.len() / 2;
    if gaps.len() % 2 == 1 {
        Some(gaps[mid])
    } else {
        Some((gaps[mid - 1] + gaps[mid]) / 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_session::decision::DecisionKind;

    fn decision(at: &str, kind: DecisionKind) -> Decision {
        Decision {
            timestamp: at.parse().expect("valid instant"),
            kind,
            symbol: None,
            reason: "test".to_string(),
            order_id: None,
        }
    }

    #[test]
    fn fewer_than_two_decisions_report_no_cadence() {
        assert_eq!(digest(&[]).median_gap_secs, None);
        let one = digest(&[decision("2026-01-01T00:00:00Z", DecisionKind::Skip)]);
        assert_eq!(one.median_gap_secs, None);
    }

    #[test]
    fn median_gap_survives_out_of_order_timestamps() {
        // 0s, 300s, 400s appended out of order: gaps 300 and 100 → median 200.
        let digest = digest(&[
            decision("2026-01-01T00:05:00Z", DecisionKind::Alert),
            decision("2026-01-01T00:00:00Z", DecisionKind::Alert),
            decision("2026-01-01T00:06:40Z", DecisionKind::Alert),
        ]);
        assert_eq!(digest.median_gap_secs, Some(200));
    }

    #[test]
    fn odd_gap_count_takes_the_middle_gap() {
        // Gaps 60, 120, 600 → median 120.
        let digest = digest(&[
            decision("2026-01-01T00:00:00Z", DecisionKind::Alert),
            decision("2026-01-01T00:01:00Z", DecisionKind::Alert),
            decision("2026-01-01T00:03:00Z", DecisionKind::Alert),
            decision("2026-01-01T00:13:00Z", DecisionKind::Alert),
        ]);
        assert_eq!(digest.median_gap_secs, Some(120));
    }
}
