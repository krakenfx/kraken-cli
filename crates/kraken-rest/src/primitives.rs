//! Value objects shared across endpoints.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeMap as _};
use std::{fmt, marker::PhantomData};

/// An asset code in its modern form: `XXBT` decodes as `BTC`, `ZUSD` as `USD`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Asset(String);

impl Asset {
    /// Maps Kraken's legacy X/Z-prefixed codes to their modern form. A table,
    /// not a prefix strip: XRP, XLM and ZEC are modern codes that start with X
    /// or Z.
    #[must_use]
    pub fn normalise(raw: &str) -> &str {
        match raw {
            "XXBT" | "XBT" => "BTC",
            "XETC" => "ETC",
            "XETH" => "ETH",
            "XLTC" => "LTC",
            "XMLN" => "MLN",
            "XREP" => "REP",
            "XXDG" => "XDG",
            "XXLM" => "XLM",
            "XXMR" => "XMR",
            "XXRP" => "XRP",
            "XZEC" => "ZEC",
            "ZARS" => "ARS",
            "ZAUD" => "AUD",
            "ZCAD" => "CAD",
            "ZCLP" => "CLP",
            "ZCOP" => "COP",
            "ZDKK" => "DKK",
            "ZEUR" => "EUR",
            "ZGBP" => "GBP",
            "ZGEL" => "GEL",
            "ZGHS" => "GHS",
            "ZJPY" => "JPY",
            "ZLKR" => "LKR",
            "ZMXN" => "MXN",
            "ZPLN" => "PLN",
            "ZSEK" => "SEK",
            "ZUGX" => "UGX",
            "ZUSD" => "USD",
            "ZVND" => "VND",
            "ZXOF" => "XOF",
            "KFEE" => "FEE",
            other => other,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Asset {
    fn from(raw: &str) -> Self {
        Self(Self::normalise(raw).to_owned())
    }
}

impl<'de> Deserialize<'de> for Asset {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(|raw| Self::from(raw.as_str()))
    }
}

impl fmt::Display for Asset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A trading pair as the caller spells it (`BTC/USD`, `XBTUSD`, `AAPLx/USD`).
/// Kept verbatim: case carries meaning for tokenized assets.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Pair(String);

impl Pair {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for Pair {
    fn from(pair: String) -> Self {
        Self(pair)
    }
}

impl From<&str> for Pair {
    fn from(pair: &str) -> Self {
        Self(pair.to_owned())
    }
}

impl From<Pair> for String {
    fn from(pair: Pair) -> Self {
        pair.0
    }
}

impl fmt::Display for Pair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A Unix timestamp published as its wire token, `"1688667796.8802"`. Kraken
/// sends seconds as a number with a fractional part; the published string is
/// never an f64 re-encoding of a decimal the caller cannot see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct UnixTime(String);

impl UnixTime {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whole seconds and nanoseconds since the epoch, exact to the sent digits;
    /// `None` for a token that is not a time.
    #[must_use]
    pub fn parts(&self) -> Option<(i64, u32)> {
        use rust_decimal::prelude::ToPrimitive as _;
        let seconds: rust_decimal::Decimal = self.0.parse().ok()?;
        let nanos = (seconds.fract() * rust_decimal::Decimal::from(1_000_000_000)).to_u32()?;
        Some((seconds.trunc().to_i64()?, nanos))
    }
}

impl<'de> Deserialize<'de> for UnixTime {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Number(serde_json::Number),
            Text(String),
        }
        Ok(Self(match Wire::deserialize(d)? {
            Wire::Number(n) => n.to_string(),
            Wire::Text(s) => s,
        }))
    }
}

impl fmt::Display for UnixTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A JSON object of rows, in the order Kraken sent them. Keyed history arrives
/// newest-first, and entries sharing a timestamp have no other ordering signal,
/// so a hash map would lose it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyedRows<K, V>(Vec<(K, V)>);

impl<K, V> KeyedRows<K, V> {
    pub fn iter(&self) -> std::slice::Iter<'_, (K, V)> {
        self.0.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.0.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.0.iter().map(|(_, v)| v)
    }
}

impl<K: PartialEq, V> KeyedRows<K, V> {
    pub fn get(&self, key: &K) -> Option<&V> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

impl<K, V> Default for KeyedRows<K, V> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<K, V> FromIterator<(K, V)> for KeyedRows<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<K, V> IntoIterator for KeyedRows<K, V> {
    type Item = (K, V);
    type IntoIter = std::vec::IntoIter<(K, V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, K, V> IntoIterator for &'a KeyedRows<K, V> {
    type Item = &'a (K, V);
    type IntoIter = std::slice::Iter<'a, (K, V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<K: Serialize, V: Serialize> Serialize for KeyedRows<K, V> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<'de, K: Deserialize<'de>, V: Deserialize<'de>> Deserialize<'de> for KeyedRows<K, V> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct InOrder<K, V>(PhantomData<(K, V)>);

        impl<'de, K: Deserialize<'de>, V: Deserialize<'de>> de::Visitor<'de> for InOrder<K, V> {
            type Value = KeyedRows<K, V>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object of rows")
            }

            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut rows = Vec::with_capacity(map.size_hint().unwrap_or(0));
                while let Some(entry) = map.next_entry()? {
                    rows.push(entry);
                }
                Ok(KeyedRows(rows))
            }
        }

        d.deserialize_map(InOrder(PhantomData))
    }
}

/// An identifier Kraken assigns to a record of kind `T`: `Id<Order>` is a
/// txid, `Id<LedgerEntry>` a ledger id. The kind keeps one id from being
/// passed where another is expected.
pub struct Id<T> {
    id: String,
    kind: PhantomData<fn() -> T>,
}

