//! Immutable experiment specifications and mechanical verdicts.
//!
//! [`seal`] hashes canonical JSON before a run; [`verify`] treats later edits
//! as damage. Storage and clocks remain in [`store`](crate::store).

use crate::{LabError, Result, Scorecard, hash::sha256_hex};
use chrono::{DateTime, Utc};
use kraken_session::SessionName;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// A frozen experiment: the spec plus the hash that seals it.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Experiment {
    pub name: String,
    /// The falsifiable claim, in the author's words.
    pub hypothesis: String,
    /// The strategy under test, if one is named.
    pub strategy: Option<String>,
    pub criteria: Criteria,
    /// Time included in the sealed specification.
    pub created_at: DateTime<Utc>,
    /// Optional run plan sealed before results exist.
    pub session_plan: Option<SessionPlan>,
    /// `sha256:<hex>` over the spec's canonical JSON bytes (everything but
    /// this field).
    pub frozen: String,
}

/// Mechanical success thresholds over a [`Scorecard`]. Every populated
/// criterion must pass; an unpopulated one is simply not checked.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Criteria {
    /// Minimum `metrics.return_pct`, in percent.
    pub min_return_pct: Option<Decimal>,
    /// Maximum `metrics.max_drawdown_pct`, in percent.
    pub max_drawdown_pct: Option<Decimal>,
    /// Minimum number of fills — guards against a "pass" earned by never
    /// trading.
    pub min_fills: Option<u64>,
}

impl Criteria {
    fn is_empty(&self) -> bool {
        self.min_return_pct.is_none() && self.max_drawdown_pct.is_none() && self.min_fills.is_none()
    }

    /// Evaluates every configured threshold; missing observations fail closed.
    pub fn evaluate(&self, card: &Scorecard) -> Verdict {
        let mut checks = Vec::new();
        if let Some(threshold) = self.min_return_pct {
            let observed = card.metrics.return_pct;
            checks.push(Check {
                criterion: "min_return_pct",
                threshold,
                observed,
                pass: observed.is_some_and(|value| value >= threshold),
            });
        }
        if let Some(threshold) = self.max_drawdown_pct {
            let observed = card.metrics.max_drawdown_pct;
            checks.push(Check {
                criterion: "max_drawdown_pct",
                threshold,
                observed,
                pass: observed.is_some_and(|value| value <= threshold),
            });
        }
        if let Some(threshold) = self.min_fills {
            let observed = Decimal::from(card.trades.fills as u64);
            checks.push(Check {
                criterion: "min_fills",
                threshold: Decimal::from(threshold),
                observed: Some(observed),
                pass: observed >= Decimal::from(threshold),
            });
        }
        Verdict {
            pass: checks.iter().all(|check| check.pass),
            checks,
        }
    }
}

/// The mechanical outcome: pass iff every check passed.
#[derive(Debug, Serialize)]
pub struct Verdict {
    pub pass: bool,
    pub checks: Vec<Check>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize)]
pub struct Check {
    pub criterion: &'static str,
    pub threshold: Decimal,
    /// `None` when the scorecard could not derive the number — an honest
    /// fail, never treated as met.
    pub observed: Option<Decimal>,
    pub pass: bool,
}

/// Pre-registered runs in execution order, included in the sealed bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionPlan(pub Vec<PlannedSession>);

/// A planned session whose serialized form is part of the permanent seal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlannedSession {
    /// A live paper run over roughly this wall-clock window (e.g. "24h").
    Live { window: String },
    /// Seals the tape hash so a re-recording cannot satisfy the plan.
    Replay { tape: String, content_hash: String },
}

impl PlannedSession {
    /// The human handle `lab next` reports (`live:24h` / `replay:<ref>`).
    pub fn label(&self) -> String {
        match self {
            Self::Live { window } => format!("live:{window}"),
            Self::Replay { tape, .. } => format!("replay:{tape}"),
        }
    }
}

impl SessionPlan {
    /// Returns the hash bound to a replay leg, if present.
    pub fn sealed_replay_hash(&self, tape: &str) -> Option<&str> {
        self.0.iter().find_map(|entry| match entry {
            PlannedSession::Replay {
                tape: sealed,
                content_hash,
            } if sealed == tape => Some(content_hash.as_str()),
            _ => None,
        })
    }
}

