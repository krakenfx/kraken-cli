//! Account permission policy, enforced locally before orders reach a backend.

use crate::{
    Result, WorkspaceError,
    manifest::{WorkspaceManifest, WorkspaceMode},
};

/// Canonicalize an allow-list: every entry through the engine's pair grammar
/// (so `BTC/USD`, `btcusd`, and `XBTUSD` are one entry), deduped and sorted.
/// An explicit empty list stays empty — that is deny-all, not unrestricted.
pub fn canonicalize_pairs(pairs: &[String]) -> Result<Vec<String>> {
    let mut canonical = pairs
        .iter()
        .map(|pair| canonical_pair(pair))
        .collect::<Result<Vec<_>>>()?;
    canonical.sort_unstable();
    canonical.dedup();
    Ok(canonical)
}

/// One pair in policy form (`BASE/QUOTE`), via the engine's grammar so venue
/// aliases collapse (XBT → BTC) and the check can never disagree with fills.
pub fn canonical_pair(pair: &str) -> Result<String> {
    let (_, base, quote) = kraken_paper::parse_pair(pair)?;
    Ok(format!("{base}/{quote}"))
}

/// Pair permission over a policy-form pair. `None` = unrestricted; an
/// explicit empty list = deny-all.
pub fn ensure_pair_allowed(manifest: &WorkspaceManifest, canonical_pair: &str) -> Result<()> {
    let Some(allowed) = &manifest.allowed_pairs else {
        return Ok(());
    };
    if allowed.iter().any(|pair| pair == canonical_pair) {
        return Ok(());
    }
    Err(WorkspaceError::PairNotAllowed {
        pair: canonical_pair.to_string(),
        workspace: manifest.name.clone(),
        allowed: if allowed.is_empty() {
            "none (deny-all)".to_string()
        } else {
            allowed.join(", ")
        },
    })
}

/// Both public modes are valid workspace contracts.
pub fn ensure_mode_creatable(_mode: WorkspaceMode, _name: &str) -> Result<()> {
    Ok(())
}

/// The honest refusal for a money surface with no paper equivalent.
pub fn unsupported(what: &str, workspace: &str, mode: WorkspaceMode) -> WorkspaceError {
    WorkspaceError::Unsupported {
        what: what.to_string(),
        workspace: workspace.to_string(),
        mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rust_decimal_macros::dec;

    fn manifest(allowed_pairs: Option<Vec<String>>) -> WorkspaceManifest {
        WorkspaceManifest {
            workspace_version: crate::WORKSPACE_VERSION.to_string(),
            cli_version: "0.0.0-test".to_string(),
            name: "scalper".to_string(),
            capital: dec!(1000),
            currency: "USD".to_string(),
            mode: WorkspaceMode::Paper,
            fee_rate: dec!(0.0026),
            slippage_rate: dec!(0),
            allowed_pairs,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn pair_allowed_when_no_allowlist_is_set() {
        assert!(ensure_pair_allowed(&manifest(None), "SOL/USD").is_ok());
    }

    #[test]
    fn explicit_empty_allowlist_denies_all_pairs() {
        let err = ensure_pair_allowed(&manifest(Some(Vec::new())), "BTC/USD")
            .expect_err("deny-all must refuse");
        assert_eq!(err.category(), "validation");
        assert!(err.to_string().contains("deny-all"), "{err}");
    }

    #[test]
    fn pair_check_canonicalizes_before_comparing() {
        // XBTUSD, btcusd, and BTC/USD are one pair in policy form.
        let allowed = canonicalize_pairs(&["xbtusd".to_string(), "BTC/USD".to_string()])
            .expect("canonicalize");
        assert_eq!(allowed, vec!["BTC/USD".to_string()], "aliases collapse");
        let subject = manifest(Some(allowed));
        assert!(ensure_pair_allowed(&subject, &canonical_pair("btcusd").expect("valid")).is_ok());
        assert!(ensure_pair_allowed(&subject, &canonical_pair("ETH/USD").expect("valid")).is_err());
    }

    #[test]
    fn live_mode_is_creatable() {
        ensure_mode_creatable(WorkspaceMode::Live, "scalper").expect("live is valid");
    }
}
