//! Argument values → domain values.
//!
//! The command surface declares its own vocabulary in
//! [`kraken_contract::domain_values`]. These conversions bridge the argument
//! values to the domain types the engine uses.
//!
//! Free functions rather than `From` impls: both sides are foreign to this
//! crate, so the orphan rule forbids the trait. Every match is exhaustive on
//! purpose — a variant added to either side without the other stops compiling
//! here, which is the only thing keeping the two spellings in step.

use kraken_contract::domain_values as arg;

pub(crate) fn workspace_mode(mode: arg::WorkspaceMode) -> kraken_workspace::WorkspaceMode {
    match mode {
        arg::WorkspaceMode::Paper => kraken_workspace::WorkspaceMode::Paper,
        arg::WorkspaceMode::Live => kraken_workspace::WorkspaceMode::Live,
    }
}

pub(crate) fn zone(zone: arg::Zone) -> kraken_session::session::Zone {
    use kraken_session::session::Zone as Domain;
    match zone {
        arg::Zone::Above => Domain::Above,
        arg::Zone::Within => Domain::Within,
        arg::Zone::Below => Domain::Below,
    }
}

pub(crate) fn decision_kind(kind: arg::DecisionKind) -> kraken_session::decision::DecisionKind {
    use kraken_session::decision::DecisionKind as Domain;
    match kind {
        arg::DecisionKind::Buy => Domain::Buy,
        arg::DecisionKind::Sell => Domain::Sell,
        arg::DecisionKind::Cancel => Domain::Cancel,
        arg::DecisionKind::Skip => Domain::Skip,
        arg::DecisionKind::Alert => Domain::Alert,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two spellings of a value must serialize identically, or a proposal
    /// and the record it produces would disagree about the same command.
    #[test]
    fn argument_and_domain_values_share_a_wire_form() {
        for mode in [arg::WorkspaceMode::Paper, arg::WorkspaceMode::Live] {
            let domain = workspace_mode(mode);
            assert_eq!(
                serde_json::to_string(&mode).expect("serialize"),
                serde_json::to_string(&domain).expect("serialize"),
            );
        }
        for kind in [
            arg::DecisionKind::Buy,
            arg::DecisionKind::Sell,
            arg::DecisionKind::Cancel,
            arg::DecisionKind::Skip,
            arg::DecisionKind::Alert,
        ] {
            let domain = decision_kind(kind);
            assert_eq!(
                serde_json::to_string(&kind).expect("serialize"),
                serde_json::to_string(&domain).expect("serialize"),
            );
        }
        for z in [arg::Zone::Above, arg::Zone::Within, arg::Zone::Below] {
            let domain = zone(z);
            assert_eq!(
                serde_json::to_string(&z).expect("serialize"),
                serde_json::to_string(&domain).expect("serialize"),
            );
        }
    }
}