/// Canonical hash input: every contract field except `frozen`.
#[skip_serializing_none]
#[derive(Serialize)]
struct SpecView<'a> {
    name: &'a str,
    hypothesis: &'a str,
    strategy: Option<&'a str>,
    criteria: &'a Criteria,
    created_at: &'a DateTime<Utc>,
    // Keeping this optional field last preserves hashes written before it existed.
    session_plan: Option<&'a SessionPlan>,
}

impl<'a> SpecView<'a> {
    fn of(experiment: &'a Experiment) -> Self {
        SpecView {
            name: &experiment.name,
            hypothesis: &experiment.hypothesis,
            strategy: experiment.strategy.as_deref(),
            criteria: &experiment.criteria,
            created_at: &experiment.created_at,
            session_plan: experiment.session_plan.as_ref(),
        }
    }
}

fn spec_hash(view: &SpecView<'_>) -> Result<String> {
    let bytes =
        serde_json::to_vec(view).map_err(|e| LabError::Damaged(format!("unhashable spec: {e}")))?;
    Ok(sha256_hex(&bytes))
}

/// Validates and seals a specification without consulting storage or a clock.
pub(crate) fn seal(
    name: &str,
    hypothesis: &str,
    strategy: Option<String>,
    criteria: Criteria,
    session_plan: Option<SessionPlan>,
    created_at: DateTime<Utc>,
) -> Result<Experiment> {
    // Experiment names later become session names, so both share one grammar.
    if let Err(e) = name.parse::<SessionName>() {
        return Err(LabError::Spec(e.to_string()));
    }
    if hypothesis.trim().is_empty() {
        return Err(LabError::Spec(
            "an experiment needs a hypothesis; pass --hypothesis".into(),
        ));
    }
    if criteria.is_empty() {
        return Err(LabError::Spec(
            "an experiment needs at least one success criterion; pass --min-return-pct, \
             --max-drawdown-pct, or --min-fills"
                .into(),
        ));
    }
    if let Some(plan) = &session_plan
        && plan.0.is_empty()
    {
        return Err(LabError::Spec(
            "a session plan needs at least one --session entry (live:<window> or replay:<ref>)"
                .into(),
        ));
    }
    let frozen = spec_hash(&SpecView {
        name,
        hypothesis,
        strategy: strategy.as_deref(),
        criteria: &criteria,
        created_at: &created_at,
        session_plan: session_plan.as_ref(),
    })?;
    Ok(Experiment {
        name: name.to_string(),
        hypothesis: hypothesis.to_string(),
        strategy,
        criteria,
        created_at,
        session_plan,
        frozen,
    })
}

