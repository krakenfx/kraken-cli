//! Deterministic paper execution against a recorded tape.
//!
//! The journal lock serializes lazy fills, while the pump writes a separate
//! session tape. A cursor only prevents replaying already-decided frames.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use kraken_lab::mark;
use kraken_paper::account::PaperAccount;
use kraken_paper::{AccountEvent, PaperState};
use kraken_recording::{Source, TapeRef, TapeSource, write_json_atomic};
use kraken_session::manifest::SessionSource;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::errors::{KrakenError, Result};

/// The venue cursor's file name inside a session directory.
pub(crate) const VENUE_CURSOR_FILE: &str = "venue_cursor.json";

/// A replay session's offline venue: the affine map, the source tape, and
/// the mark table the fold warms. Live runs never construct one.
pub(crate) struct ReplayVenue {
    source: SessionSource,
    source_path: PathBuf,
    cursor_path: PathBuf,
    table: mark::Table,
    /// Wall-clock instant the fold must not advance past — a closed
    /// window's recorded end. `None` while the session is live: fold to now.
    ceiling: Option<DateTime<Utc>>,
}

/// Last folded tape position, in source-tape coordinates. Ordering matches
/// the reader's `(at, seq)` yield order.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Cursor {
    at: DateTime<Utc>,
    seq: i64,
}

impl ReplayVenue {
    /// `Some` iff the scope's ACTIVE run replays a tape (its session.json
    /// carries a source block). No active session, or a live-market run, is a
    /// plain REST venue.
    pub(crate) fn resolve_active(scope: &Path) -> Result<Option<Self>> {
        let Some((session_id, manifest)) = kraken_workspace::session::active(scope)? else {
            return Ok(None);
        };
        let Some(source) = manifest.source else {
            return Ok(None);
        };
        // An active session's window is open, so no ceiling: fold to now.
        Self::for_run(
            &crate::config::config_dir()?,
            scope,
            &kraken_session::session::session_dir(scope, session_id.ordinal()),
            source,
            manifest.window.ended_at,
        )
        .map(Some)
    }

    /// The venue for a session whose source block is already in hand
    /// (run stop/show, which have loaded session.json anyway). The cursor lives
    /// in the session's own directory — one cursor per window, never shared.
    pub(crate) fn for_run(
        library: &Path,
        scope: &Path,
        session_dir: &Path,
        source: SessionSource,
        ceiling: Option<DateTime<Utc>>,
    ) -> Result<Self> {
        let tape: TapeRef = source.tape.parse()?;
        let source_path = kraken_replay::resolve_source_path(library, scope, &tape)?;
        Ok(Self {
            source,
            source_path,
            cursor_path: session_dir.join(VENUE_CURSOR_FILE),
            table: mark::Table::default(),
            ceiling,
        })
    }

    /// Fold the source tape up to the current session instant: every due
    /// frame warms the marks; frames past the cursor also decide resting
    /// fills, journaled at their session-time instants (tape-time
    /// attribution stays derivable through the affine map).
    pub(crate) fn fold_due(&mut self, account: &mut PaperAccount) -> Result<()> {
        let due = self.source.tape_time(self.ceiling.unwrap_or_else(Utc::now));
        let cursor: Option<Cursor> = read_cursor(&self.cursor_path)?;
        let reader = TapeSource::open(&self.source_path)?;
        let mut last = cursor;
        for event in reader.read()? {
            if event.at > due {
                break;
            }
            let moved = self.table.observe(&event.frame);
            let past_cursor = cursor.is_none_or(|c| (event.at, event.seq) > (c.at, c.seq));
            if past_cursor {
                if moved {
                    self.settle_fills(account, self.source.session_time(event.at))?;
                }
                last = Some(Cursor {
                    at: event.at,
                    seq: event.seq,
                });
            }
        }
        // An order placed after the last mark move can already cross the
        // standing marks — the live venue would fill it on the next
        // reconcile, so the fold settles once more at the due instant.
        self.settle_fills(account, self.source.session_time(due))?;
        if last != cursor {
            write_json_atomic(&self.cursor_path, &last)?;
        }
        Ok(())
    }

