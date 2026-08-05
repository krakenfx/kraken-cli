//! Event-sourced paper-account storage.
//!
//! The locked journal is authoritative. Projection changes follow durable
//! appends, keeping each read-decide-write command atomic across processes.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use kraken_recording::JsonlSink;
use kraken_recording::Sink;
use kraken_recording::Source;
use kraken_recording::paper::{journal, replay};
use serde::{Deserialize, Serialize};

use crate::{AccountEvent, CommandEntry, PaperError, PaperState, Result};

/// Version stamped on every record this build writes. A higher version aborts
/// the replay — skipping a newer build's state event would corrupt balances.
pub const RECORD_VERSION: u32 = 1;

/// The storage layer owns the journal contract; re-exported for the
/// playground's lock-holder diagnostics.
pub use kraken_recording::paper::JOURNAL_PURPOSE;

fn record_version_default() -> u32 {
    RECORD_VERSION
}

/// Where a command entered the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Cli,
    Mcp,
}

impl Origin {
    pub fn from_mcp_mode(mcp_mode: bool) -> Self {
        if mcp_mode { Self::Mcp } else { Self::Cli }
    }
}

/// One `events.jsonl` line: a versioned, stamped envelope around an event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountRecord {
    #[serde(default = "record_version_default")]
    pub v: u32,
    /// Append instant — the authoritative time of `Reset` epochs and
    /// cancellations (submissions and fills carry their own stamps).
    pub ts: DateTime<Utc>,
    pub origin: Origin,
    #[serde(flatten)]
    pub event: AccountEvent,
}

/// A locked journal and its live projection.
///
/// Initialization is journal state, not file existence.
pub struct PaperAccount {
    origin: Origin,
    sink: JsonlSink<AccountRecord>,
    /// The live state: replayed once at open, advanced by every append.
    projection: Projection,
}

impl PaperAccount {
    /// Locks before replay so corruption and version errors fail at open.
    pub fn open_at(log_path: PathBuf, origin: Origin) -> Result<Self> {
        let sink = journal(log_path.clone())?;
        let mut projection = Projection::default();
        if let Some(source) = replay(log_path) {
            for record in source.read()? {
                projection.apply(&record)?;
            }
        }
        Ok(Self {
            origin,
            sink,
            projection,
        })
    }

    /// Whether an `Initialized`/`Reset` epoch is on the record — an existing
    /// but epoch-less journal (a crashed first init) reads as uninitialized.
    pub fn is_initialized(&self) -> bool {
        self.projection.state().is_some()
    }

    /// The live state, or "not initialized" when no epoch is on the record.
    /// Callers clone the borrow to draft decisions; the projection itself
    /// advances only through appends.
    pub fn state(&self) -> Result<&PaperState> {
        self.projection.state().ok_or_else(|| {
            PaperError::Rejected(
                "Paper account not initialized. Create a workspace: 'kraken workspace create \
                 <name> --capital <amount>'."
                    .into(),
            )
        })
    }

    /// Wrap events in records stamped with one append instant — one shared
    /// instant per batch, so within-batch order is file position on replay.
    #[must_use]
    pub fn stamp(&self, events: Vec<AccountEvent>) -> Vec<AccountRecord> {
        self.stamp_at(events, Utc::now())
    }

    /// [`stamp`](Self::stamp) with a caller-chosen instant. For fills folded
    /// lazily from a recorded tape: the record must carry the fill's
    /// session-time instant, not the fold's wall clock, or the timeline
    /// would order every folded fill at command time.
    #[must_use]
    pub fn stamp_at(&self, events: Vec<AccountEvent>, ts: DateTime<Utc>) -> Vec<AccountRecord> {
        events
            .into_iter()
            .map(|event| AccountRecord {
                v: RECORD_VERSION,
                ts,
                origin: self.origin,
                event,
            })
            .collect()
    }

    /// Commits events and their post-apply audit as one durable batch.
    pub fn commit(&mut self, events: Vec<AccountEvent>, mut entry: CommandEntry) -> Result<()> {
        let mut records = self.stamp(events);
        let mut next = self.projection.clone();
        for record in &records {
            next.apply(record)?;
        }
        entry.balances = next.state().map(|state| state.balances.clone());
        let audit = self.stamp(vec![AccountEvent::Command(entry)]);
        for record in &audit {
            next.apply(record)?;
        }
        records.extend(audit);
        self.sink.record(&records)?;
        self.projection = next;
        Ok(())
    }

