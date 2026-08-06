//! Run policy over a scope — the directory holding a `journal.jsonl`, be it
//! a workspace or the config root for the real account. The artifact itself
//! (layout, `session.json`, the windowed reader) lives in `kraken_session::run`;
//! here is what the artifact cannot know: how ordinals allocate, what a session
//! ref resolves to, which run is active, and the pure reconciliation core
//! that heals a live journal from venue history.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use kraken_core::OrderSide;
use kraken_paper::{BALANCE_DUST, PaperTrade};
use kraken_session::manifest::SessionManifest;
use kraken_session::session::{parse_ordinal, session_dir, session_file_path, sessions_root};
use rust_decimal::Decimal;
use serde::Serialize;

use crate::{Result, WorkspaceError};

/// A session's identity within its scope. Displays — and serializes — as the
/// directory name `s<n>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SessionId(u32);

impl SessionId {
    pub fn ordinal(self) -> u32 {
        self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "s{}", self.0)
    }
}

impl Serialize for SessionId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// How a `--session` argument addresses a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRefSpec {
    /// The highest ordinal on disk — the default everywhere.
    Latest,
    Ordinal(u32),
    Label(String),
}

impl FromStr for SessionRefSpec {
    type Err = std::convert::Infallible;

    /// Never fails: `latest` and `s<n>` have fixed meanings, anything else
    /// is a label lookup (which fails later, with the sessions listed).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "latest" => Self::Latest,
            other => match parse_ordinal(other) {
                Some(n) => Self::Ordinal(n),
                None => Self::Label(other.to_string()),
            },
        })
    }
}

impl fmt::Display for SessionRefSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Latest => write!(f, "latest"),
            Self::Ordinal(n) => write!(f, "s{n}"),
            Self::Label(label) => write!(f, "{label}"),
        }
    }
}

/// Liveness derived at read time, never stored: `session.json` says whether the
/// window closed; the recording lock says whether a recorder still writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SessionStatus {
    /// Window open, recorder lock held.
    Recording,
    /// Window closed by `session stop`.
    Stopped,
    /// Window open but no recorder holds the tape — a crash or kill. Never
    /// blocks the next `run start`.
    Aborted,
    /// An unreadable `session.json`; described, never dropped from the listing.
    Damaged,
}

/// One `run list` row.
#[derive(Debug, Clone, Serialize)]
pub struct SessionRecord {
    pub session: SessionId,
    pub status: SessionStatus,
    pub label: Option<String>,
    pub experiment: Option<String>,
    /// The replay source ref, when the session replayed a tape.
    pub source: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    /// What is wrong, for a damaged row.
    pub note: Option<String>,
}

/// Allocate the next session: `max(s<n>) + 1`, never reusing an ordinal (no
/// command deletes a session directory). Refuses while a session is still recording,
/// and refuses a label another run already carries.
///
/// Caller contract: hold the scope journal's writer lock (the open
/// `PaperAccount`) across allocate → provision → start-marker append. That
/// lock is what serializes two concurrent `run start`s — this function only
/// scans.
pub fn allocate(scope: &Path, label: Option<&str>) -> Result<SessionId> {
    if let Some((active, _)) = active(scope)? {
        return Err(WorkspaceError::SessionActive {
            session: active.to_string(),
        });
    }
    let runs = list(scope)?;
    if let Some(label) = label
        && let Some(taken) = runs.iter().find(|r| r.label.as_deref() == Some(label))
    {
        return Err(WorkspaceError::LabelTaken {
            label: label.to_string(),
            session: taken.session.to_string(),
        });
    }
    let next = runs.iter().map(|r| r.session.0).max().unwrap_or(0) + 1;
    Ok(SessionId(next))
}

/// Every run in the scope, ordinal-sorted (numerically — `r10` after `r2`).
/// A damaged `session.json` becomes a described row, never a failed listing.
pub fn list(scope: &Path) -> Result<Vec<SessionRecord>> {
    let entries = match std::fs::read_dir(sessions_root(scope)) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut runs = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Some(ordinal) = entry.file_name().to_str().and_then(parse_ordinal) else {
            continue;
        };
        runs.push(record(scope, SessionId(ordinal)));
    }
    runs.sort_by_key(|r| r.session.0);
    Ok(runs)
}