    /// Decide and journal any fills the current marks trigger, stamped `at`.
    fn settle_fills(&self, account: &mut PaperAccount, at: DateTime<Utc>) -> Result<()> {
        if !account.is_initialized() {
            return Ok(());
        }
        let state = account.state()?;
        // The vast majority of frames settle nothing (market-order and DCA
        // strategies rest no orders). Skip building the per-symbol price map —
        // one allocation and a `format!` per mark — when there is nothing that
        // could fill.
        if state.open_orders.is_empty() {
            return Ok(());
        }
        let prices = self.table.to_prices(&state.starting_currency);
        let fills = state.decide_pending_fills(&prices);
        if fills.is_empty() {
            return Ok(());
        }
        let events = fills
            .into_iter()
            .map(|mut trade| {
                trade.filled_at = at;
                AccountEvent::OrderFilled { trade }
            })
            .collect();
        let records = account.stamp_at(events, at);
        Ok(account.append(&records)?)
    }

    /// The pair's current tape mark as `(ask, bid)`. Mid both sides — the
    /// top-of-book proxy the scorecard's replay caveats disclose.
    pub(crate) fn ticker_price(&self, pair: &str) -> Result<(Decimal, Decimal)> {
        let (base, quote) = kraken_paper::canonical_key(pair).ok_or_else(|| {
            KrakenError::Validation(format!("Unrecognized trading pair '{pair}'"))
        })?;
        self.table
            .to_prices(&quote)
            .get(&format!("{base}{quote}"))
            .copied()
            .ok_or_else(|| {
                KrakenError::Validation(format!(
                    "no recorded {pair} mark at this tape position yet; wait for the replay \
                     to reach a price frame (source {})",
                    self.source.tape
                ))
            })
    }

    /// Value the account from the folded marks — never REST: a live price
    /// against a recorded tape would be a lie.
    pub(crate) fn value(&self, state: &PaperState) -> (Decimal, bool) {
        let prices = self.table.to_prices(&state.starting_currency);
        state.compute_portfolio_value(&prices)
    }
}