/// Recompute the seal over a loaded spec and bind it to the name it was
/// requested under. A hash mismatch means the spec was edited after freezing;
/// a name mismatch means it was renamed (or resolved on a case-insensitive
/// filesystem) — both are [`LabError::Damaged`], so discovery can never key
/// off a tampered or foreign identity.
pub(crate) fn verify(experiment: &Experiment, requested_name: &str) -> Result<()> {
    let expected = spec_hash(&SpecView::of(experiment))?;
    if experiment.frozen != expected {
        return Err(LabError::Damaged(format!(
            "experiment '{requested_name}' does not match its frozen hash; the spec was modified \
             after freezing (recorded {}, recomputed {expected})",
            experiment.frozen
        )));
    }
    if experiment.name != requested_name {
        return Err(LabError::Damaged(format!(
            "experiment '{requested_name}' resolves to a spec sealed under the name '{}'; a \
             frozen spec's filename must match its sealed name (names are case-sensitive)",
            experiment.name
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn criteria() -> Criteria {
        Criteria {
            min_return_pct: Some(dec!(1.5)),
            max_drawdown_pct: None,
            min_fills: Some(3),
        }
    }

    fn plan() -> SessionPlan {
        SessionPlan(vec![
            PlannedSession::Replay {
                tape: "tape:jun-crash".to_string(),
                content_hash: "sha256:abc".to_string(),
            },
            PlannedSession::Live {
                window: "24h".to_string(),
            },
        ])
    }

    #[test]
    fn seal_then_verify_round_trips_and_tamper_reads_as_damage() {
        let created_at = "2026-01-01T00:00:00Z".parse().unwrap();
        let mut sealed = seal("m1", "h", None, criteria(), None, created_at).unwrap();
        assert!(sealed.frozen.starts_with("sha256:"));
        verify(&sealed, "m1").unwrap();

        // Move a goalpost by hand: the recomputed hash no longer matches.
        sealed.criteria.min_return_pct = Some(dec!(0.0));
        let err = verify(&sealed, "m1").unwrap_err();
        assert!(matches!(err, LabError::Damaged(_)), "got: {err}");
        assert!(err.to_string().contains("modified after"), "got: {err}");
    }

    #[test]
    fn verify_binds_the_sealed_name_to_the_requested_one() {
        let created_at = "2026-01-01T00:00:00Z".parse().unwrap();
        let sealed = seal("m1", "h", None, criteria(), None, created_at).unwrap();
        let err = verify(&sealed, "m2").unwrap_err();
        assert!(matches!(err, LabError::Damaged(_)), "got: {err}");
        assert!(
            err.to_string().contains("sealed under the name 'm1'"),
            "got: {err}"
        );
    }

    #[test]
    fn seal_gates_reject_empty_criteria_bad_names_and_blank_hypotheses() {
        let created_at = "2026-01-01T00:00:00Z".parse().unwrap();
        for (name, hypothesis, criteria) in [
            ("m1", "h", Criteria::default()),
            ("bad/name", "h", criteria()),
            ("m1", "   ", criteria()),
        ] {
            let err = seal(name, hypothesis, None, criteria, None, created_at).unwrap_err();
            assert!(matches!(err, LabError::Spec(_)), "got: {err}");
        }
    }

    #[test]
    fn seal_rejects_an_empty_run_plan() {
        let created_at = "2026-01-01T00:00:00Z".parse().unwrap();
        let err = seal(
            "m1",
            "h",
            None,
            criteria(),
            Some(SessionPlan(Vec::new())),
            created_at,
        )
        .unwrap_err();
        assert!(matches!(err, LabError::Spec(_)), "got: {err}");
        assert!(err.to_string().contains("--session entry"), "got: {err}");
    }

    #[test]
    fn planned_run_serializes_with_the_pinned_kind_tag() {
        // This exact JSON is hashed into seals — a repr change would orphan
        // every frozen spec that carries a plan.
        let json = serde_json::to_value(plan()).unwrap();
        let pinned = serde_json::json!([
            {"kind": "replay", "tape": "tape:jun-crash", "content_hash": "sha256:abc"},
            {"kind": "live", "window": "24h"}
        ]);
        assert_eq!(json, pinned);
        let back: SessionPlan = serde_json::from_value(json).unwrap();
        assert_eq!(back, plan());
    }

    #[test]
    fn sealed_replay_hash_finds_the_leg_by_tape_ref() {
        let plan = plan();
        assert_eq!(
            plan.sealed_replay_hash("tape:jun-crash"),
            Some("sha256:abc")
        );
        assert_eq!(plan.sealed_replay_hash("tape:other"), None);
        // A live leg has no sealed tape, so its window never resolves a hash.
        assert_eq!(plan.sealed_replay_hash("24h"), None);
    }

    /// Golden hash pin: the canonical spec bytes are a contract — a serde
    /// rename, field reorder, or representation change breaks every frozen
    /// experiment on disk, and must fail here first.
    #[test]
    fn spec_hash_is_pinned_for_a_golden_spec() {
        let criteria = Criteria {
            min_return_pct: Some(dec!(1.5)),
            max_drawdown_pct: None,
            min_fills: Some(3),
        };
        let created_at = "2026-01-01T00:00:00Z".parse().unwrap();
        let view = SpecView {
            name: "golden",
            hypothesis: "the golden hypothesis",
            strategy: Some("recipe-golden"),
            criteria: &criteria,
            created_at: &created_at,
            session_plan: None,
        };
        let bytes = serde_json::to_string(&view).unwrap();
        assert_eq!(
            bytes,
            r#"{"name":"golden","hypothesis":"the golden hypothesis","strategy":"recipe-golden","criteria":{"min_return_pct":1.5,"min_fills":3},"created_at":"2026-01-01T00:00:00Z"}"#
        );
        assert_eq!(
            spec_hash(&view).unwrap(),
            "sha256:243b1f1ab33343cd33f2245d43b54fec4d8d572438898af2982e4ac3aac8115f"
        );
    }
}
