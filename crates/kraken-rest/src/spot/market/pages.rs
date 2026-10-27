//! The `{"<PAIR>": [rows…], "last": cursor}` shape OHLC, Trades and Spread
//! share: one entry keyed by Kraken's wire pair name, plus a paging cursor.

use crate::Pair;
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};
use std::{fmt, marker::PhantomData};

pub(super) struct PairPage<R, L> {
    pub(super) pair: Pair,
    pub(super) rows: Vec<R>,
    pub(super) last: L,
}

impl<'de, R: Deserialize<'de>, L: Deserialize<'de>> Deserialize<'de> for PairPage<R, L> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct PageVisitor<R, L>(PhantomData<(R, L)>);

        impl<'de, R: Deserialize<'de>, L: Deserialize<'de>> Visitor<'de> for PageVisitor<R, L> {
            type Value = PairPage<R, L>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("one pair's rows and a `last` cursor")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut page: (Option<(String, Vec<R>)>, Option<L>) = (None, None);
                while let Some(key) = map.next_key::<String>()? {
                    if key == "last" {
                        page.1 = Some(map.next_value()?);
                    } else if page.0.is_some() {
                        return Err(de::Error::custom(format!(
                            "a second pair `{key}` in one page"
                        )));
                    } else {
                        page.0 = Some((key, map.next_value()?));
                    }
                }
                let (pair, rows) = page.0.ok_or_else(|| de::Error::missing_field("<pair>"))?;
                let last = page.1.ok_or_else(|| de::Error::missing_field("last"))?;
                Ok(PairPage {
                    pair: pair.into(),
                    rows,
                    last,
                })
            }
        }

        d.deserialize_map(PageVisitor(PhantomData))
    }
}