fn read_cursor(path: &Path) -> Result<Option<Cursor>> {
    match std::fs::read_to_string(path) {
        Ok(data) => Ok(serde_json::from_str(&data)
            .map_err(|e| KrakenError::Parse(format!("damaged venue cursor: {e}")))?),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use kraken_paper::account::Origin;
    use kraken_paper::{OrderSide, PaperConfig};
    use rust_decimal_macros::dec;

    use super::*;

    fn ticker_line(ts: &str, ask: f64, bid: f64) -> String {
        serde_json::json!({
            "channel": "ticker", "type": "update",
            "data": [{
                "symbol": "BTC/USD", "bid": bid, "bid_qty": 1.0, "ask": ask, "ask_qty": 1.0,
                "last": bid, "volume": 100.0, "vwap": bid, "low": bid, "high": ask,
                "change": 0.0, "change_pct": 0.0, "timestamp": ts
            }]
        })
        .to_string()
    }

    /// A finalized three-frame BTC/USD tape: 50k → 48k → 52k.
    fn write_tape(base: &Path) {
        let dir = kraken_recording::tapes_root(base);
        std::fs::create_dir_all(&dir).unwrap();
        let lines = [
            ticker_line("2026-01-01T00:00:00Z", 50_000.0, 50_000.0),
            ticker_line("2026-01-01T00:00:10Z", 48_000.0, 48_000.0),
            ticker_line("2026-01-01T00:00:20Z", 52_000.0, 52_000.0),
        ]
        .join("\n");
        std::fs::write(dir.join("tape.jsonl"), lines + "\n").unwrap();
        std::fs::write(
            dir.join("tape.jsonl.meta.json"),
            serde_json::json!({
                "schema_version": kraken_recording::schema::SCHEMA_VERSION,
                "source": "kraken-spot-ws-v2",
                "symbols": ["BTC/USD"],
                "channels": ["ticker"],
                "window_start": "2026-01-01T00:00:00Z",
                "cli_version": "0.0.0-test",
                "window_end": "2026-01-01T00:00:20Z",
                "events_dropped": 0,
                "frames_unparsed": 0,
                "reconnect_count": 0
            })
            .to_string(),
        )
        .unwrap();
    }

    /// A session whose whole tape is already due: `started_at` sits an hour
    /// in the past, so `tape_time(now)` is far beyond the last frame.
    fn venue_and_account(base: &Path) -> (ReplayVenue, PaperAccount) {
        let session_dir = kraken_session::session::session_dir(base, 1);
        std::fs::create_dir_all(&session_dir).unwrap();
        let source = SessionSource {
            tape: "tape:tape".to_string(),
            speed: 1.0,
            anchor: "2026-01-01T00:00:00Z".parse().unwrap(),
            started_at: Utc::now() - Duration::hours(1),
            content_hash: None,
        };
        let venue = ReplayVenue::for_run(base, base, &session_dir, source, None).unwrap();
        let mut account = PaperAccount::open_at(base.join("journal.jsonl"), Origin::Cli).unwrap();
        let init = AccountEvent::Initialized(PaperConfig {
            balance: dec!(10_000),
            currency: "USD".to_string(),
            fee_rate: dec!(0),
            slippage_rate: dec!(0),
        });
        let records = account.stamp(vec![init]);
        account.append_if_uninitialized(&records).unwrap();
        (venue, account)
    }

    #[test]
    fn fold_settles_a_crossing_limit_order_at_tape_time_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        write_tape(base);
        let (mut venue, mut account) = venue_and_account(base);

        // Resting buy at 49k: the 48k frame crosses it; the 52k one must not
        // refill it.
        let order = account
            .state()
            .unwrap()
            .decide_limit_order(OrderSide::Buy, "BTC/USD", dec!(0.1), dec!(49_000))
            .unwrap();
        let records = account.stamp(vec![AccountEvent::OrderSubmitted { order }]);
        account.append(&records).unwrap();

        venue.fold_due(&mut account).unwrap();
        let state = account.state().unwrap();
        assert_eq!(state.filled_trades.len(), 1, "one fill from the 48k frame");
        let fill = &state.filled_trades[0];
        assert_eq!(fill.volume, dec!(0.1));
        // The fill lands at the frame's session-time instant, not the fold's
        // wall clock: 10 tape seconds after started_at.
        let expected = venue
            .source
            .session_time("2026-01-01T00:00:10Z".parse().unwrap());
        assert_eq!(fill.filled_at, expected);
    }

    #[test]
    fn refolding_never_duplicates_fills() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        write_tape(base);
        let (mut venue, mut account) = venue_and_account(base);
        let order = account
            .state()
            .unwrap()
            .decide_limit_order(OrderSide::Buy, "BTC/USD", dec!(0.1), dec!(49_000))
            .unwrap();
        let records = account.stamp(vec![AccountEvent::OrderSubmitted { order }]);
        account.append(&records).unwrap();
        venue.fold_due(&mut account).unwrap();
        assert_eq!(account.state().unwrap().filled_trades.len(), 1);

        // Fresh venue (as a new command process would build): the cursor
        // keeps folded frames folded.
        let mut venue = ReplayVenue::for_run(
            base,
            base,
            &kraken_session::session::session_dir(base, 1),
            SessionSource {
                tape: "tape:tape".to_string(),
                speed: 1.0,
                anchor: "2026-01-01T00:00:00Z".parse().unwrap(),
                started_at: Utc::now() - Duration::hours(1),
                content_hash: None,
            },
            None,
        )
        .unwrap();
        venue.fold_due(&mut account).unwrap();
        assert_eq!(
            account.state().unwrap().filled_trades.len(),
            1,
            "cursor prevents re-deciding folded frames"
        );
    }

    #[test]
    fn a_finalized_session_does_not_fold_past_its_recorded_end() {
        // The bug: after stop, a paper command re-folds up to tape_time(now)
        // and journals a fill from a frame the session never reached. The ceiling
        // (the session's recorded end) must cap the fold so the sealed journal
        // stays reproducible.
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        write_tape(base);
        let session_dir = kraken_session::session::session_dir(base, 1);
        std::fs::create_dir_all(&session_dir).unwrap();
        let started_at = Utc::now() - Duration::hours(1);
        let source = SessionSource {
            tape: "tape:tape".to_string(),
            speed: 1.0,
            anchor: "2026-01-01T00:00:00Z".parse().unwrap(),
            started_at,
            content_hash: None,
        };
        // Cap 5 tape-seconds in — before the t+10s (48k) frame that crosses.
        let ceiling = Some(started_at + Duration::seconds(5));
        let mut venue = ReplayVenue::for_run(base, base, &session_dir, source, ceiling).unwrap();
        let mut account = PaperAccount::open_at(base.join("journal.jsonl"), Origin::Cli).unwrap();
        let init = AccountEvent::Initialized(PaperConfig {
            balance: dec!(10_000),
            currency: "USD".to_string(),
            fee_rate: dec!(0),
            slippage_rate: dec!(0),
        });
        let records = account.stamp(vec![init]);
        account.append_if_uninitialized(&records).unwrap();
        let order = account
            .state()
            .unwrap()
            .decide_limit_order(OrderSide::Buy, "BTC/USD", dec!(0.1), dec!(49_000))
            .unwrap();
        let records = account.stamp(vec![AccountEvent::OrderSubmitted { order }]);
        account.append(&records).unwrap();

        venue.fold_due(&mut account).unwrap();

        assert!(
            account.state().unwrap().filled_trades.is_empty(),
            "the 48k frame is past the ceiling, so the resting buy must not fill"
        );
    }

    #[test]
    fn ticker_price_is_the_folded_mark_and_absent_marks_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        write_tape(base);
        let (mut venue, mut account) = venue_and_account(base);

        assert!(
            venue.ticker_price("BTC/USD").is_err(),
            "no fold yet, no mark"
        );
        venue.fold_due(&mut account).unwrap();
        let (ask, bid) = venue.ticker_price("BTC/USD").unwrap();
        // Mid both sides — the top-of-book proxy. The mark rides the same
        // 3-observation median guard as the scoring fold: median(50k, 48k,
        // 52k) = 50k, so pricing and scoring can never disagree on a mark.
        assert_eq!((ask, bid), (dec!(50_000), dec!(50_000)));
        assert!(venue.ticker_price("ETH/USD").is_err());
    }

    #[test]
    fn value_marks_holdings_from_the_tape_never_rest() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        write_tape(base);
        let (mut venue, mut account) = venue_and_account(base);
        let order = account
            .state()
            .unwrap()
            .decide_limit_order(OrderSide::Buy, "BTC/USD", dec!(0.1), dec!(49_000))
            .unwrap();
        let records = account.stamp(vec![AccountEvent::OrderSubmitted { order }]);
        account.append(&records).unwrap();
        venue.fold_due(&mut account).unwrap();

        let (value, complete) = venue.value(account.state().unwrap());
        // The fill costs 0.1 × 49k (the two-observation warm-up mean when the
        // 48k frame crossed); the end mark is the 3-frame median 50k:
        // 10_000 − 4_900 + 0.1 × 50_000 = 10_100.
        assert_eq!(value, dec!(10_100));
        assert!(complete);
    }
}
