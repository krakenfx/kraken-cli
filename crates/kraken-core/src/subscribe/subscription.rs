//! [`WsSubscription`] — the strictly-typed `subscribe` params, one variant per
//! channel.
//!
//! Source: the per-channel `subscribe` schemas under
//! <https://docs.kraken.com/api/docs/websocket-v2>. Params by channel:
//!
//! | channel      | params                                                            |
//! |--------------|-------------------------------------------------------------------|
//! | `ticker`     | `symbol[]`, `event_trigger` (`bbo`/`trades`), `snapshot`          |
//! | `trade`      | `symbol[]`, `snapshot`                                            |
//! | `book`       | `symbol[]`, `depth` (10/25/100/500/1000), `snapshot`             |
//! | `ohlc`       | `symbol[]`, `interval` (minutes), `snapshot`                     |
//! | `instrument` | `snapshot` (not symbol-scoped)                                   |
//! | `executions` | `snap_trades`, `snap_orders`, `order_status`, `rebased`, `ratecounter`, `users` |
//! | `balances`   | `snapshot`, `rebased`, `users`                                   |
//! | `level3`     | `symbol[]`, `depth` (10/100/1000), `snapshot`                    |
//!
//! Optional fields are `Option<_>` and omitted when unset, so the server
//! applies its own defaults. The auth token is not modelled here — it is
//! flattened into `params` at send time by [`Wire`](crate::Wire).

use super::channel::{BookDepth, Level3Depth, OhlcInterval, SubscribableChannel};
use crate::endpoint::Endpoint;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use strum::{Display, EnumString};

/// What change re-publishes a `ticker` frame: every book event (`bbo`) or only
/// a trade (`trades`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum EventTrigger {
    Bbo,
    Trades,
}

/// The `users` param on the account channels. `all` — the only value the wire
/// accepts — streams subaccount events alongside the master's (master accounts
/// only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum Users {
    All,
}

/// Channel-specific subscribe params, serialized with a `"channel"` tag. `None`
/// options are omitted, matching the v2 contract exactly.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "channel", rename_all = "lowercase")]
pub enum WsSubscription {
    Ticker {
        symbol: Vec<String>,
        event_trigger: Option<EventTrigger>,
        snapshot: Option<bool>,
    },
    Trade {
        symbol: Vec<String>,
        snapshot: Option<bool>,
    },
    Book {
        symbol: Vec<String>,
        depth: BookDepth,
        snapshot: Option<bool>,
    },
    Ohlc {
        symbol: Vec<String>,
        interval: OhlcInterval,
        snapshot: Option<bool>,
    },
    Instrument {
        // The instrument channel is not symbol-scoped in the v2 docs; `symbol` is kept
        // only so a caller *may* narrow the feed, and is dropped from the wire when empty.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        symbol: Vec<String>,
        snapshot: Option<bool>,
    },
    Executions {
        snap_trades: Option<bool>,
        snap_orders: Option<bool>,
        order_status: Option<bool>,
        rebased: Option<bool>,
        ratecounter: Option<bool>,
        users: Option<Users>,
    },
    Balances {
        snapshot: Option<bool>,
        rebased: Option<bool>,
        users: Option<Users>,
    },
    Level3 {
        symbol: Vec<String>,
        depth: Option<Level3Depth>,
        snapshot: Option<bool>,
    },
}

impl WsSubscription {
    /// The wire `method` every subscription frame carries — the subscribe
    /// side's counterpart of [`Request::METHOD`](crate::Request::METHOD).
    pub const METHOD: &'static str = "subscribe";

    /// Force the `snapshot` flag on — the connection does this when replaying a
    /// subscription after a reconnect, so local state is rebuilt cleanly. A
    /// no-op for `Executions`, which has no `snapshot` field
    /// (`snap_trades`/`snap_orders` instead).
    pub fn force_snapshot(&mut self) {
        match self {
            Self::Ticker { snapshot: s, .. }
            | Self::Trade { snapshot: s, .. }
            | Self::Book { snapshot: s, .. }
            | Self::Ohlc { snapshot: s, .. }
            | Self::Instrument { snapshot: s, .. }
            | Self::Balances { snapshot: s, .. }
            | Self::Level3 { snapshot: s, .. } => *s = Some(true),
            Self::Executions { .. } => {}
        }
    }

    /// The channel these params subscribe to.
    pub fn channel(&self) -> SubscribableChannel {
        match self {
            Self::Ticker { .. } => SubscribableChannel::Ticker,
            Self::Trade { .. } => SubscribableChannel::Trade,
            Self::Book { .. } => SubscribableChannel::Book,
            Self::Ohlc { .. } => SubscribableChannel::Ohlc,
            Self::Instrument { .. } => SubscribableChannel::Instrument,
            Self::Executions { .. } => SubscribableChannel::Executions,
            Self::Balances { .. } => SubscribableChannel::Balances,
            Self::Level3 { .. } => SubscribableChannel::Level3,
        }
    }

    /// The subscribed symbols (empty for account channels with no symbol).
    pub fn symbols(&self) -> &[String] {
        match self {
            Self::Ticker { symbol, .. }
            | Self::Trade { symbol, .. }
            | Self::Book { symbol, .. }
            | Self::Ohlc { symbol, .. }
            | Self::Instrument { symbol, .. }
            | Self::Level3 { symbol, .. } => symbol,
            Self::Executions { .. } | Self::Balances { .. } => &[],
        }
    }

