//! Exact (de)serialization for `Decimal` fields that cross the wire boundary as
//! money.
//!
//! The crate-wide `serde-float` default routes `Decimal` through `f64`, which
//! silently rounds past ~17 significant digits — tolerable on ingest (forced by
//! the flattened envelope, see the manifest note on `arbitrary_precision`),
//! never for money we place on the wire. The serializers emit the decimal's own
//! digits as a raw JSON number; the deserializers capture the raw token and let
//! `Decimal` parse its own digits, so user-supplied JSON (`--orders`) is never
//! rounded before the exact serializers run.
//!
//! The deserializers require serde_json's native str/slice deserializer
//! (`RawValue` does not survive `serde_json::Value` or a `#[serde(flatten)]`
//! envelope) — fine for the request types, which are parsed straight from user
//! JSON text.

use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::Error as _};

pub(super) fn serialize<S: Serializer>(value: &Decimal, serializer: S) -> Result<S::Ok, S::Error> {
    // `Decimal`'s `Display` form is a plain decimal literal, i.e. a valid JSON
    // number.
    serde_json::value::RawValue::from_string(value.to_string())
        .map_err(S::Error::custom)?
        .serialize(serializer)
}

pub(super) fn opt<S: Serializer>(
    value: &Option<Decimal>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(decimal) => serialize(decimal, serializer),
        None => serializer.serialize_none(),
    }
}

pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Decimal, D::Error> {
    let raw = Box::<serde_json::value::RawValue>::deserialize(deserializer)?;
    parse_token(raw.get()).map_err(D::Error::custom)
}

pub(super) fn de_opt<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Decimal>, D::Error> {
    let raw = Option::<Box<serde_json::value::RawValue>>::deserialize(deserializer)?;
    raw.map(|raw| parse_token(raw.get()).map_err(D::Error::custom))
        .transpose()
}

/// Parse one raw JSON token as a decimal: a bare number (plain or scientific
/// notation, which `from_str` alone rejects) or the same value quoted as a
/// string.
fn parse_token(token: &str) -> Result<Decimal, String> {
    let digits = if token.starts_with('"') {
        serde_json::from_str::<String>(token).map_err(|e| format!("invalid string: {e}"))?
    } else {
        token.to_string()
    };
    Decimal::from_str_exact(&digits)
        .or_else(|_| Decimal::from_scientific(&digits))
        .map_err(|e| format!("`{digits}` is not a decimal: {e}"))
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize)]
    struct Order {
        #[serde(serialize_with = "super::serialize")]
        qty: Decimal,
        #[serde(serialize_with = "super::opt")]
        price: Option<Decimal>,
    }

    #[derive(Debug, Deserialize)]
    struct Incoming {
        #[serde(deserialize_with = "super::deserialize")]
        qty: Decimal,
        #[serde(default, deserialize_with = "super::de_opt")]
        price: Option<Decimal>,
    }

    #[test]
    fn a_qty_beyond_f64_precision_serializes_its_exact_digits() {
        // 18 significant digits: the f64 route would emit 0.12345678901234568.
        let order = Order {
            qty: "0.123456789012345678".parse().expect("a valid decimal"),
            price: Some("1234.567890123456789".parse().expect("a valid decimal")),
        };
        assert_eq!(
            serde_json::to_string(&order).expect("serializes"),
            r#"{"qty":0.123456789012345678,"price":1234.567890123456789}"#
        );
    }

    #[test]
    fn a_qty_beyond_f64_precision_deserializes_its_exact_digits() {
        // The decode-side mirror: a bare number and a quoted string both keep all 18/19
        // digits (the f64 route would decode 0.12345678901234568).
        let incoming: Incoming =
            serde_json::from_str(r#"{"qty":0.123456789012345678,"price":"1234.567890123456789"}"#)
                .expect("both token forms decode");
        assert_eq!(incoming.qty.to_string(), "0.123456789012345678");
        assert_eq!(
            incoming.price.expect("present").to_string(),
            "1234.567890123456789"
        );
    }

    #[test]
    fn a_scientific_notation_token_still_decodes() {
        // JSON permits exponent form; `Decimal::from_str` alone rejects it.
        let incoming: Incoming = serde_json::from_str(r#"{"qty":1.5e2}"#).expect("decodes");
        assert_eq!(incoming.qty, Decimal::from(150));
        assert!(incoming.price.is_none(), "a missing field stays None");
    }

    #[test]
    fn a_non_decimal_token_names_itself_in_the_error() {
        let err = serde_json::from_str::<Incoming>(r#"{"qty":"not-money"}"#).unwrap_err();
        assert!(err.to_string().contains("not-money"), "got: {err}");
    }

    proptest::proptest! {
        /// The exactness invariant for ANY representable Decimal, not just the pinned
        /// examples: serialize -> raw JSON number -> deserialize preserves every digit.
        #[test]
        fn any_decimal_round_trips_digit_exact(
            mantissa in -79_228_162_514_264_337_593_543_950_335i128
                ..=79_228_162_514_264_337_593_543_950_335i128,
            scale in 0u32..=28,
        ) {
            let value = Decimal::from_i128_with_scale(mantissa, scale);
            let json = serde_json::to_string(&Order { qty: value, price: None })
                .expect("serializes");
            let back: Incoming = serde_json::from_str(&json).expect("round-trips");
            proptest::prop_assert_eq!(back.qty.to_string(), value.to_string());
        }
    }
}
