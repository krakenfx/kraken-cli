//! Decimal (de)serialization. Kraken sends money as decimal strings, a few
//! fields as JSON numbers; both decode exactly into [`Decimal`]. How a value is
//! published is the field's choice: [`number`] (the workspace's `serde-float`
//! form) or [`string`] (digit for digit, trailing zeros kept).

use rust_decimal::Decimal;
use serde::{Deserializer, Serializer, de};
use std::{fmt, str::FromStr};

struct DecimalVisitor;

impl de::Visitor<'_> for DecimalVisitor {
    type Value = Decimal;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a decimal string or number")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Decimal, E> {
        Decimal::from_str(v).map_err(|e| E::custom(format!("{v:?} is not a decimal: {e}")))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Decimal, E> {
        Ok(v.into())
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Decimal, E> {
        Ok(v.into())
    }

    /// Through the shortest round-trip form, so `0.99` decodes as written
    /// rather than as its binary expansion. An integral float keeps one place
    /// (`5000000.0`), so [`json`] republishes it as the float Kraken sent.
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Decimal, E> {
        let text = v.to_string();
        if v.is_finite() && !text.contains('.') {
            return self.visit_str(&format!("{text}.0"));
        }
        self.visit_str(&text)
    }
}

fn decode<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Decimal, D::Error> {
    deserializer.deserialize_any(DecimalVisitor)
}

/// A decimal inside a positional wire array (`["81401.4", "1", "1.000"]`).
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(transparent)]
pub(crate) struct Wire(#[serde(with = "number")] pub(crate) Decimal);

struct OptionVisitor;

impl<'de> de::Visitor<'de> for OptionVisitor {
    type Value = Option<Decimal>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a decimal string, number or null")
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        decode(deserializer).map(Some)
    }
}

fn decode_option<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Decimal>, D::Error> {
    deserializer.deserialize_option(OptionVisitor)
}

/// Published as a JSON number.
pub(crate) mod number {
    use super::*;
    use serde::Serialize;

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Decimal, D::Error> {
        decode(d)
    }

    pub(crate) fn serialize<S: Serializer>(value: &Decimal, s: S) -> Result<S::Ok, S::Error> {
        Serialize::serialize(value, s)
    }

    pub(crate) mod option {
        use super::*;

        pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<Decimal>, D::Error> {
            decode_option(d)
        }

        pub(crate) fn serialize<S: Serializer>(
            value: &Option<Decimal>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            Serialize::serialize(value, s)
        }
    }
}

/// Published as the JSON number Kraken Futures sent: an integer as an integer
/// (`81346`), a float as a float (`0.0001`, `5000000.0`). The workspace's
/// `serde-float` form would re-encode `81346` as `81346.0`.
pub(crate) mod json {
    use super::*;
    use rust_decimal::prelude::ToPrimitive as _;

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Decimal, D::Error> {
        decode(d)
    }

    pub(crate) fn serialize<S: Serializer>(value: &Decimal, s: S) -> Result<S::Ok, S::Error> {
        if value.scale() == 0
            && let Some(integer) = value.to_i64()
        {
            return s.serialize_i64(integer);
        }
        match value.to_f64() {
            Some(float) => s.serialize_f64(float),
            None => Err(serde::ser::Error::custom(format!(
                "{value} has no JSON number form"
            ))),
        }
    }

    pub(crate) mod option {
        use super::*;

        pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<Decimal>, D::Error> {
            decode_option(d)
        }

        pub(crate) fn serialize<S: Serializer>(
            value: &Option<Decimal>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            match value {
                Some(value) => super::serialize(value, s),
                None => s.serialize_none(),
            }
        }
    }
}

/// Published as the decimal string, digit for digit.
pub(crate) mod string {
    use super::*;

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Decimal, D::Error> {
        decode(d)
    }

    pub(crate) fn serialize<S: Serializer>(value: &Decimal, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(value)
    }

    pub(crate) mod option {
        use super::*;

        pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<Decimal>, D::Error> {
            decode_option(d)
        }

        pub(crate) fn serialize<S: Serializer>(
            value: &Option<Decimal>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            match value {
                Some(value) => s.collect_str(value),
                None => s.serialize_none(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize)]
    struct Exact(#[serde(with = "string")] Decimal);

    #[test]
    fn a_float_on_the_wire_decodes_as_written() {
        let Exact(value) = serde_json::from_str("0.99").unwrap();
        assert_eq!(value.to_string(), "0.99");
    }

    #[test]
    fn a_non_decimal_string_names_the_value() {
        let err = serde_json::from_str::<Exact>(r#""1,5""#).unwrap_err();
        assert!(
            err.to_string().contains(r#""1,5" is not a decimal"#),
            "{err}"
        );
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct Futures(#[serde(with = "json")] Decimal);

    #[test]
    fn a_futures_integer_stays_an_integer() {
        let price: Futures = serde_json::from_str("81346").unwrap();
        assert_eq!(serde_json::to_string(&price).unwrap(), "81346");
        let size: Futures = serde_json::from_str("0.0001").unwrap();
        assert_eq!(serde_json::to_string(&size).unwrap(), "0.0001");
    }

    #[test]
    fn a_futures_integral_float_stays_a_float() {
        for wire in ["5000000.0", "0.0", "-2.0"] {
            let volume: Futures = serde_json::from_str(wire).unwrap();
            assert_eq!(serde_json::to_string(&volume).unwrap(), wire);
        }
    }

    proptest! {
        #[test]
        fn a_decimal_string_round_trips_digit_for_digit(mantissa in any::<i64>(), scale in 0u32..=18) {
            let wire = serde_json::to_string(&Decimal::new(mantissa, scale).to_string()).unwrap();
            let decoded: Exact = serde_json::from_str(&wire).unwrap();
            prop_assert_eq!(serde_json::to_string(&decoded).unwrap(), wire);
        }
    }
}