    /// Drop the symbols `keep` rejects, in place. Returns whether the
    /// subscription still subscribes to anything: a symbol-scoped channel
    /// emptied out subscribes to nothing, while the account channels always
    /// do. A narrowed `instrument` list emptied out also counts as nothing
    /// — an empty list on the wire would *widen* to the whole feed.
    #[must_use = "a dropped verdict replays an emptied subscription, which for `instrument` widens \
                  to the whole feed"]
    pub fn retain_symbols(&mut self, mut keep: impl FnMut(&str) -> bool) -> bool {
        match self {
            Self::Ticker { symbol, .. }
            | Self::Trade { symbol, .. }
            | Self::Book { symbol, .. }
            | Self::Ohlc { symbol, .. }
            | Self::Level3 { symbol, .. } => {
                symbol.retain(|s| keep(s));
                !symbol.is_empty()
            }
            Self::Instrument { symbol, .. } => {
                if symbol.is_empty() {
                    return true;
                }
                symbol.retain(|s| keep(s));
                !symbol.is_empty()
            }
            Self::Executions { .. } | Self::Balances { .. } => true,
        }
    }

    /// The socket endpoint this subscription rides.
    pub fn endpoint(&self) -> Endpoint {
        self.channel().endpoint()
    }

    /// Whether this subscription needs an auth token.
    pub fn requires_token(&self) -> bool {
        self.endpoint().requires_token()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Wire;

    fn frame(params: WsSubscription) -> serde_json::Value {
        serde_json::to_value(Wire::new("subscribe", None, &params, Some(1))).unwrap()
    }

    fn executions() -> WsSubscription {
        WsSubscription::Executions {
            snap_trades: None,
            snap_orders: None,
            order_status: None,
            rebased: None,
            ratecounter: None,
            users: None,
        }
    }

    fn ticker(symbol: Vec<String>, event_trigger: Option<EventTrigger>) -> WsSubscription {
        WsSubscription::Ticker {
            symbol,
            event_trigger,
            snapshot: None,
        }
    }

    #[test]
    fn ticker_frame_has_expected_shape() {
        let v = frame(ticker(
            vec!["BTC/USD".into(), "ETH/USD".into()],
            Some(EventTrigger::Bbo),
        ));
        assert_eq!(
            v,
            serde_json::json!({
                "method": "subscribe",
                "req_id": 1,
                "params": {
                    "channel": "ticker",
                    "symbol": ["BTC/USD", "ETH/USD"],
                    "event_trigger": "bbo",
                },
            })
        );
    }

    #[test]
    fn users_all_serializes_as_the_wire_string_and_round_trips() {
        let v = frame(WsSubscription::Executions {
            snap_trades: None,
            snap_orders: None,
            order_status: None,
            rebased: None,
            ratecounter: None,
            users: Some(Users::All),
        });
        assert_eq!(v["params"]["users"], "all");
        // `users` is omitted entirely when unset — never `null`.
        assert!(frame(executions())["params"].get("users").is_none());
        let raw = serde_json::to_string(&Users::All).unwrap();
        assert_eq!(raw, r#""all""#);
        assert_eq!(serde_json::from_str::<Users>(&raw).unwrap(), Users::All);
    }

    #[test]
    fn private_subscribe_flattens_token_into_params() {
        let v = serde_json::to_value(Wire::new("subscribe", Some("tok-1"), &executions(), None))
            .unwrap();
        assert_eq!(v["params"]["channel"], "executions");
        assert_eq!(v["params"]["token"], "tok-1");
        assert!(v["params"].get("symbol").is_none());
        assert!(v.get("req_id").is_none());
    }

    #[test]
    fn endpoint_and_token_follow_the_channel() {
        let ticker = ticker(vec!["X".into()], None);
        assert_eq!(ticker.endpoint(), Endpoint::Public);
        assert!(!ticker.requires_token());
        assert!(executions().requires_token());
        let level3 = WsSubscription::Level3 {
            symbol: vec!["X".into()],
            depth: None,
            snapshot: None,
        };
        assert_eq!(level3.endpoint(), Endpoint::L3);
    }
}

#[cfg(test)]
mod retain_properties {
    use super::*;

    proptest::proptest! {
        /// `retain_symbols` on a symbol-scoped channel reports exactly whether
        /// anything remains, and only ever narrows the list — the invariant the
        /// replay/eviction machinery leans on.
        #[test]
        fn retain_reports_exactly_whether_anything_remains(
            symbols in proptest::collection::vec("[A-Z]{2,4}/USD", 0..6),
            keep_mask in proptest::collection::vec(proptest::bool::ANY, 6),
        ) {
            let original = symbols.clone();
            let keep = |s: &str| {
                original
                    .iter()
                    .position(|o| o == s)
                    .is_some_and(|i| keep_mask[i])
            };
            let mut sub = WsSubscription::Ticker {
                symbol: symbols,
                event_trigger: None,
                snapshot: None,
            };
            let anything_left = sub.retain_symbols(keep);
            let WsSubscription::Ticker { symbol, .. } = &sub else {
                unreachable!("retain never changes the variant");
            };
            proptest::prop_assert_eq!(anything_left, !symbol.is_empty());
            proptest::prop_assert!(symbol.iter().all(|s| keep(s) && original.contains(s)));
        }
    }
}
