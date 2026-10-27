use crate::{
    CommaList, Delivery, KeyedRows, Pair,
    decimal::Wire,
    spot::{Endpoint, Public, sealed::Sealed, vocabulary::AssetClass},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize};
use serde_with::skip_serializing_none;

/// `GET /0/public/Ticker`. One asset class per request: Kraken scopes a
/// ticker query to a single class namespace.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct TickerQuery {
    /// Every pair of the class when empty.
    #[serde(skip_serializing_if = "CommaList::is_empty")]
    pub pair: CommaList<Pair>,
    pub asset_class: Option<AssetClass>,
}

/// Tickers keyed by Kraken's wire pair name (`XXBTZUSD`), as it answered.
pub type Tickers = KeyedRows<String, Ticker>;

/// A pair's ticker. "Today" is since 00:00 UTC; "24h" is rolling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ticker {
    pub ask_price: Decimal,
    pub ask_whole_lot_volume: Decimal,
    pub ask_lot_volume: Decimal,
    pub bid_price: Decimal,
    pub bid_whole_lot_volume: Decimal,
    pub bid_lot_volume: Decimal,
    pub last_price: Decimal,
    pub last_volume: Decimal,
    pub volume_today: Decimal,
    pub volume_24h: Decimal,
    pub vwap_today: Decimal,
    pub vwap_24h: Decimal,
    pub trades_today: u64,
    pub trades_24h: u64,
    pub low_today: Decimal,
    pub low_24h: Decimal,
    pub high_today: Decimal,
    pub high_24h: Decimal,
    pub open: Decimal,
}

/// Kraken's single-letter keys over positional arrays.
#[derive(Deserialize)]
struct WireTicker {
    a: [Wire; 3],
    b: [Wire; 3],
    c: [Wire; 2],
    v: [Wire; 2],
    p: [Wire; 2],
    t: [u64; 2],
    l: [Wire; 2],
    h: [Wire; 2],
    o: Wire,
}

impl<'de> Deserialize<'de> for Ticker {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let WireTicker {
            a:
                [
                    Wire(ask_price),
                    Wire(ask_whole_lot_volume),
                    Wire(ask_lot_volume),
                ],
            b:
                [
                    Wire(bid_price),
                    Wire(bid_whole_lot_volume),
                    Wire(bid_lot_volume),
                ],
            c: [Wire(last_price), Wire(last_volume)],
            v: [Wire(volume_today), Wire(volume_24h)],
            p: [Wire(vwap_today), Wire(vwap_24h)],
            t: [trades_today, trades_24h],
            l: [Wire(low_today), Wire(low_24h)],
            h: [Wire(high_today), Wire(high_24h)],
            o: Wire(open),
        } = WireTicker::deserialize(d)?;
        Ok(Self {
            ask_price,
            ask_whole_lot_volume,
            ask_lot_volume,
            bid_price,
            bid_whole_lot_volume,
            bid_lot_volume,
            last_price,
            last_volume,
            volume_today,
            volume_24h,
            vwap_today,
            vwap_24h,
            trades_today,
            trades_24h,
            low_today,
            low_24h,
            high_today,
            high_24h,
            open,
        })
    }
}

impl Sealed for TickerQuery {}
impl Endpoint for TickerQuery {
    const PATH: &'static str = "Ticker";
    type Access = Public;
    type Response = Tickers;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;
    use rust_decimal_macros::dec;

    #[test]
    fn positional_arrays_decode_into_named_fields() {
        let tickers: Tickers = decode_fixture("Ticker.json").unwrap();
        let btc = tickers.get(&"XXBTZUSD".to_owned()).unwrap();
        assert_eq!(btc.ask_price, dec!(81401.40000));
        assert_eq!(btc.trades_24h, 149364);
        assert_eq!(btc.open, dec!(81843.00000));
    }

    #[test]
    fn a_short_array_fails_with_its_field_path() {
        let json = r#"{"XXBTZUSD":{"a":["1","1"],"b":["1","1","1"],"c":["1","1"],"v":["1","1"],"p":["1","1"],"t":[1,1],"l":["1","1"],"h":["1","1"],"o":"1"}}"#;
        let err = serde_path_to_error::deserialize::<_, Tickers>(
            &mut serde_json::Deserializer::from_str(json),
        )
        .unwrap_err();
        assert_eq!(err.path().to_string(), "XXBTZUSD.a");
    }

    #[test]
    fn the_pair_list_is_comma_joined() {
        let query = TickerQuery {
            pair: vec!["XBTUSD".into(), "ETHUSD".into()].into(),
            asset_class: None,
        };
        assert_eq!(
            serde_json::to_string(&query).unwrap(),
            r#"{"pair":"XBTUSD,ETHUSD"}"#
        );
    }
}
