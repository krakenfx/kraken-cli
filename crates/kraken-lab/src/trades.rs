//! FIFO round-trip attribution over replay fills.
//!
//! Fees are allocated pro rata and open lots are excluded. Sign-flip
//! heuristics misgrade scaled entries, so sells close the oldest buy lots.

use std::collections::{HashMap, VecDeque};

use kraken_paper::{OrderSide, PaperTrade};
use rust_decimal::Decimal;

/// One FIFO-closed round trip. `net_pnl` is quoted in the pair's own quote
/// currency — cross-currency sums are the caller's judgment call.
#[derive(Debug)]
pub(crate) struct RoundTrip {
    pub(crate) quote: String,
    pub(crate) net_pnl: Decimal,
}

/// Match `fills` (journal order) into closed round trips. Open remainders —
/// buy lots never sold — are not graded; spot paper cannot short, so a sell
/// tail without an open lot is ignored rather than fabricated into a trip.
pub(crate) fn round_trips(fills: &[PaperTrade]) -> Vec<RoundTrip> {
    struct Lot {
        volume: Decimal,
        price: Decimal,
        fee_per_unit: Decimal,
    }
    let mut open: HashMap<&str, VecDeque<Lot>> = HashMap::new();
    let mut trips = Vec::new();
    for fill in fills {
        if fill.volume <= Decimal::ZERO {
            continue;
        }
        let fee_per_unit = fill.fee / fill.volume;
        match fill.side {
            OrderSide::Buy => open.entry(fill.pair.as_str()).or_default().push_back(Lot {
                volume: fill.volume,
                price: fill.price,
                fee_per_unit,
            }),
            OrderSide::Sell => {
                let lots = open.entry(fill.pair.as_str()).or_default();
                let mut remaining = fill.volume;
                while remaining > Decimal::ZERO {
                    let Some(lot) = lots.front_mut() else {
                        break;
                    };
                    let matched = remaining.min(lot.volume);
                    let net_pnl =
                        (fill.price - lot.price - lot.fee_per_unit - fee_per_unit) * matched;
                    trips.push(RoundTrip {
                        quote: fill.quote.clone(),
                        net_pnl,
                    });
                    lot.volume -= matched;
                    remaining -= matched;
                    if lot.volume == Decimal::ZERO {
                        lots.pop_front();
                    }
                }
            }
        }
    }
    trips
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use rust_decimal_macros::dec;

    use super::*;

    fn fill(side: OrderSide, volume: Decimal, price: Decimal, fee: Decimal) -> PaperTrade {
        PaperTrade {
            id: "t".into(),
            order_id: "o".into(),
            pair: "BTC/USD".into(),
            base: "BTC".into(),
            quote: "USD".into(),
            side,
            volume,
            price,
            fee,
            cost: volume * price,
            filled_at: Utc::now(),
            reference_quote: None,
        }
    }

    #[test]
    fn partial_close_grades_only_the_sold_portion() {
        // The DCA shape: two buys, one sell smaller than the first lot.
        let fills = [
            fill(OrderSide::Buy, dec!(0.2), dec!(100), dec!(0)),
            fill(OrderSide::Buy, dec!(0.1), dec!(110), dec!(0)),
            fill(OrderSide::Sell, dec!(0.25), dec!(120), dec!(0)),
        ];
        let trips = round_trips(&fills);
        assert_eq!(trips.len(), 2, "one full lot + one partial lot closed");
        assert_eq!(trips[0].net_pnl, dec!(4)); // (120-100) × 0.2
        assert_eq!(trips[1].net_pnl, dec!(0.5)); // (120-110) × 0.05
    }

    #[test]
    fn fees_allocate_pro_rata_and_can_flip_a_win_to_a_loss() {
        // Price gain 1 × 0.1 = 0.1; fees 0.08 (buy, all matched) + 0.04
        // (sell) = 0.12 — a gross win graded as a net loss.
        let fills = [
            fill(OrderSide::Buy, dec!(0.1), dec!(100), dec!(0.08)),
            fill(OrderSide::Sell, dec!(0.1), dec!(101), dec!(0.04)),
        ];
        let trips = round_trips(&fills);
        assert_eq!(trips.len(), 1);
        assert_eq!(trips[0].net_pnl, dec!(-0.02));
    }

    #[test]
    fn buys_only_close_nothing() {
        let fills = [
            fill(OrderSide::Buy, dec!(0.1), dec!(100), dec!(0)),
            fill(OrderSide::Buy, dec!(0.1), dec!(90), dec!(0)),
        ];
        assert!(round_trips(&fills).is_empty());
    }

    /// Spot paper cannot short, so a sell with no open lot (an epoch that
    /// began holding assets it never bought) grades nothing rather than
    /// fabricating a trip.
    #[test]
    fn sell_without_an_open_lot_grades_nothing() {
        let fills = [
            fill(OrderSide::Sell, dec!(0.1), dec!(100), dec!(0)),
            fill(OrderSide::Buy, dec!(0.2), dec!(90), dec!(0)),
            // Closes the 0.2 lot fully; the 0.1 tail has nothing to match.
            fill(OrderSide::Sell, dec!(0.3), dec!(95), dec!(0)),
        ];
        let trips = round_trips(&fills);
        assert_eq!(trips.len(), 1, "only the bought lot is graded");
        assert_eq!(trips[0].net_pnl, dec!(1)); // (95-90) × 0.2
    }

    #[test]
    fn pairs_match_independently() {
        let mut eth = fill(OrderSide::Buy, dec!(1), dec!(10), dec!(0));
        eth.pair = "ETH/USD".into();
        let fills = [
            fill(OrderSide::Buy, dec!(0.1), dec!(100), dec!(0)),
            eth,
            fill(OrderSide::Sell, dec!(0.1), dec!(120), dec!(0)),
        ];
        let trips = round_trips(&fills);
        assert_eq!(trips.len(), 1, "the ETH lot stays open");
        assert_eq!(trips[0].net_pnl, dec!(2));
    }
}
