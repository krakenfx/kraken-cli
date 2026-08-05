//! Mechanical paper-to-live promotion checks.
//!
//! Scoped credentials remain a hard blocker; refusals return the full evidence
//! checklist.

use serde::Serialize;

use crate::{WorkspaceManifest, WorkspaceMode};

/// One experiment's evidence, as `lab compare` graded it.
#[derive(Debug, Clone, Serialize)]
pub struct ExperimentEvidence {
    pub experiment: String,
    pub total_sessions: usize,
    /// Runs whose sealed criteria passed mechanically.
    pub passing_sessions: usize,
    /// Passing runs that traded the live market — replay passes count only
    /// as screening, never as the promotion-carrying evidence.
    pub passing_live_sessions: usize,
}

impl ExperimentEvidence {
    /// Per experiment: at least two passes with a live
    /// pass among them. Two replay passes never qualify.
    fn qualifies(&self) -> bool {
        self.passing_sessions >= 2 && self.passing_live_sessions >= 1
    }
}

/// One line of the checklist: a criterion, whether the evidence satisfies
/// it, and the observation behind the verdict.
#[derive(Debug, Clone, Serialize)]
pub struct ChecklistItem {
    pub criterion: String,
    pub satisfied: bool,
    pub detail: String,
}

/// What the binary observed on disk; [`evaluate`] never reads anything.
#[derive(Debug, Clone, Default)]
pub struct PromotionInputs {
    pub experiments: Vec<ExperimentEvidence>,
    /// Fills inside stopped windows without a decision-log entry.
    pub manual_trades_in_windows: usize,
}

/// The full evidence picture a promotion decision reads.
#[derive(Debug, Clone, Serialize)]
pub struct PromotionChecklist {
    pub workspace: String,
    pub mode: WorkspaceMode,
    /// False until every criterion holds and no hard blocker remains.
    pub promotable: bool,
    pub criteria: Vec<ChecklistItem>,
    pub experiments: Vec<ExperimentEvidence>,
    pub manual_trades_in_windows: usize,
    /// What no amount of evidence can satisfy yet.
    pub blockers: Vec<String>,
}

/// Grade the evidence against the frozen gate. Pure — the caller gathers.
pub fn evaluate(manifest: &WorkspaceManifest, inputs: PromotionInputs) -> PromotionChecklist {
    let PromotionInputs {
        experiments,
        manual_trades_in_windows,
    } = inputs;

    let sealed = !experiments.is_empty();
    let best_passes = experiments
        .iter()
        .map(|e| e.passing_sessions)
        .max()
        .unwrap_or(0);
    let qualifying = experiments.iter().find(|e| e.qualifies());

    let criteria = vec![
        ChecklistItem {
            criterion: "a sealed experiment exists".to_string(),
            satisfied: sealed,
            detail: if sealed {
                format!("{} sealed", experiments.len())
            } else {
                "none — freeze one with 'kraken lab new'".to_string()
            },
        },
        ChecklistItem {
            criterion: "an experiment has at least 2 passing sessions".to_string(),
            satisfied: best_passes >= 2,
            detail: format!("best experiment has {best_passes} passing session(s)"),
        },
        ChecklistItem {
            criterion: "its passes include a live session".to_string(),
            satisfied: qualifying.is_some(),
            detail: match qualifying {
                Some(evidence) => format!(
                    "'{}': {} passes, {} live",
                    evidence.experiment, evidence.passing_sessions, evidence.passing_live_sessions
                ),
                None => {
                    "no experiment pairs 2 passes with a live pass — replay alone never promotes"
                        .to_string()
                }
            },
        },
        ChecklistItem {
            criterion: "manual trades inside session windows are disclosed".to_string(),
            // The gate requires disclosure, not zero manual activity.
            satisfied: true,
            detail: format!("{manual_trades_in_windows} manual trade(s) in stopped windows"),
        },
        ChecklistItem {
            criterion: "live credentials scoped to this workspace".to_string(),
            satisfied: false,
            detail: "workspace-scoped credentials are not configured".to_string(),
        },
    ];

    let blockers = vec![
        "live execution needs scoped credentials; until then promotion is \
         evaluated but never performed"
            .to_string(),
    ];

    PromotionChecklist {
        workspace: manifest.name.clone(),
        mode: manifest.mode,
        promotable: criteria.iter().all(|c| c.satisfied) && blockers.is_empty(),
        criteria,
        experiments,
        manual_trades_in_windows,
        blockers,
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    fn manifest() -> WorkspaceManifest {
        WorkspaceManifest {
            workspace_version: crate::WORKSPACE_VERSION.to_string(),
            cli_version: "0.0.0-test".to_string(),
            name: "btc-momentum".to_string(),
            capital: dec!(10_000),
            currency: "USD".to_string(),
            mode: WorkspaceMode::Paper,
            fee_rate: dec!(0.0026),
            slippage_rate: dec!(0),
            allowed_pairs: None,
            created_at: "2026-01-01T00:00:00Z".parse().unwrap(),
        }
    }

    fn evidence(passing: usize, live: usize) -> ExperimentEvidence {
        ExperimentEvidence {
            experiment: "m1".to_string(),
            total_sessions: passing,
            passing_sessions: passing,
            passing_live_sessions: live,
        }
    }

    #[test]
    fn promotion_checklist_requires_a_passing_live_session() {
        let checklist = evaluate(
            &manifest(),
            PromotionInputs {
                experiments: vec![evidence(3, 1)],
                manual_trades_in_windows: 0,
            },
        );
        let live_item = &checklist.criteria[2];
        assert!(live_item.satisfied, "{live_item:?}");
        assert!(
            !checklist.promotable,
            "the credentials blocker holds regardless of evidence"
        );
    }

    #[test]
    fn two_replay_passes_do_not_promote() {
        let checklist = evaluate(
            &manifest(),
            PromotionInputs {
                experiments: vec![evidence(2, 0)],
                manual_trades_in_windows: 0,
            },
        );
        assert!(checklist.criteria[1].satisfied, "two passes exist");
        assert!(
            !checklist.criteria[2].satisfied,
            "without a live pass the gate must hold: {:?}",
            checklist.criteria[2]
        );
    }

    #[test]
    fn manual_trades_inside_windows_are_disclosed_not_hidden() {
        let checklist = evaluate(
            &manifest(),
            PromotionInputs {
                experiments: vec![evidence(2, 1)],
                manual_trades_in_windows: 3,
            },
        );
        assert_eq!(checklist.manual_trades_in_windows, 3);
        let disclosure = &checklist.criteria[3];
        assert!(disclosure.satisfied, "disclosure is the requirement");
        assert!(disclosure.detail.contains('3'), "{disclosure:?}");
    }

    #[test]
    fn no_experiments_fail_the_first_criterion_with_the_freeze_hint() {
        let checklist = evaluate(&manifest(), PromotionInputs::default());
        assert!(!checklist.criteria[0].satisfied);
        assert!(checklist.criteria[0].detail.contains("kraken lab new"));
        assert!(!checklist.promotable);
    }
}