fn record(scope: &Path, run: SessionId) -> SessionRecord {
    match SessionManifest::load(&session_file_path(scope, run.0)) {
        Ok(manifest) => SessionRecord {
            session: run,
            status: status_of(scope, run, &manifest),
            label: manifest.label.clone(),
            experiment: manifest.experiment.clone(),
            source: manifest.source.as_ref().map(|s| s.tape.clone()),
            started_at: Some(manifest.window.started_at),
            ended_at: manifest.window.ended_at,
            note: None,
        },
        Err(err) => SessionRecord {
            session: run,
            status: SessionStatus::Damaged,
            label: None,
            experiment: None,
            source: None,
            started_at: None,
            ended_at: None,
            note: Some(err.to_string()),
        },
    }
}

/// Resolve a session ref to its contract. `Latest` with no runs, an unknown
/// ordinal, or an unknown label all name what WAS addressable.
pub fn resolve(scope: &Path, spec: &SessionRefSpec) -> Result<(SessionId, SessionManifest)> {
    let load = |run: SessionId| -> Result<(SessionId, SessionManifest)> {
        Ok((
            run,
            SessionManifest::load(&session_file_path(scope, run.0))?,
        ))
    };
    match spec {
        SessionRefSpec::Ordinal(n) => {
            if !session_file_path(scope, *n).exists() {
                return Err(not_found(scope, spec));
            }
            load(SessionId(*n))
        }
        SessionRefSpec::Latest => {
            let latest = list(scope)?.into_iter().map(|r| r.session).max();
            match latest {
                Some(run) => load(run),
                None => Err(not_found(scope, spec)),
            }
        }
        SessionRefSpec::Label(label) => {
            let runs = list(scope)?;
            let hit = runs
                .iter()
                .find(|r| r.label.as_deref() == Some(label.as_str()));
            match hit {
                Some(row) => load(row.session),
                None => Err(not_found(scope, spec)),
            }
        }
    }
}