    /// Append one fsync'd batch, then advance the live projection — an event
    /// enters state only once it is on the record. The epoch gate is the
    /// command layer's check, race-free because this handle holds the
    /// journal's only lock.
    pub fn append(&mut self, records: &[AccountRecord]) -> Result<()> {
        self.sink.record(records)?;
        for record in records {
            self.projection.apply(record)?;
        }
        Ok(())
    }

    /// [`Self::append`] gated on no epoch being on the record — the account
    /// owns the "one first init" invariant (provision's entry point).
    pub fn append_if_uninitialized(&mut self, records: &[AccountRecord]) -> Result<()> {
        self.ensure_uninitialized()?;
        self.append(records)
    }

    /// [`Self::commit`] under the same gate — `paper init`'s entry point.
    pub fn commit_if_uninitialized(
        &mut self,
        events: Vec<AccountEvent>,
        entry: CommandEntry,
    ) -> Result<()> {
        self.ensure_uninitialized()?;
        self.commit(events, entry)
    }

    fn ensure_uninitialized(&self) -> Result<()> {
        if self.is_initialized() {
            return Err(PaperError::Rejected(
                "Paper account already initialized. Use 'kraken paper reset' to start over.".into(),
            ));
        }
        Ok(())
    }

    /// Delete the journal at `path` under its writer lock; already absent
    /// counts as success. Never replays: removal is `--force` recovery's
    /// escape hatch, so it must work on exactly the journals an open refuses.
    pub fn remove_at(path: PathBuf) -> Result<()> {
        // `fs::exists`, not `Path::exists`: a stat failure must surface, not
        // read as "absent".
        if !std::fs::exists(&path)? {
            return Ok(());
        }
        Ok(journal::<AccountRecord>(path)?.remove()?)
    }

    /// Append for observational entries: a failure is warned, never surfaced
    /// — audit must not shadow the command's own result.
    pub fn audit(&mut self, records: &[AccountRecord]) {
        if let Err(err) = self.sink.record(records) {
            tracing::warn!(error = %err, "failed to append account audit entry");
            return;
        }
        for record in records {
            // Our own records carry a supported version, so apply can only
            // refuse foreign input — warn, never surface.
            if let Err(err) = self.projection.apply(record) {
                tracing::warn!(error = %err, "audit record not applied to the live projection");
            }
        }
    }
}

/// The journal's live read model: state replayed one record at a time.
///
/// The open-time replay and every later append advance the same
/// [`Projection::apply`], so the two cannot drift.
#[derive(Debug, Default, Clone)]
struct Projection {
    state: PaperState,
    /// Set by an `Initialized`/`Reset` epoch — replaying nothing must not
    /// conjure the default account.
    initialized: bool,
}

impl Projection {
    /// Advance by one record: an unknown event kind is warned and skipped; a
    /// newer-versioned record is a hard error, since skipping a state event
    /// would corrupt balances.
    fn apply(&mut self, record: &AccountRecord) -> Result<()> {
        if record.v > RECORD_VERSION {
            return Err(PaperError::Incompatible(format!(
                "account log record version {} is newer than this build supports ({RECORD_VERSION}); upgrade kraken",
                record.v
            )));
        }
        match &record.event {
            // An attached journal is a usable account: the venue snapshot is
            // its epoch. A mid-window `Reconciled` deliberately does NOT
            // initialize — reconciliation refines an account, never opens one.
            AccountEvent::Initialized(_) | AccountEvent::Reset(_) | AccountEvent::Attached(_) => {
                self.initialized = true
            }
            AccountEvent::Unknown => {
                tracing::warn!(ts = %record.ts, "skipping unknown account event kind (newer build?)");
            }
            _ => {}
        }
        self.state.apply(record.ts, &record.event);
        Ok(())
    }

