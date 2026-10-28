//! Workspace totals assembled from authoritative session outcomes.
//!
//! Unscored sessions remain error rows rather than contributing invented zeros.

use crate::{
    WorkspaceManifest, WorkspaceMode,
    session::{DriftLine, SessionStatus},
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_with::skip_serializing_none;

/// One session's line of the report. Money fields are `None` exactly when
/// `error` says why — a row never carries invented numbers.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct SessionRow {
    pub session: String,
    pub label: Option<String>,
    pub experiment: Option<String>,
    pub status: SessionStatus,
    /// The scorecard anchor, verbatim: `starting_balance` / `final_value`.
    #[serde(with = "rust_decimal::serde::str_option")]
    pub opening_equity: Option<Decimal>,
    #[serde(with = "rust_decimal::serde::str_option")]
    pub closing_equity: Option<Decimal>,
    #[serde(with = "rust_decimal::serde::str_option")]
    pub pnl: Option<Decimal>,
    pub fills: Option<usize>,
    /// Fills inside the window without a decision-log entry.
    pub manual_trades: Option<usize>,
    /// The mechanical verdict, when the session is stamped with an experiment.
    pub verdict: Option<bool>,
    /// Why this row carries no numbers.
    pub error: Option<String>,
}

/// Trading outside any stopped session window — disclosed, never summed into a
/// run.
#[derive(Debug, Clone, Serialize)]
pub struct ManualSegment {
    pub fills: usize,
}

/// What the binary observed; [`assemble`] only sums and discloses.
#[derive(Debug, Clone, Default)]
pub struct ReportInputs {
    pub sessions: Vec<SessionRow>,
    /// The run still recording, excluded from every sum.
    pub running: Option<String>,
    pub manual_fills: usize,
    pub reset_epochs: usize,
    /// When this workspace's fills stopped being simulated, from the journal's
    /// `fill_source_changed` marker. `None` for a workspace that never
    /// switched — a live-from-birth one included, since it has no paper era.
    pub live_since: Option<DateTime<Utc>>,
    /// Current equity as the caller valued it (the crate stays
    /// network-free): `(value, complete)`.
    pub equity: Option<(Decimal, bool)>,
    /// Folded balances vs the last `Reconciled` snapshot (live journals).
    pub drift: Option<Vec<DriftLine>>,
}

/// The whole picture, JSON payload = this type verbatim.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceReport {
    pub workspace: String,
    pub mode: WorkspaceMode,
    pub currency: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub capital: Decimal,
    #[serde(with = "rust_decimal::serde::str_option")]
    pub equity: Option<Decimal>,
    pub valuation_complete: bool,
    /// The Decimal sum of the emitted run rows' `pnl` — nothing else.
    #[serde(with = "rust_decimal::serde::str")]
    pub sessions_pnl: Decimal,
    pub reset_epochs: usize,
    /// When simulated history ends and real history begins, for a workspace
    /// that was promoted rather than created live.
    pub live_since: Option<DateTime<Utc>>,
    pub sessions: Vec<SessionRow>,
    pub manual: ManualSegment,
    /// Disclosed AND excluded from every sum.
    pub running: Option<String>,
    pub drift: Option<Vec<DriftLine>>,
    pub caveats: Vec<String>,
}