fn not_found(scope: &Path, spec: &SessionRefSpec) -> WorkspaceError {
    let known = list(scope)
        .map(|runs| {
            runs.iter()
                .map(|r| match &r.label {
                    Some(label) => format!("{} ({label})", r.session),
                    None => r.session.to_string(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    WorkspaceError::SessionNotFound {
        reference: spec.to_string(),
        known: if known.is_empty() {
            "none yet — start one with 'kraken session start'".to_string()
        } else {
            known
        },
    }
}

/// The run still recording in this scope, if any: window open AND a recorder
/// holding one of the session's tape locks. Aborted runs (open window, dead
/// recorder) never count — a crash must not block the next start.
pub fn active(scope: &Path) -> Result<Option<(SessionId, SessionManifest)>> {
    for row in list(scope)?.into_iter().rev() {
        if row.status != SessionStatus::Recording {
            continue;
        }
        let manifest = SessionManifest::load(&session_file_path(scope, row.session.0))?;
        return Ok(Some((row.session, manifest)));
    }
    Ok(None)
}

fn status_of(scope: &Path, run: SessionId, manifest: &SessionManifest) -> SessionStatus {
    if manifest.window.ended_at.is_some() {
        return SessionStatus::Stopped;
    }
    let dir = session_dir(scope, run.0);
    let alive = manifest
        .recordings
        .iter()
        .any(|rec| kraken_recording::is_lock_held(&dir.join(&rec.file)));
    if alive {
        SessionStatus::Recording
    } else {
        SessionStatus::Aborted
    }
}

/// One venue trade as `session stop` reconciliation consumes it — parsed by the
/// binary from `TradesHistory`, joined here by txid.
#[derive(Debug, Clone)]
pub struct VenueFill {
    /// The venue trade txid — becomes `PaperTrade.id`, the idempotency key.
    pub txid: String,
    /// The venue order txid — becomes `PaperTrade.order_id`, the same key
    /// the live mirror stamps at submission.
    pub order_txid: String,
    pub pair: String,
    pub side: OrderSide,
    pub volume: Decimal,
    pub price: Decimal,
    pub fee: Decimal,
    pub cost: Decimal,
    pub time: DateTime<Utc>,
}

/// The venue fills the journal has not seen: keyed on venue txid ==
/// `PaperTrade.id`, so appending the result and reconciling again yields
/// nothing — a second `session stop` appends nothing.
pub fn missing_fills<'v>(known: &[PaperTrade], venue: &'v [VenueFill]) -> Vec<&'v VenueFill> {
    let seen: HashSet<&str> = known.iter().map(|t| t.id.as_str()).collect();
    venue
        .iter()
        .filter(|fill| !seen.contains(fill.txid.as_str()))
        .collect()
}

/// One asset whose folded balance disagrees with the venue beyond dust.
#[derive(Debug, Clone, Serialize)]
pub struct DriftLine {
    pub asset: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub local: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub venue: Decimal,
}

/// Folded state vs a venue snapshot, both sides' assets, dust-tolerant so
/// float-era residue never reads as drift.
pub fn drift(
    local: &HashMap<String, Decimal>,
    venue: &BTreeMap<String, Decimal>,
) -> Vec<DriftLine> {
    let mut assets: Vec<&String> = local.keys().chain(venue.keys()).collect();
    assets.sort();
    assets.dedup();
    assets
        .into_iter()
        .filter_map(|asset| {
            let ours = local.get(asset).copied().unwrap_or(Decimal::ZERO);
            let theirs = venue.get(asset).copied().unwrap_or(Decimal::ZERO);
            ((ours - theirs).abs() > BALANCE_DUST).then(|| DriftLine {
                asset: asset.clone(),
                local: ours,
                venue: theirs,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use kraken_session::manifest::{PaperRef, SessionManifest, SessionWindow};
    use rust_decimal_macros::dec;

    use super::*;

    fn write_session(scope: &Path, ordinal: u32, label: Option<&str>, stopped: bool) {
        let manifest = SessionManifest::new(
            format!("s{ordinal}").parse().unwrap(),
            "0.0.0-test".to_string(),
            4242,
            Vec::new(),
            PaperRef {
                starting_balance: dec!(10_000),
                currency: "USD".to_string(),
            },
            SessionWindow {
                session: format!("s{ordinal}"),
                started_at: "2026-01-01T00:00:00Z".parse().unwrap(),
                opening_equity: dec!(10_000),
                opening_complete: true,
                ended_at: stopped.then(|| "2026-01-01T01:00:00Z".parse().unwrap()),
            },
        )
        .with_label(label.map(str::to_string));
        manifest
            .save(&session_file_path(scope, ordinal))
            .expect("fixture session.json");
    }

    #[test]
    fn ordinals_allocate_max_plus_one_and_never_reuse() {
        let scope = tempfile::tempdir().unwrap();
        assert_eq!(allocate(scope.path(), None).unwrap().to_string(), "s1");
        write_session(scope.path(), 1, None, true);
        write_session(scope.path(), 7, None, true);
        assert_eq!(
            allocate(scope.path(), None).unwrap().to_string(),
            "s8",
            "gaps stay gaps; the max drives the next ordinal"
        );
    }

    #[test]
    fn allocate_refuses_a_taken_label() {
        let scope = tempfile::tempdir().unwrap();
        write_session(scope.path(), 1, Some("momentum-r1"), true);
        let err = allocate(scope.path(), Some("momentum-r1")).unwrap_err();
        assert!(matches!(err, WorkspaceError::LabelTaken { .. }), "{err}");
    }

    #[test]
    fn a_crashed_session_never_blocks_the_next_start() {
        let scope = tempfile::tempdir().unwrap();
        // Open window, no recorder lock: aborted, not active.
        write_session(scope.path(), 1, None, false);
        assert!(active(scope.path()).unwrap().is_none());
        assert_eq!(allocate(scope.path(), None).unwrap().to_string(), "s2");
    }

    #[test]
    fn sessions_sort_numerically_not_lexically() {
        let scope = tempfile::tempdir().unwrap();
        for n in [1, 2, 10] {
            write_session(scope.path(), n, None, true);
        }
        let order: Vec<String> = list(scope.path())
            .unwrap()
            .iter()
            .map(|r| r.session.to_string())
            .collect();
        assert_eq!(order, ["s1", "s2", "s10"]);
    }

    #[test]
    fn resolve_latest_ordinal_and_label() {
        let scope = tempfile::tempdir().unwrap();
        write_session(scope.path(), 1, Some("first"), true);
        write_session(scope.path(), 2, None, true);
        let (latest, _) = resolve(scope.path(), &SessionRefSpec::Latest).unwrap();
        assert_eq!(latest.to_string(), "s2");
        let (by_ordinal, _) = resolve(scope.path(), &SessionRefSpec::Ordinal(1)).unwrap();
        assert_eq!(by_ordinal.to_string(), "s1");
        let (by_label, _) =
            resolve(scope.path(), &SessionRefSpec::Label("first".to_string())).unwrap();
        assert_eq!(by_label.to_string(), "s1");
        let err = resolve(scope.path(), &SessionRefSpec::Label("ghost".to_string())).unwrap_err();
        assert!(
            err.to_string().contains("s1 (first)"),
            "the refusal lists what exists: {err}"
        );
    }

    #[test]
    fn a_damaged_session_json_is_a_described_row_never_a_failed_listing() {
        let scope = tempfile::tempdir().unwrap();
        write_session(scope.path(), 1, None, true);
        std::fs::create_dir_all(session_dir(scope.path(), 2)).unwrap();
        std::fs::write(session_file_path(scope.path(), 2), "{ not json").unwrap();
        let runs = list(scope.path()).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[1].status, SessionStatus::Damaged);
        assert!(runs[1].note.is_some());
    }

    #[test]
    fn reconcile_twice_appends_nothing() {
        let venue = vec![VenueFill {
            txid: "TVENUE-1".to_string(),
            order_txid: "OVENUE-1".to_string(),
            pair: "BTCUSD".to_string(),
            side: OrderSide::Buy,
            volume: dec!(0.1),
            price: dec!(50_000),
            fee: dec!(13),
            cost: dec!(5_000),
            time: "2026-01-01T00:30:00Z".parse().unwrap(),
        }];
        assert_eq!(missing_fills(&[], &venue).len(), 1);

        // After the first reconcile the fill is on the journal under the
        // venue txid — the second pass must see nothing.
        let folded = vec![PaperTrade {
            id: "TVENUE-1".to_string(),
            order_id: "OVENUE-1".to_string(),
            pair: "BTCUSD".to_string(),
            base: "BTC".to_string(),
            quote: "USD".to_string(),
            side: OrderSide::Buy,
            volume: dec!(0.1),
            price: dec!(50_000),
            fee: dec!(13),
            cost: dec!(5_000),
            filled_at: "2026-01-01T00:30:00Z".parse().unwrap(),
            reference_quote: None,
        }];
        assert!(missing_fills(&folded, &venue).is_empty());
    }

    #[test]
    fn drift_ignores_dust_and_reports_both_sides() {
        let local: HashMap<String, Decimal> =
            [("USD".to_string(), dec!(100)), ("BTC".to_string(), dec!(1))]
                .into_iter()
                .collect();
        let venue: BTreeMap<String, Decimal> = [
            ("USD".to_string(), dec!(100.0000000000001)),
            ("ETH".to_string(), dec!(2)),
        ]
        .into_iter()
        .collect();
        let lines = drift(&local, &venue);
        let assets: Vec<&str> = lines.iter().map(|l| l.asset.as_str()).collect();
        assert_eq!(
            assets,
            ["BTC", "ETH"],
            "USD residue is dust, both-side assets report"
        );
    }
}