    /// The replayed state, or `None` when no epoch is on the record.
    fn state(&self) -> Option<&PaperState> {
        self.initialized.then_some(&self.state)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use kraken_core::OrderSide;
    use kraken_recording::JsonlSource;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    use super::*;
    use crate::{CommandEntry, PaperConfig};

    fn config(balance: Decimal) -> PaperConfig {
        PaperConfig {
            balance,
            currency: "USD".into(),
            fee_rate: dec!(0.0),
            slippage_rate: dec!(0.0),
        }
    }

    fn open(dir: &tempfile::TempDir) -> PaperAccount {
        PaperAccount::open_at(dir.path().join("events.jsonl"), Origin::Cli).unwrap()
    }

    #[test]
    fn busy_account_rejects_a_second_open_with_in_use() {
        // While a command's handle holds the account, a concurrent open is
        // rejected outright — fail-fast at the account layer, not just the lock.
        let dir = tempfile::tempdir().unwrap();
        let _held = open(&dir); // a command mid-flight, lock spanning its lifetime

        // `PaperAccount` is deliberately non-Debug, so match rather than unwrap_err.
        let err = match PaperAccount::open_at(dir.path().join("events.jsonl"), Origin::Cli) {
            Ok(_) => panic!("busy account must be rejected"),
            Err(e) => e,
        };
        // The lock refusal is the store's, surfaced transparently — pinned as
        // `Journal(Rejected)` so the binary's bridge lands it on `validation`.
        assert!(matches!(
            err,
            PaperError::Journal(kraken_recording::Error::Rejected(_))
        ));
        assert!(err.to_string().contains("in use"), "got: {err}");
    }

    /// A fresh replay must reproduce the working state exactly, timestamps
    /// included — the core event-sourcing invariant.
    #[test]
    fn replay_reproduces_committed_state_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let mut account = open(&dir);
        let mut state = PaperState::default();

        let init = account.stamp(vec![AccountEvent::Initialized(config(dec!(10_000.0)))]);
        for r in &init {
            state.apply(r.ts, &r.event);
        }
        account.append(&init).unwrap();
        // Re-open as every real command does; drop first to release the lock.
        drop(account);
        let mut account = open(&dir);

        let order = state
            .decide_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.1), dec!(45_000.0))
            .unwrap();
        let submit = account.stamp(vec![AccountEvent::OrderSubmitted { order }]);
        for r in &submit {
            state.apply(r.ts, &r.event);
        }
        account.append(&submit).unwrap();

        let trade = state
            .decide_market_order(
                OrderSide::Buy,
                "ETHUSD",
                dec!(1.0),
                dec!(2_000.0),
                dec!(1_999.0),
            )
            .unwrap();
        let fill = account.stamp(vec![AccountEvent::OrderFilled { trade }]);
        for r in &fill {
            state.apply(r.ts, &r.event);
        }
        account.append(&fill).unwrap();

        let cancel = state.decide_cancel("PAPER-00001").unwrap();
        let cancelled = account.stamp(vec![AccountEvent::OrderCancelled { order: cancel }]);
        for r in &cancelled {
            state.apply(r.ts, &r.event);
        }
        account.append(&cancelled).unwrap();

        // Both the live projection and a fresh open's replay must equal the
        // state the commands built.
        let live = account.state().unwrap().clone();
        drop(account); // release the lock so the fresh open below succeeds
        let reopened = open(&dir);
        let fresh = reopened.state().unwrap().clone();
        for replayed in [live, fresh] {
            assert_eq!(
                serde_json::to_value(&replayed).unwrap(),
                serde_json::to_value(&state).unwrap(),
                "replay(journal) must equal the state the commands built"
            );
            assert_eq!(replayed.filled_trades.len(), 1);
            assert_eq!(replayed.cancelled_orders.len(), 1);
            assert!(replayed.open_orders.is_empty());
        }
    }

    /// The exactness invariant beyond the f64 wire: a fill whose money
    /// carries more significant digits than a JSON double survives the
    /// journal round-trip bit-for-bit. Regression for the `serde-float`
    /// journal (cost 6179.0266686284136 used to reload as …628414).
    #[test]
    fn replay_stays_exact_beyond_f64_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut account = open(&dir);
        account
            .append(&account.stamp(vec![AccountEvent::Initialized(PaperConfig {
                balance: dec!(10_000.0),
                currency: "USD".into(),
                fee_rate: dec!(0.0026),
                slippage_rate: dec!(0.001),
            })]))
            .unwrap();
        drop(account);
        let mut account = open(&dir);

        // 0.12345678 × 50_000.12 × 1.001 = 6179.0266686284136 — 17
        // significant digits, not representable as an f64.
        let trade = account
            .state()
            .unwrap()
            .decide_market_order(
                OrderSide::Buy,
                "BTCUSD",
                dec!(0.12345678),
                dec!(50_000.12),
                dec!(49_999.88),
            )
            .unwrap();
        let expected_cost = trade.cost;
        assert_eq!(expected_cost, dec!(6179.0266686284136));
        account
            .append(&account.stamp(vec![AccountEvent::OrderFilled { trade }]))
            .unwrap();

        let live = account.state().unwrap().clone();
        drop(account);
        let fresh = open(&dir).state().unwrap().clone();
        for state in [&live, &fresh] {
            assert_eq!(state.filled_trades[0].cost, expected_cost);
        }
        assert_eq!(
            serde_json::to_value(&fresh).unwrap(),
            serde_json::to_value(&live).unwrap(),
            "a fresh replay must equal the live projection, digits included"
        );
    }

    #[test]
    fn observational_events_do_not_change_the_replay() {
        let dir = tempfile::tempdir().unwrap();
        let mut account = open(&dir);
        account
            .append(&account.stamp(vec![AccountEvent::Initialized(config(dec!(500.0)))]))
            .unwrap();
        drop(account);
        let mut account = open(&dir);
        let before = serde_json::to_value(account.state().unwrap()).unwrap();

        account.audit(&account.stamp(vec![AccountEvent::Command(CommandEntry {
            name: "status".into(),
            ..CommandEntry::default()
        })]));
        let after = serde_json::to_value(account.state().unwrap()).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn reset_starts_a_new_epoch_and_history_survives_in_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = PaperState::default();

        // A fresh handle per commit, as every real command runs.
        let commit = |state: &mut PaperState, event: AccountEvent| {
            let mut account = open(&dir);
            let records = account.stamp(vec![event]);
            for r in &records {
                state.apply(r.ts, &r.event);
            }
            account.append(&records).unwrap();
        };

        commit(
            &mut state,
            AccountEvent::Initialized(config(dec!(10_000.0))),
        );
        let trade = state
            .decide_market_order(
                OrderSide::Buy,
                "BTCUSD",
                dec!(0.1),
                dec!(50_000.0),
                dec!(49_900.0),
            )
            .unwrap();
        commit(&mut state, AccountEvent::OrderFilled { trade });
        commit(&mut state, AccountEvent::Reset(config(dec!(10_000.0))));

        // Post-reset the replayed state is fresh — and order ids restart.
        let account = open(&dir);
        let replayed = account.state().unwrap();
        assert!(replayed.filled_trades.is_empty());
        let next = replayed
            .decide_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.01), dec!(40_000.0))
            .unwrap();
        assert_eq!(next.id, "PAPER-00001", "reset restarts the id epoch");

        // But the pre-reset fill is still on the record for replay.
        let entries: Vec<AccountRecord> = JsonlSource::open(dir.path().join("events.jsonl"))
            .unwrap()
            .read()
            .unwrap()
            .collect();
        let fills = entries
            .iter()
            .filter(|r| matches!(r.event, AccountEvent::OrderFilled { .. }))
            .count();
        assert_eq!(fills, 1, "history must survive reset in the log");
    }

    #[test]
    fn empty_log_is_not_an_initialized_account() {
        // A crash before init's first durable line leaves an empty log — that
        // must read as uninitialized, never as a conjured default account.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        std::fs::write(&path, "").unwrap();
        let account = PaperAccount::open_at(path, Origin::Cli).unwrap();
        assert!(!account.is_initialized());
        let err = account.state().unwrap_err();
        assert!(err.to_string().contains("not initialized"), "got: {err}");
    }

    #[test]
    fn crashed_first_init_recovers_on_reinit() {
        // Torn-only fragment (crash mid first append): reads as uninitialized,
        // and a re-run init appends a clean epoch over the repaired log.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        std::fs::write(&path, "{\"v\":1,\"ts\":\"2026").unwrap();
        let mut account = PaperAccount::open_at(path, Origin::Cli).unwrap();
        assert!(!account.is_initialized());

        account
            .append(&account.stamp(vec![AccountEvent::Initialized(config(dec!(5_000.0)))]))
            .unwrap();
        let state = account.state().unwrap();
        assert_eq!(state.balances.get("USD"), Some(&dec!(5_000.0)));
    }

    #[test]
    fn concurrent_init_is_rejected_at_open_not_interleaved() {
        // Two processes race `paper init`: the loser is rejected "in use" at
        // open — no unlocked window in which to stack a second epoch.
        let dir = tempfile::tempdir().unwrap();
        let mut winner = open(&dir);
        let err = match PaperAccount::open_at(dir.path().join("events.jsonl"), Origin::Cli) {
            Ok(_) => panic!("second open must be rejected while the first is held"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("in use"), "got: {err}");

        winner
            .append(&winner.stamp(vec![AccountEvent::Initialized(config(dec!(5_000.0)))]))
            .unwrap();
        drop(winner);
        let survivor = open(&dir);
        let state = survivor.state().unwrap();
        assert_eq!(state.balances.get("USD"), Some(&dec!(5_000.0)));
    }

    #[test]
    fn audit_only_log_stays_uninitialized() {
        // Observational records appended to an epoch-less log (the failure
        // audit path) must not flip the account to initialized.
        let dir = tempfile::tempdir().unwrap();
        let mut account = open(&dir);
        account.audit(&account.stamp(vec![AccountEvent::Command(CommandEntry {
            name: "status".into(),
            ..CommandEntry::default()
        })]));
        assert!(!account.is_initialized());
    }

    #[test]
    fn append_repairs_a_torn_tail_instead_of_gluing() {
        // An append must not glue onto a crash fragment — that would turn a
        // recoverable torn tail into a malformed interior line.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let mut account = PaperAccount::open_at(path.clone(), Origin::Cli).unwrap();
        account
            .append(&account.stamp(vec![AccountEvent::Initialized(config(dec!(500.0)))]))
            .unwrap();
        drop(account);
        let mut account = PaperAccount::open_at(path.clone(), Origin::Cli).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"{\"v\":1,\"ts\":\"2026").unwrap();
        drop(file);

        account.audit(&account.stamp(vec![AccountEvent::Command(CommandEntry {
            name: "status".into(),
            ..CommandEntry::default()
        })]));
        // Every line parses: the fragment was truncated, not glued onto.
        let entries: Vec<AccountRecord> = JsonlSource::open(path.clone())
            .unwrap()
            .read()
            .unwrap()
            .collect();
        assert_eq!(entries.len(), 2);
        assert!(account.state().is_ok());
    }

    #[test]
    fn torn_tail_does_not_corrupt_the_replay() {
        let dir = tempfile::tempdir().unwrap();
        let mut account = open(&dir);
        account
            .append(&account.stamp(vec![AccountEvent::Initialized(config(dec!(500.0)))]))
            .unwrap();
        let path = dir.path().join("events.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"{\"v\":1,\"ts\":\"2026").unwrap();
        drop(file);

        // The fragment is never parsed: the replay sees only clean lines.
        drop(account);
        let account = open(&dir);
        let state = account.state().unwrap();
        assert_eq!(state.balances.get("USD"), Some(&dec!(500.0)));
        let entries: Vec<AccountRecord> = JsonlSource::open(path.clone())
            .unwrap()
            .read()
            .unwrap()
            .collect();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn unknown_event_kind_is_tolerated_by_the_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"v":1,"ts":"2026-01-01T00:00:00Z","origin":"cli","event":"initialized","balance":"500.0","currency":"USD","fee_rate":"0.0","slippage_rate":"0.0"}"#,
                "\n",
                r#"{"v":1,"ts":"2026-01-01T00:00:01Z","origin":"mcp","event":"margin_call_simulated","severity":"high"}"#,
                "\n"
            ),
        )
        .unwrap();
        let account = PaperAccount::open_at(path, Origin::Cli).unwrap();
        let state = account.state().unwrap();
        assert_eq!(state.balances.get("USD"), Some(&dec!(500.0)));
    }

    #[test]
    fn newer_record_version_is_rejected_at_open() {
        // A newer build's journal refuses the whole handle at open — no
        // command can half-use it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"v":2,"ts":"2026-01-01T00:00:00Z","origin":"cli","event":"initialized","balance":"500.0","currency":"USD","fee_rate":"0.0","slippage_rate":"0.0"}"#,
                "\n"
            ),
        )
        .unwrap();
        let err = match PaperAccount::open_at(path, Origin::Cli) {
            Ok(_) => panic!("newer journal must be rejected at open"),
            Err(e) => e,
        };
        assert!(matches!(err, PaperError::Incompatible(_)), "got: {err:?}");
    }

    #[test]
    fn remove_at_deletes_a_journal_this_build_cannot_replay() {
        // Removal is `--force` recovery's escape hatch, so it must work on
        // exactly the journals `open_at` refuses — no replay before unlink.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        std::fs::write(&path, "not json\n").unwrap();

        PaperAccount::remove_at(path.clone()).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn remove_at_is_rejected_while_the_journal_is_in_use() {
        // The unlink is serialized under the writer lock — a `--force` reset
        // must not delete the file under an in-flight command's handle.
        let dir = tempfile::tempdir().unwrap();
        let _held = open(&dir); // a command mid-flight, lock spanning its lifetime

        let err = PaperAccount::remove_at(dir.path().join("events.jsonl")).unwrap_err();
        assert!(err.to_string().contains("in use"), "got: {err}");
        assert!(
            dir.path().join("events.jsonl").exists(),
            "a held journal survives"
        );
    }

    #[test]
    fn remove_at_of_a_missing_journal_is_success_and_leaves_no_trace() {
        // Tolerating absence must not conjure what it removes: no journal,
        // no lock sidecar, not even the parent directory.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("events.jsonl");

        PaperAccount::remove_at(path.clone()).unwrap();
        assert!(!path.parent().unwrap().exists(), "no directory created");
    }

    #[test]
    fn init_on_an_initialized_account_is_rejected() {
        // A gated append on an initialized journal refuses instead of stacking
        // a second epoch; racing handles are already rejected at open.
        let dir = tempfile::tempdir().unwrap();
        let mut account = open(&dir);
        account
            .append_if_uninitialized(
                &account.stamp(vec![AccountEvent::Initialized(config(dec!(1_000.0)))]),
            )
            .unwrap();
        let err = account
            .append_if_uninitialized(
                &account.stamp(vec![AccountEvent::Initialized(config(dec!(2_000.0)))]),
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("already initialized"),
            "got: {err}"
        );
        assert_eq!(
            account.state().unwrap().balances.get("USD"),
            Some(&dec!(1_000.0))
        );
    }

    #[test]
    fn commit_stamps_audit_balances_and_advances_the_projection() {
        // Commit appends the events plus a Command audit entry carrying
        // post-apply balances; the live projection reflects the batch at once.
        let dir = tempfile::tempdir().unwrap();
        let mut account = open(&dir);
        account
            .commit(
                vec![AccountEvent::Initialized(config(dec!(1_000.0)))],
                CommandEntry {
                    name: "init".into(),
                    ..CommandEntry::default()
                },
            )
            .unwrap();

        assert_eq!(
            account.state().unwrap().balances.get("USD"),
            Some(&dec!(1_000.0))
        );
        let records: Vec<AccountRecord> = JsonlSource::open(dir.path().join("events.jsonl"))
            .unwrap()
            .read()
            .unwrap()
            .collect();
        assert_eq!(records.len(), 2, "events + one audit entry");
        match &records[1].event {
            AccountEvent::Command(entry) => {
                let balances = entry.balances.as_ref().expect("audit carries balances");
                assert_eq!(balances.get("USD"), Some(&dec!(1_000.0)));
            }
            other => panic!("last record must be the audit entry, got {other:?}"),
        }
    }

    #[test]
    fn records_carry_no_secret_shaped_fields() {
        let dir = tempfile::tempdir().unwrap();
        let account = open(&dir);
        let records = account.stamp(vec![
            AccountEvent::Initialized(config(dec!(10_000.0))),
            AccountEvent::Command(CommandEntry {
                name: "buy".into(),
                pair: Some("BTCUSD".into()),
                ..CommandEntry::default()
            }),
        ]);
        for record in &records {
            let json = serde_json::to_string(record).unwrap();
            for needle in ["api_key", "api_secret", "password", "otp", "token"] {
                assert!(!json.contains(needle), "{needle} in {json}");
            }
        }
    }

    #[test]
    fn attached_snapshot_initializes_and_sets_balances() {
        let dir = tempfile::tempdir().unwrap();
        let mut account = open(&dir);
        assert!(!account.is_initialized(), "no epoch yet");

        let snapshot = crate::VenueSnapshot {
            balances: [("USD".to_string(), dec!(123.45))].into_iter().collect(),
            complete: true,
            anchor: None,
        };
        let records = account.stamp(vec![AccountEvent::Attached(snapshot)]);
        account.append(&records).unwrap();

        let state = account.state().expect("an attached journal is usable");
        assert_eq!(state.balances["USD"], dec!(123.45));
    }

    /// The window-anchor contract: a snapshot carrying `equity`
    /// re-anchors P&L at that value; a plain mirror snapshot leaves the
    /// anchor untouched.
    #[test]
    fn attached_equity_re_anchors_starting_balance_and_plain_attach_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let mut account = open(&dir);
        let records = account.stamp(vec![AccountEvent::Initialized(config(dec!(10_000.0)))]);
        account.append(&records).unwrap();

        let plain = crate::VenueSnapshot {
            balances: [("USD".to_string(), dec!(9_000.0))].into_iter().collect(),
            complete: true,
            anchor: None,
        };
        let records = account.stamp(vec![AccountEvent::Attached(plain)]);
        account.append(&records).unwrap();
        assert_eq!(
            account.state().unwrap().starting_balance,
            dec!(10_000.0),
            "a mirror attach observes balances, never the anchor"
        );

        let anchored = crate::VenueSnapshot {
            balances: [("USD".to_string(), dec!(9_000.0))].into_iter().collect(),
            complete: true,
            anchor: Some(crate::WindowAnchor {
                equity: dec!(9_123.45),
                currency: "USD".to_string(),
                fee_rate: dec!(0.0),
                slippage_rate: dec!(0.0),
            }),
        };
        let records = account.stamp(vec![AccountEvent::Attached(anchored)]);
        account.append(&records).unwrap();
        let state = account.state().unwrap();
        assert_eq!(state.starting_balance, dec!(9_123.45));
        assert_eq!(state.starting_currency, "USD");
    }

    /// A window-opening snapshot restores the account's rates, so a scoring
    /// window folded from it reports them rather than the type defaults — the
    /// `Initialized`/`Reset` that set them lies before the window. A plain
    /// mirror attach carries no rates and must leave them untouched.
    #[test]
    fn attached_rates_restore_into_the_folded_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut account = open(&dir);
        let mut init = config(dec!(10_000.0));
        init.fee_rate = dec!(0.005);
        init.slippage_rate = dec!(0.003);
        account
            .append(&account.stamp(vec![AccountEvent::Initialized(init)]))
            .unwrap();

        // A plain mirror attach (no rates) leaves the account's rates intact.
        let plain = crate::VenueSnapshot {
            balances: [("USD".to_string(), dec!(10_000.0))].into_iter().collect(),
            complete: true,
            anchor: None,
        };
        account
            .append(&account.stamp(vec![AccountEvent::Attached(plain)]))
            .unwrap();
        let state = account.state().unwrap();
        assert_eq!(
            state.fee_rate,
            dec!(0.005),
            "plain attach must not clear rates"
        );
        assert_eq!(state.slippage_rate, dec!(0.003));

        // A window opener carrying rates re-anchors them (a mid-window reset
        // could have changed them; the opener re-establishes the window's).
        let opener = crate::VenueSnapshot {
            balances: [("USD".to_string(), dec!(10_000.0))].into_iter().collect(),
            complete: true,
            anchor: Some(crate::WindowAnchor {
                equity: dec!(10_000.0),
                currency: "USD".to_string(),
                fee_rate: dec!(0.001),
                slippage_rate: dec!(0.002),
            }),
        };
        account
            .append(&account.stamp(vec![AccountEvent::Attached(opener)]))
            .unwrap();
        let state = account.state().unwrap();
        assert_eq!(state.fee_rate, dec!(0.001));
        assert_eq!(state.slippage_rate, dec!(0.002));
    }

    #[test]
    fn reconciled_overwrites_balances_without_clearing_history() {
        let dir = tempfile::tempdir().unwrap();
        let mut account = open(&dir);
        let records = account.stamp(vec![AccountEvent::Initialized(config(dec!(10_000.0)))]);
        account.append(&records).unwrap();
        let trade = account
            .state()
            .unwrap()
            .clone()
            .decide_market_order(
                OrderSide::Buy,
                "BTCUSD",
                dec!(0.1),
                dec!(50_000.0),
                dec!(49_900.0),
            )
            .unwrap();
        account
            .commit(
                vec![AccountEvent::OrderFilled { trade }],
                CommandEntry::default(),
            )
            .unwrap();

        // The venue is the truth: balances become the snapshot; the fill
        // history stays — it is what the snapshot reconciles against.
        let snapshot = crate::VenueSnapshot {
            balances: [
                ("USD".to_string(), dec!(4_999.0)),
                ("BTC".to_string(), dec!(0.1)),
            ]
            .into_iter()
            .collect(),
            complete: true,
            anchor: None,
        };
        let records = account.stamp(vec![AccountEvent::Reconciled(snapshot)]);
        account.append(&records).unwrap();

        let state = account.state().unwrap();
        assert_eq!(state.balances["USD"], dec!(4_999.0));
        assert_eq!(state.filled_trades.len(), 1, "history survives");
    }

    #[test]
    fn reconciled_alone_never_initializes_an_account() {
        // Reconciliation refines an account; only Attached (or an epoch)
        // opens one.
        let dir = tempfile::tempdir().unwrap();
        let mut account = open(&dir);
        let snapshot = crate::VenueSnapshot {
            balances: std::collections::BTreeMap::new(),
            complete: false,
            anchor: None,
        };
        let records = account.stamp(vec![AccountEvent::Reconciled(snapshot)]);
        account.append(&records).unwrap();
        assert!(!account.is_initialized());
    }

    #[test]
    fn venue_snapshot_round_trips_digit_exact() {
        let snapshot = crate::VenueSnapshot {
            balances: [("USD".to_string(), dec!(9999.983091586))]
                .into_iter()
                .collect(),
            complete: false,
            // An anchor carrying digit-exact rates: the nested money must ride
            // the wire as strings too, or a re-recorded rate loses precision.
            anchor: Some(crate::WindowAnchor {
                equity: dec!(9999.983091586),
                currency: "USD".to_string(),
                fee_rate: dec!(0.0026),
                slippage_rate: dec!(0.001),
            }),
        };
        let record = AccountRecord {
            v: RECORD_VERSION,
            ts: "2026-01-01T00:00:00Z".parse().unwrap(),
            origin: Origin::Cli,
            event: AccountEvent::Attached(snapshot.clone()),
        };
        let json = serde_json::to_string(&record).unwrap();
        assert!(
            json.contains("\"9999.983091586\""),
            "money is a string on the wire: {json}"
        );
        assert!(
            json.contains("\"0.0026\"") && json.contains("\"0.001\""),
            "anchor rates are strings on the wire: {json}"
        );
        let back: AccountRecord = serde_json::from_str(&json).unwrap();
        let AccountEvent::Attached(decoded) = back.event else {
            panic!("wrong event kind");
        };
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn record_round_trips_through_serde() {
        let dir = tempfile::tempdir().unwrap();
        let account = open(&dir);
        let mut state = PaperState::default();
        state.apply(
            "2026-01-01T00:00:00Z".parse().unwrap(),
            &AccountEvent::Initialized(config(dec!(10_000.0))),
        );
        let trade = state
            .decide_market_order(
                OrderSide::Buy,
                "BTCUSD",
                dec!(0.1),
                dec!(50_000.0),
                dec!(49_900.0),
            )
            .unwrap();
        for record in account.stamp(vec![
            AccountEvent::OrderFilled { trade },
            AccountEvent::OrderRejected(crate::OrderRejection {
                side: OrderSide::Sell,
                pair: "BTCUSD".into(),
                volume: dec!(5.0),
                price: None,
                category: "validation".into(),
            }),
        ]) {
            let line = serde_json::to_string(&record).unwrap();
            assert!(!line.contains('\n'));
            let back: AccountRecord = serde_json::from_str(&line).unwrap();
            assert_eq!(
                serde_json::to_value(&back).unwrap(),
                serde_json::to_value(&record).unwrap()
            );
        }
    }

    /// A session marker's `session` stamp must survive the wire, and an
    /// unstamped entry must not serialize the key at all (old readers see no change).
    #[test]
    fn session_marker_command_entry_round_trips_and_stays_additive() {
        let marker = CommandEntry {
            name: "session start".into(),
            session: Some("s3".into()),
            ..CommandEntry::default()
        };
        let json = serde_json::to_string(&AccountEvent::Command(marker)).unwrap();
        assert!(
            json.contains("\"session\":\"s3\""),
            "marker carries its session: {json}"
        );
        let AccountEvent::Command(back) = serde_json::from_str(&json).unwrap() else {
            panic!("wrong event kind");
        };
        assert_eq!(back.session.as_deref(), Some("s3"));

        let plain = serde_json::to_string(&CommandEntry {
            name: "buy".into(),
            ..CommandEntry::default()
        })
        .unwrap();
        assert!(
            !plain.contains("\"session\""),
            "unstamped entries must not grow a key: {plain}"
        );
    }
}