impl<T> Id<T> {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.id
    }
}

impl<T> From<String> for Id<T> {
    fn from(id: String) -> Self {
        Self {
            id,
            kind: PhantomData,
        }
    }
}

impl<T> From<&str> for Id<T> {
    fn from(id: &str) -> Self {
        id.to_owned().into()
    }
}

// Written by hand: a derive would demand the same traits of `T`.
impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        self.id.clone().into()
    }
}

impl<T> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<T> Eq for Id<T> {}

impl<T> std::hash::Hash for Id<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.id, f)
    }
}

impl<T> fmt::Display for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.id)
    }
}

impl<T> Serialize for Id<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.id)
    }
}

impl<'de, T> Deserialize<'de> for Id<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Into::into)
    }
}

/// Kraken's comma-joined lists (`"post,fciq"`, `XBTUSD,ETHUSD`), each item one
/// `T` that serializes as a string. Published as it was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommaList<T>(pub Vec<T>);

impl<T> CommaList<T> {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<T> Default for CommaList<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T> From<Vec<T>> for CommaList<T> {
    fn from(items: Vec<T>) -> Self {
        Self(items)
    }
}

impl<T> FromIterator<T> for CommaList<T> {
    fn from_iter<I: IntoIterator<Item = T>>(items: I) -> Self {
        Self(items.into_iter().collect())
    }
}

impl<T: Serialize> Serialize for CommaList<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let tokens = self
            .0
            .iter()
            .map(|token| match serde_json::to_value(token) {
                Ok(serde_json::Value::String(token)) => Ok(token),
                _ => Err(serde::ser::Error::custom(
                    "a list token must serialize as a string",
                )),
            })
            .collect::<Result<Vec<_>, S::Error>>()?;
        s.serialize_str(&tokens.join(","))
    }
}

impl<'de, T: de::DeserializeOwned> Deserialize<'de> for CommaList<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let joined = String::deserialize(d)?;
        joined
            .split(',')
            .filter(|token| !token.is_empty())
            .map(|token| T::deserialize(de::value::StrDeserializer::<D::Error>::new(token)))
            .collect::<Result<_, _>>()
            .map(Self)
    }
}

/// Logs a token an open enum has no variant for. Open enums are the ones
/// Kraken grows without notice; the rest fail the decode.
pub(crate) fn unrecognized<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let token = String::deserialize(d)?;
    tracing::warn!(
        token,
        "unrecognized value in an open enum; passing it through"
    );
    Ok(token)
}

/// A reply that is a list or an object, decided by its JSON shape rather than
/// by trying each variant, so a drifted row still names its field.
pub(crate) enum ListOrObject<L, M> {
    List(L),
    Object(M),
}

impl<'de, L: Deserialize<'de>, M: Deserialize<'de>> Deserialize<'de> for ListOrObject<L, M> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct ShapeVisitor<L, M>(PhantomData<(L, M)>);

        impl<'de, L: Deserialize<'de>, M: Deserialize<'de>> de::Visitor<'de> for ShapeVisitor<L, M> {
            type Value = ListOrObject<L, M>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a list or an object")
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, rows: A) -> Result<Self::Value, A::Error> {
                L::deserialize(de::value::SeqAccessDeserializer::new(rows)).map(ListOrObject::List)
            }

            fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                M::deserialize(de::value::MapAccessDeserializer::new(map)).map(ListOrObject::Object)
            }
        }

        d.deserialize_any(ShapeVisitor(PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn only_the_legacy_table_is_renamed() {
        for (wire, modern) in [
            ("XXBT", "BTC"),
            ("ZUSD", "USD"),
            ("XRP", "XRP"),
            ("ZEC", "ZEC"),
        ] {
            assert_eq!(Asset::from(wire).as_str(), modern);
        }
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum Flag {
        Post,
        Fciq,
    }

    #[test]
    fn a_comma_list_round_trips_as_its_wire_string() {
        let flags: CommaList<Flag> = serde_json::from_str(r#""post,fciq""#).unwrap();
        assert_eq!(flags.0, [Flag::Post, Flag::Fciq]);
        assert_eq!(serde_json::to_string(&flags).unwrap(), r#""post,fciq""#);
    }

    #[test]
    fn an_empty_comma_list_is_no_tokens() {
        let flags: CommaList<Flag> = serde_json::from_str(r#""""#).unwrap();
        assert!(flags.0.is_empty());
        assert_eq!(serde_json::to_string(&flags).unwrap(), r#""""#);
    }

    #[test]
    fn an_unknown_list_token_fails_the_decode() {
        assert!(serde_json::from_str::<CommaList<Flag>>(r#""post,nope""#).is_err());
    }

    #[test]
    fn a_fractional_timestamp_keeps_its_wire_digits() {
        let time: UnixTime = serde_json::from_str("1792499896.0352738").unwrap();
        assert_eq!(time.as_str(), "1792499896.0352738");
    }

    proptest! {
        #[test]
        fn keyed_rows_keep_wire_order(keys in prop::collection::vec("[A-Z]{6}", 0..32)) {
            let unique = keys;
            let object = format!(
                "{{{}}}",
                unique.iter().enumerate().map(|(i, k)| format!("\"{k}{i}\":{i}")).collect::<Vec<_>>().join(",")
            );
            let rows: KeyedRows<String, u32> = serde_json::from_str(&object).unwrap();
            let order: Vec<u32> = rows.values().copied().collect();
            prop_assert_eq!(order, (0..unique.len() as u32).collect::<Vec<_>>());
            prop_assert_eq!(serde_json::to_string(&rows).unwrap(), object);
        }
    }
}