/// The one place sums happen. Totals are Decimal sums of the rows this
/// report emits; error rows contribute a caveat instead of a number.
pub fn assemble(manifest: &WorkspaceManifest, inputs: ReportInputs) -> WorkspaceReport {
    let ReportInputs {
        sessions,
        running,
        manual_fills,
        reset_epochs,
        live_since,
        equity,
        drift,
    } = inputs;

    let sessions_pnl: Decimal = sessions.iter().filter_map(|row| row.pnl).sum();

    let mut caveats = Vec::new();
    let unscored = sessions.iter().filter(|row| row.error.is_some()).count();
    if unscored > 0 {
        caveats.push(format!(
            "{unscored} session(s) carry no numbers (see their error field); sessions_pnl sums \
             only the scored rows"
        ));
    }
    if let Some(run) = &running {
        caveats.push(format!(
            "session {run} is still recording; it is excluded from every sum"
        ));
    }
    if manual_fills > 0 {
        caveats.push(format!(
            "{manual_fills} fill(s) happened outside any stopped session window"
        ));
    }
    if reset_epochs > 0 {
        caveats.push(format!(
            "{reset_epochs} reset epoch(s): equity is measured from the newest epoch's capital"
        ));
    }
    // Without this the promotion's epoch is indistinguishable from an ordinary
    // reset, and a reader cannot tell which half of the journal was simulated.
    if let Some(at) = live_since {
        caveats.push(format!(
            "fills before {at} were simulated; this workspace went live then, and equity is \
             measured from the live epoch's allocation"
        ));
    }
    let (equity, valuation_complete) = match equity {
        Some((value, complete)) => (Some(value), complete),
        None => {
            caveats.push("current equity unavailable (valuation failed)".to_string());
            (None, false)
        }
    };
    if let Some(lines) = &drift
        && !lines.is_empty()
    {
        caveats.push(format!(
            "{} asset(s) drift from the last venue reconciliation",
            lines.len()
        ));
    }

    WorkspaceReport {
        workspace: manifest.name.clone(),
        mode: manifest.mode,
        currency: manifest.currency.clone(),
        capital: manifest.capital,
        equity,
        valuation_complete,
        sessions_pnl,
        reset_epochs,
        live_since,
        sessions,
        manual: ManualSegment {
            fills: manual_fills,
        },
        running,
        drift,
        caveats,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

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

    fn scored_row(run: &str, pnl: Decimal) -> SessionRow {
        SessionRow {
            session: run.to_string(),
            label: None,
            experiment: None,
            status: SessionStatus::Stopped,
            opening_equity: Some(dec!(10_000)),
            closing_equity: Some(dec!(10_000) + pnl),
            pnl: Some(pnl),
            fills: Some(1),
            manual_trades: Some(0),
            verdict: None,
            error: None,
        }
    }

    fn error_row(run: &str) -> SessionRow {
        SessionRow {
            session: run.to_string(),
            label: None,
            experiment: None,
            status: SessionStatus::Aborted,
            opening_equity: None,
            closing_equity: None,
            pnl: None,
            fills: None,
            manual_trades: None,
            verdict: None,
            error: Some(
                "validation: session 's2' has no final summary to anchor on; run 'kraken session \
                 stop' first"
                    .to_string(),
            ),
        }
    }

    /// The report law: the total is the Decimal sum of the emitted rows
    /// — digit-exact through a serde round trip.
    #[test]
    fn report_totals_are_the_decimal_sum_of_emitted_rows() {
        let report = assemble(
            &manifest(),
            ReportInputs {
                sessions: vec![
                    scored_row("r1", dec!(-0.1675346)),
                    scored_row("r3", dec!(0.0852500)),
                ],
                equity: Some((dec!(9_999.9177154), true)),
                ..ReportInputs::default()
            },
        );
        assert_eq!(report.sessions_pnl, dec!(-0.0822846));

        let json = serde_json::to_value(&report).unwrap();
        let summed: Decimal = json["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["pnl"].as_str().unwrap().parse::<Decimal>().unwrap())
            .sum();
        assert_eq!(
            json["sessions_pnl"].as_str().unwrap(),
            summed.to_string(),
            "the wire total re-sums byte-equal from the wire rows"
        );
    }

    #[test]
    fn error_rows_never_fake_a_zero_and_are_caveated() {
        let report = assemble(
            &manifest(),
            ReportInputs {
                sessions: vec![scored_row("r1", dec!(5)), error_row("r2")],
                equity: Some((dec!(10_005), true)),
                ..ReportInputs::default()
            },
        );
        assert_eq!(
            report.sessions_pnl,
            dec!(5),
            "error rows contribute nothing"
        );
        assert!(report.sessions[1].pnl.is_none());
        assert!(
            report
                .caveats
                .iter()
                .any(|c| c.contains("carry no numbers")),
            "{:?}",
            report.caveats
        );
    }

    #[test]
    fn running_session_is_disclosed_and_excluded_from_sums() {
        let report = assemble(
            &manifest(),
            ReportInputs {
                sessions: vec![scored_row("r1", dec!(1))],
                running: Some("r2".to_string()),
                equity: Some((dec!(10_001), true)),
                ..ReportInputs::default()
            },
        );
        assert_eq!(report.running.as_deref(), Some("r2"));
        assert_eq!(report.sessions_pnl, dec!(1));
        assert!(
            report.caveats.iter().any(|c| c.contains("still recording")),
            "{:?}",
            report.caveats
        );
    }

    #[test]
    fn drift_lines_surface_reconciled_deltas() {
        let report = assemble(
            &manifest(),
            ReportInputs {
                equity: Some((dec!(10_000), true)),
                drift: Some(vec![DriftLine {
                    asset: "BTC".to_string(),
                    local: dec!(1),
                    venue: dec!(0.9),
                }]),
                ..ReportInputs::default()
            },
        );
        assert!(
            report.caveats.iter().any(|c| c.contains("drift")),
            "{:?}",
            report.caveats
        );
        assert_eq!(report.drift.as_ref().unwrap()[0].asset, "BTC");
    }

    #[test]
    fn failed_valuation_is_disclosed_never_zeroed() {
        let report = assemble(&manifest(), ReportInputs::default());
        assert!(report.equity.is_none());
        assert!(!report.valuation_complete);
        assert!(
            report
                .caveats
                .iter()
                .any(|c| c.contains("equity unavailable")),
            "{:?}",
            report.caveats
        );
    }
}
