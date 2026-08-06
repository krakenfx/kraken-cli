//! The CLI's response schema, applied to raw wire payloads in one place.
//!
//! Kraken's wire format answers with cryptic short keys (`eb`, `vol`,
//! `descr.type`), legacy asset codes (`ZUSD`, `XXBT`), decimal strings, and
//! positional arrays. Every transform here reshapes such a payload into the
//! CLI's stable output schema: readable field names, money as JSON numbers,
//! order/ledger timestamps as strings (the wire token verbatim, never an
//! f64 re-encode), modern asset codes, positional market arrays as named
//! objects, and wire `null`s omitted.
//!
//! Two preservation rules bound every transform:
//! - Unknown wire *fields* pass through verbatim instead of being dropped,
//!   so data added by flags like `trades-history --ledgers` — or by Kraken
//!   later — always survives. The one exception is the AssetPairs
//!   legacy-alias strip (`altname`/`wsname`/`base`/`quote`/`lot`): a pair's
//!   modern identity is its map key, and the deprecated aliases would
//!   contradict it.
//! - Unknown wire *tokens* in enum-valued fields (an order `status` or
//!   ledger `type` Kraken adds later) stay verbatim — the account's own
//!   money records are never narrowed to a placeholder.

use std::str::FromStr;

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::{Map, Number, Value};

/// How one known field is published in the output schema.
enum Kind {
    /// Verbatim value.
    Keep,
    /// Decimal wire string → JSON number (the workspace's rust_decimal
    /// `serde-float` form; trailing zeros collapse, e.g. `"10000.00"` → `1e4`).
    Money,
    /// Unix-seconds wire number → its verbatim string (no f64 re-encode).
    Time,
    /// Legacy asset code → modern form (`XXBT` → `BTC`).
    Asset,
    /// Lenient trigger decode: only `"last"`/`"index"` survive; any other
    /// shape is omitted.
    Trigger,
}

/// One published field: wire key, output key, value form.
type FieldSpec = (&'static str, &'static str, Kind);

const TRADE_BALANCE_SPEC: &[FieldSpec] = &[
    ("eb", "equivalent_balance", Kind::Money),
    ("tb", "trade_balance", Kind::Money),
    ("m", "margin_amount", Kind::Money),
    ("n", "unrealized_pnl", Kind::Money),
    ("c", "cost_basis", Kind::Money),
    ("v", "floating_valuation", Kind::Money),
    ("e", "equity", Kind::Money),
    ("mf", "free_margin", Kind::Money),
    ("ml", "margin_level_pct", Kind::Money),
    ("uv", "unrealized_value", Kind::Money),
    ("mfo", "free_margin_original", Kind::Money),
];

const EXTENDED_BALANCE_ENTRY_SPEC: &[FieldSpec] = &[
    ("balance", "balance", Kind::Money),
    ("hold_trade", "hold_trade", Kind::Money),
    ("credit", "credit", Kind::Money),
    ("credit_used", "credit_used", Kind::Money),
];

/// One OpenOrders/ClosedOrders row.
const ORDER_SPEC: &[FieldSpec] = &[
    ("refid", "refid", Kind::Keep),
    ("userref", "userref", Kind::Keep),
    ("cl_ord_id", "cl_ord_id", Kind::Keep),
    ("status", "status", Kind::Keep),
    ("opentm", "opentm", Kind::Time),
    ("closetm", "closetm", Kind::Time),
    ("starttm", "starttm", Kind::Time),
    ("expiretm", "expiretm", Kind::Time),
    ("vol", "volume", Kind::Money),
    ("vol_exec", "volume_executed", Kind::Money),
    ("cost", "cost", Kind::Money),
    ("fee", "fee", Kind::Money),
    ("price", "price", Kind::Money),
    ("stopprice", "stopprice", Kind::Money),
    ("limitprice", "limitprice", Kind::Money),
    ("misc", "misc", Kind::Keep),
    ("oflags", "oflags", Kind::Keep),
    ("trades", "trades", Kind::Keep),
    ("time_in_force", "time_in_force", Kind::Keep),
    ("reason", "reason", Kind::Keep),
    ("trigger", "trigger", Kind::Trigger),
];

/// The `descr` object on an order row; `type` is buy/sell, published as
/// `side`.
const ORDER_DESCR_SPEC: &[FieldSpec] = &[
    ("pair", "pair", Kind::Keep),
    ("aclass", "aclass", Kind::Keep),
    ("type", "side", Kind::Keep),
    ("ordertype", "ordertype", Kind::Keep),
    ("price", "price", Kind::Money),
    ("price2", "price2", Kind::Money),
    ("leverage", "leverage", Kind::Keep),
    ("order", "order", Kind::Keep),
    ("close", "close", Kind::Keep),
];

/// One TradesHistory row. `vol` deliberately stays unrenamed here (unlike
/// order rows' `volume`) — each endpoint's published field set is a pinned
/// contract, not a cross-endpoint unification.
const TRADE_SPEC: &[FieldSpec] = &[
    ("ordertxid", "ordertxid", Kind::Keep),
    ("postxid", "postxid", Kind::Keep),
    ("posstatus", "posstatus", Kind::Keep),
    ("pair", "pair", Kind::Keep),
    ("aclass", "aclass", Kind::Keep),
    ("time", "time", Kind::Time),
    ("type", "side", Kind::Keep),
    ("ordertype", "ordertype", Kind::Keep),
    ("tradeordertype", "tradeordertype", Kind::Keep),
    ("price", "price", Kind::Money),
    ("cost", "cost", Kind::Money),
    ("fee", "fee", Kind::Money),
    ("vol", "vol", Kind::Money),
    ("margin", "margin", Kind::Money),
    ("leverage", "leverage", Kind::Money),
    ("misc", "misc", Kind::Keep),
    ("trade_id", "trade_id", Kind::Keep),
    ("maker", "maker", Kind::Keep),
];

/// One OpenPositions row. `class` (this endpoint's anomaly for `aclass`)
/// becomes `asset_class`; `rollovertm` stays a string.
const POSITION_SPEC: &[FieldSpec] = &[
    ("ordertxid", "ordertxid", Kind::Keep),
    ("posstatus", "posstatus", Kind::Keep),
    ("pair", "pair", Kind::Keep),
    ("class", "asset_class", Kind::Keep),
    ("time", "time", Kind::Time),
    ("type", "side", Kind::Keep),
    ("ordertype", "ordertype", Kind::Keep),
    ("cost", "cost", Kind::Money),
    ("fee", "fee", Kind::Money),
    ("vol", "vol", Kind::Money),
    ("vol_closed", "vol_closed", Kind::Money),
    ("margin", "margin", Kind::Money),
    ("terms", "terms", Kind::Keep),
    ("rollovertm", "rollovertm", Kind::Keep),
    ("misc", "misc", Kind::Keep),
    ("oflags", "oflags", Kind::Keep),
    ("value", "value", Kind::Money),
    ("net", "net", Kind::Money),
];

/// One Ledgers row.
const LEDGER_SPEC: &[FieldSpec] = &[
    ("refid", "refid", Kind::Keep),
    ("time", "time", Kind::Time),
    ("type", "ledger_type", Kind::Keep),
    ("subtype", "subtype", Kind::Keep),
    ("aclass", "aclass", Kind::Keep),
    ("asset", "asset", Kind::Asset),
    ("amount", "amount", Kind::Money),
    ("fee", "fee", Kind::Money),
    ("balance", "balance", Kind::Money),
];

/// TradeVolume. The `fees`/`fees_maker` schedules pass through untouched.
const TRADE_VOLUME_SPEC: &[FieldSpec] = &[
    ("currency", "currency", Kind::Asset),
    ("volume", "volume", Kind::Money),
];

pub(crate) fn trade_balance(data: &mut Value) {
    apply_spec(data, TRADE_BALANCE_SPEC);
}

/// Balance: a bare `{ asset: amount }` map — modern asset keys, numeric amounts.
pub(crate) fn balance(data: &mut Value) {
    normalize_asset_keys(data);
    if let Some(obj) = data.as_object_mut() {
        for amount in obj.values_mut() {
            money_in_place(amount);
        }
    }
}

/// BalanceEx: a bare `{ asset: entry }` map — modern asset keys, numeric
/// amounts, credit fields only for credit-enabled keys (wire-absent stays absent).
pub(crate) fn extended_balance(data: &mut Value) {
    normalize_asset_keys(data);
    for_each_row(data, &[], |entry| {
        apply_spec(entry, EXTENDED_BALANCE_ENTRY_SPEC);
    });
}

/// OpenOrders/ClosedOrders: rows under `open`/`closed`; the wrapper (and
/// ClosedOrders' `count`) stays. `time_in_force` is always present in the
/// schema (default GTC), so it is injected when absent.
pub(crate) fn order_rows(data: &mut Value) {
    for_each_row(data, &["open", "closed"], |row| {
        apply_spec(row, ORDER_SPEC);
        if let Some(descr) = row.get_mut("descr") {
            apply_spec(descr, ORDER_DESCR_SPEC);
        }
        if let Some(obj) = row.as_object_mut() {
            obj.entry("time_in_force")
                .or_insert_with(|| Value::String("gtc".into()));
        }
    });
}

/// TradesHistory: rows under `trades`; `count` stays.
pub(crate) fn trade_rows(data: &mut Value) {
    for_each_row(data, &["trades"], |row| apply_spec(row, TRADE_SPEC));
}

/// OpenPositions: a bare `{ position_id: row }` map. The `--consolidation`
/// array shape has no pinned schema, so arrays pass through verbatim rather
/// than getting a speculative one.
pub(crate) fn position_rows(data: &mut Value) {
    if data.is_array() {
        return;
    }
    for_each_row(data, &[], |row| apply_spec(row, POSITION_SPEC));
}

/// Ledgers (`{ ledger: {...}, count }`) and QueryLedgers (bare map).
pub(crate) fn ledger_rows(data: &mut Value) {
    for_each_row(data, &["ledger"], |row| apply_spec(row, LEDGER_SPEC));
}

pub(crate) fn trade_volume(data: &mut Value) {
    apply_spec(data, TRADE_VOLUME_SPEC);
}

// ---- Market REST --------------------------------------------------------------
//
// Kraken's market endpoints answer in positional arrays keyed by a wire pair
// name; each transform below reshapes them into named objects. Rows that
// don't match the expected positional shape pass through verbatim — a
// malformed row must never destroy the payload.

/// OHLC: `{ "<PAIR>": [[t,o,h,l,c,vwap,vol,count],…], last }` →
/// `{ pair, candles: [{time, open, high, low, close, vwap, volume, count}], last }`
/// — `pair` echoes the request string.
pub(crate) fn ohlc(data: &mut Value, pair: &str) {
    reshape_paged_rows(data, pair, "candles", |row| {
        let cells = row.as_array()?;
        let mut candle = Map::new();
        candle.insert("time".into(), cells.first()?.clone());
        candle.insert("open".into(), money(cells.get(1)?.clone()));
        candle.insert("high".into(), money(cells.get(2)?.clone()));
        candle.insert("low".into(), money(cells.get(3)?.clone()));
        candle.insert("close".into(), money(cells.get(4)?.clone()));
        candle.insert("vwap".into(), money(cells.get(5)?.clone()));
        candle.insert("volume".into(), money(cells.get(6)?.clone()));
        candle.insert("count".into(), cells.get(7)?.clone());
        Some(Value::Object(candle))
    });
}

/// Spreads: rows `[time, bid, ask]` → `{time, bid, ask}`; the integer `last`
/// cursor becomes a string.
pub(crate) fn spreads(data: &mut Value, pair: &str) {
    reshape_paged_rows(data, pair, "spreads", |row| {
        let cells = row.as_array()?;
        let mut spread = Map::new();
        spread.insert("time".into(), cells.first()?.clone());
        spread.insert("bid".into(), money(cells.get(1)?.clone()));
        spread.insert("ask".into(), money(cells.get(2)?.clone()));
        Some(Value::Object(spread))
    });
    if let Some(last) = data.get_mut("last") {
        *last = timestamp(std::mem::take(last));
    }
}

/// Trades: rows `[price, volume, time, side, type, misc, trade_id]` → named
/// objects — `side` decoded from `b`/`s`, `order_type` kept as the raw wire
/// token (`"l"`/`"m"`), `time` kept verbatim as a string.
pub(crate) fn trades(data: &mut Value, pair: &str) {
    reshape_paged_rows(data, pair, "trades", |row| {
        let cells = row.as_array()?;
        let side = match cells.get(3)?.as_str()? {
            "b" => "buy",
            "s" => "sell",
            _ => "unknown",
        };
        let mut trade = Map::new();
        trade.insert("price".into(), money(cells.first()?.clone()));
        trade.insert("volume".into(), money(cells.get(1)?.clone()));
        trade.insert("time".into(), timestamp(cells.get(2)?.clone()));
        trade.insert("side".into(), Value::String(side.into()));
        trade.insert("order_type".into(), cells.get(4)?.clone());
        trade.insert("misc".into(), cells.get(5)?.clone());
        trade.insert("trade_id".into(), cells.get(6)?.clone());
        Some(Value::Object(trade))
    });
}

/// Ticker: each pair's `a`/`b`/`c`/`v`/`p`/`t`/`l`/`h`/`o` arrays → named
/// fields. Map keys stay as Kraken returns them.
pub(crate) fn ticker(data: &mut Value) {
    let Some(pairs) = data.as_object_mut() else {
        return;
    };
    for row in pairs.values_mut() {
        explode_array(
            row,
            "a",
            &["ask_price", "ask_whole_lot_volume", "ask_lot_volume"],
            true,
        );
        explode_array(
            row,
            "b",
            &["bid_price", "bid_whole_lot_volume", "bid_lot_volume"],
            true,
        );
        explode_array(row, "c", &["last_price", "last_volume"], true);
        explode_array(row, "v", &["volume_today", "volume_24h"], true);
        explode_array(row, "p", &["vwap_today", "vwap_24h"], true);
        explode_array(row, "t", &["trades_today", "trades_24h"], false);
        explode_array(row, "l", &["low_today", "low_24h"], true);
        explode_array(row, "h", &["high_today", "high_24h"], true);
        if let Some(obj) = row.as_object_mut()
            && let Some(open) = obj.remove("o")
        {
            obj.insert("open".into(), money(open));
        }
    }
}

/// Orderbook: `{ "<PAIR>": { asks: [[p,v,t],…], bids: […] } }` — the pair
/// wrapper is dropped and each level becomes `{price, volume, timestamp}`.
pub(crate) fn orderbook(data: &mut Value) {
    let Some(obj) = data.as_object_mut() else {
        return;
    };
    let Some(book_key) = obj
        .iter()
        .find(|(_, v)| v.get("asks").is_some() || v.get("bids").is_some())
        .map(|(k, _)| k.clone())
    else {
        return;
    };
    let Some(mut book) = obj.remove(&book_key) else {
        return;
    };
    for side in ["asks", "bids"] {
        if let Some(Value::Array(levels)) = book.get_mut(side) {
            for level in levels.iter_mut() {
                let Some(cells) = level.as_array() else {
                    continue;
                };
                let (Some(price), Some(volume), Some(ts)) =
                    (cells.first(), cells.get(1), cells.get(2))
                else {
                    continue;
                };
                let mut named = Map::new();
                named.insert("price".into(), money(price.clone()));
                named.insert("volume".into(), money(volume.clone()));
                named.insert("timestamp".into(), ts.clone());
                *level = Value::Object(named);
            }
        }
    }
    *data = book;
}

/// Assets: keys normalised to modern codes; per-asset fields keep their
/// wire names.
pub(crate) fn assets(data: &mut Value) {
    normalize_asset_keys(data);
}

/// AssetPairs: slashless wire keys are rebuilt to `BASE/QUOTE` from the row's
/// legacy codes, the legacy aliases are stripped (see module doc), decimal
/// strings become numbers, and `fees` tiers become `{volume, fee_pct}`.
pub(crate) fn asset_pairs(data: &mut Value) {
    let Some(obj) = data.as_object_mut() else {
        return;
    };
    let entries: Map<String, Value> = std::mem::take(obj)
        .into_iter()
        .map(|(key, mut row)| {
            let key = modern_pair_key(key, &row);
            reshape_pair_row(&mut row);
            (key, row)
        })
        .collect();
    *obj = entries;
}

/// An already-slashed key stays verbatim; a slashless key is rebuilt from
/// the row's `base`/`quote` when both exist.
fn modern_pair_key(key: String, row: &Value) -> String {
    if key.contains('/') {
        return key;
    }
    let code = |field: &str| {
        row.get(field)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    match (code("base"), code("quote")) {
        (Some(base), Some(quote)) => {
            format!("{}/{}", normalise_asset(base), normalise_asset(quote))
        }
        _ => key,
    }
}

fn reshape_pair_row(row: &mut Value) {
    let Some(obj) = row.as_object_mut() else {
        return;
    };
    // The legacy-alias strip (modern identity lives in the map key), plus
    // `lot` — a deprecated constant (`"unit"`).
    for legacy in ["altname", "wsname", "base", "quote", "lot"] {
        obj.remove(legacy);
    }
    for decimal_field in ["tick_size", "ordermin", "costmin"] {
        if let Some(v) = obj.get_mut(decimal_field) {
            *v = money(std::mem::take(v));
        }
    }
    if let Some(v) = obj.get_mut("fee_volume_currency") {
        *v = asset(std::mem::take(v));
    }
    for tiers_field in ["fees", "fees_maker"] {
        if let Some(Value::Array(tiers)) = obj.get_mut(tiers_field) {
            for tier in tiers.iter_mut() {
                let Some(cells) = tier.as_array() else {
                    continue;
                };
                let (Some(volume), Some(fee_pct)) = (cells.first(), cells.get(1)) else {
                    continue;
                };
                let mut named = Map::new();
                named.insert("volume".into(), money(volume.clone()));
                named.insert("fee_pct".into(), money(fee_pct.clone()));
                *tier = Value::Object(named);
            }
        }
    }
}

/// The `{ "<PAIR>": rows, last }` pattern shared by OHLC/Trades/Spreads:
/// replace the wire pair key with `pair` (echoing the request string) and
/// the row array with named `rows_key` objects. A row the mapper rejects
/// passes through verbatim.
fn reshape_paged_rows(
    data: &mut Value,
    pair: &str,
    rows_key: &str,
    map_row: impl Fn(&Value) -> Option<Value>,
) {
    let Some(obj) = data.as_object_mut() else {
        return;
    };
    let Some(wire_key) = obj.keys().find(|k| *k != "last").cloned() else {
        return;
    };
    let Some(mut rows) = obj.remove(&wire_key) else {
        return;
    };
    if let Some(rows) = rows.as_array_mut() {
        for row in rows.iter_mut() {
            if let Some(named) = map_row(row) {
                *row = named;
            }
        }
    }
    obj.insert("pair".into(), Value::String(pair.to_string()));
    obj.insert(rows_key.to_string(), rows);
}

/// Split one of Ticker's positional arrays into named fields. Money fields go
/// through [`money`]; counters stay verbatim. A non-array value is reinserted
/// untouched.
fn explode_array(row: &mut Value, wire_key: &str, names: &[&str], is_money: bool) {
    let Some(obj) = row.as_object_mut() else {
        return;
    };
    let Some(v) = obj.remove(wire_key) else {
        return;
    };
    let Value::Array(cells) = v else {
        obj.insert(wire_key.to_string(), v);
        return;
    };
    for (cell, name) in cells.into_iter().zip(names) {
        let value = if is_money { money(cell) } else { cell };
        obj.insert((*name).to_string(), value);
    }
}

// ---- WS stream ------------------------------------------------------------------

/// The `balances` channel is the one WS channel whose published keys differ
/// from the wire: an entry's ledger `type` serializes as `ledger_type`, a
/// wallet's flavour as `wallet_type`. Applied to the serialized frame at the
/// stdout boundary only — recorded tapes and the in-process typed model keep
/// the wire shape (`balances` is not recordable anyway).
pub(crate) fn ws_balances(frame: &mut Value) {
    let Some(entries) = frame.get_mut("data").and_then(Value::as_array_mut) else {
        return;
    };
    for entry in entries {
        rename_key(entry, "type", "ledger_type");
        if let Some(wallets) = entry.get_mut("wallets").and_then(Value::as_array_mut) {
            for wallet in wallets {
                rename_key(wallet, "type", "wallet_type");
            }
        }
    }
}

fn rename_key(value: &mut Value, wire: &str, out: &str) {
    if let Some(obj) = value.as_object_mut()
        && let Some(v) = obj.remove(wire)
    {
        obj.insert(out.to_string(), v);
    }
}

/// Apply a field spec to one object: rename, convert, and omit wire `null`s
/// (optional fields publish as absent). Unknown keys pass through.
fn apply_spec(value: &mut Value, spec: &[FieldSpec]) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    for (wire, out, kind) in spec {
        let Some(v) = obj.remove(*wire) else { continue };
        if v.is_null() {
            continue;
        }
        // A `None` from convert (unrecognized Trigger) is omitted.
        if let Some(converted) = convert(v, kind) {
            obj.insert((*out).to_string(), converted);
        }
    }
}

fn convert(v: Value, kind: &Kind) -> Option<Value> {
    match kind {
        Kind::Keep => Some(v),
        Kind::Money => Some(money(v)),
        Kind::Time => Some(timestamp(v)),
        Kind::Asset => Some(asset(v)),
        Kind::Trigger => match v {
            Value::String(ref s) if s == "last" || s == "index" => Some(v),
            _ => None,
        },
    }
}

/// Decimal wire value → JSON number, exactly as rust_decimal's `serde-float`
/// feature serializes (`Decimal::to_f64`): strings are parsed, and wire
/// *integers* (fee-tier volumes) re-encode as floats (`50000` → `50000.0`).
/// Unparseable values stay verbatim — a malformed field must never destroy
/// the payload.
fn money(v: Value) -> Value {
    match v {
        Value::String(s) => Decimal::from_str(&s)
            .ok()
            .and_then(|d| d.to_f64())
            .and_then(Number::from_f64)
            .map_or_else(|| Value::String(s), Value::Number),
        Value::Number(n) if !n.is_f64() => n
            .as_f64()
            .and_then(Number::from_f64)
            .map_or_else(|| Value::Number(n), Value::Number),
        other => other,
    }
}

fn money_in_place(v: &mut Value) {
    *v = money(std::mem::take(v));
}

/// Wire unix-seconds number → its verbatim string (`Number::to_string`);
/// the fractional timestamp is never re-encoded through an f64.
fn timestamp(v: Value) -> Value {
    match v {
        Value::Number(n) => Value::String(n.to_string()),
        other => other,
    }
}

fn asset(v: Value) -> Value {
    match v {
        Value::String(s) => Value::String(normalise_asset(&s).to_string()),
        other => other,
    }
}

/// Rebuild a `{ asset: ... }` map with modern asset-code keys.
fn normalize_asset_keys(data: &mut Value) {
    let Some(obj) = data.as_object_mut() else {
        return;
    };
    let entries: Map<String, Value> = std::mem::take(obj)
        .into_iter()
        .map(|(k, v)| (normalise_asset(&k).to_string(), v))
        .collect();
    *obj = entries;
}

/// Visit every row of a keyed-rows response: rows live under the first present
/// `nested` key, or at the top level when `nested` finds nothing (bare-map
/// endpoints like QueryLedgers).
fn for_each_row(data: &mut Value, nested: &[&str], mut visit: impl FnMut(&mut Value)) {
    for key in nested {
        if let Some(container) = data.get_mut(*key) {
            visit_rows(container, &mut visit);
            return;
        }
    }
    visit_rows(data, &mut visit);
}

fn visit_rows(container: &mut Value, visit: &mut impl FnMut(&mut Value)) {
    match container {
        Value::Object(rows) => rows.values_mut().for_each(&mut *visit),
        Value::Array(rows) => rows.iter_mut().for_each(visit),
        _ => {}
    }
}

/// Kraken's legacy X/Z-heritage asset codes → modern form. Table-driven,
/// not a leading-char strip: modern codes that start with X/Z (XRP, XLM,
/// ZEC) pass through; only the finite legacy set maps.
fn normalise_asset(raw: &str) -> &str {
    match raw {
        "XXBT" => "BTC",
        "XBT" => "BTC",
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn trade_balance_acronyms_become_names_and_money_becomes_numbers() {
        let mut data = json!({
            "eb": "10839.7418", "tb": "10682.5000", "m": "0.0000",
            "n": "0.0000", "c": "0.0000", "v": "0.0000", "e": "10682.5000",
            "mf": "10682.5000", "ml": "268.42"
        });
        trade_balance(&mut data);
        assert_eq!(
            data,
            json!({
                "equivalent_balance": 10839.7418, "trade_balance": 10682.5,
                "margin_amount": 0.0, "unrealized_pnl": 0.0, "cost_basis": 0.0,
                "floating_valuation": 0.0, "equity": 10682.5,
                "free_margin": 10682.5, "margin_level_pct": 268.42
            })
        );
        assert!(
            data.get("unrealized_value").is_none(),
            "absent margin-only fields stay absent"
        );
    }

    // The wire fixture is a live-captured OpenOrders row (2026-06-04); the
    // expected output is its published schema form.
    #[test]
    fn open_order_row_matches_the_schema_of_the_live_fixture() {
        let mut data = json!({
            "open": {
                "OABC12-XYZ34-KLMN56": {
                    "cl_ord_id": "9ce3b826-7c70-4bb6-a89e-d6370e5378f6",
                    "cost": "0.00000",
                    "descr": {
                        "aclass": "forex", "close": "", "leverage": "none",
                        "order": "buy 0.00010000 XBTUSDC @ limit 20000.00",
                        "ordertype": "limit", "pair": "XBTUSDC",
                        "price": "20000.00", "price2": "0", "type": "buy"
                    },
                    "expiretm": 0, "fee": "0.00000", "limitprice": "0.00000", "misc": "",
                    "oflags": "post,fciq", "opentm": 1780582233.729133_f64, "price": "0.00000",
                    "refid": null, "starttm": 0, "status": "open", "stopprice": "0.00000",
                    "time_in_force": "gtc", "userref": null,
                    "vol": "0.00010000", "vol_exec": "0.00000000"
                }
            }
        });
        order_rows(&mut data);
        assert_eq!(
            data["open"]["OABC12-XYZ34-KLMN56"],
            json!({
                "cl_ord_id": "9ce3b826-7c70-4bb6-a89e-d6370e5378f6",
                "cost": 0.0,
                "descr": {
                    "aclass": "forex", "close": "", "leverage": "none",
                    "order": "buy 0.00010000 XBTUSDC @ limit 20000.00",
                    "ordertype": "limit", "pair": "XBTUSDC",
                    "price": 20000.0, "price2": 0.0, "side": "buy"
                },
                "expiretm": "0", "fee": 0.0, "limitprice": 0.0, "misc": "",
                "oflags": "post,fciq", "opentm": "1780582233.729133", "price": 0.0,
                "starttm": "0", "status": "open", "stopprice": 0.0,
                "time_in_force": "gtc",
                "volume": 0.0001, "volume_executed": 0.0
            }),
            "null refid/userref omitted, timestamps stringified, money numeric"
        );
    }

    #[test]
    fn order_row_without_time_in_force_gets_the_gtc_default() {
        let mut data = json!({ "open": { "TX": { "vol": "1", "descr": {} } } });
        order_rows(&mut data);
        assert_eq!(data["open"]["TX"]["time_in_force"], "gtc");
    }

    #[test]
    fn unrecognized_trigger_is_omitted_by_the_lenient_decode() {
        let mut data = json!({
            "open": {
                "A": { "trigger": 42, "descr": {} },
                "B": { "trigger": "index", "descr": {} }
            }
        });
        order_rows(&mut data);
        assert!(data["open"]["A"].get("trigger").is_none());
        assert_eq!(data["open"]["B"]["trigger"], "index");
    }

    #[test]
    fn trade_history_row_renames_side_and_keeps_flag_added_extras() {
        let mut data = json!({
            "trades": {
                "THVRQM-33VKH-UCI7BS": {
                    "ordertxid": "OQCLML-BW3P3-BUCMWZ", "postxid": "TKH2SE-M7IF5-CFI7LT",
                    "pair": "XXBTZUSD", "aclass": "forex", "time": 1688667796.8802,
                    "type": "buy", "ordertype": "limit", "tradeordertype": "limit",
                    "price": "30010.00000", "cost": "600.20000", "fee": "0.60020",
                    "vol": "0.02000000", "margin": "0.00000", "leverage": "0",
                    "misc": "", "trade_id": 93748276, "maker": true,
                    "ledgers": ["L4UESK-KG3EQ-UFO4T5"]
                }
            },
            "count": 2346
        });
        trade_rows(&mut data);
        let row = &data["trades"]["THVRQM-33VKH-UCI7BS"];
        assert_eq!(row["side"], "buy");
        assert_eq!(
            row["vol"],
            json!(0.02),
            "`vol` stays unrenamed on trade rows"
        );
        assert_eq!(row["time"], "1688667796.8802");
        assert_eq!(row["trade_id"], 93748276);
        assert_eq!(row["maker"], true);
        assert_eq!(
            row["ledgers"],
            json!(["L4UESK-KG3EQ-UFO4T5"]),
            "--ledgers data survives on the legacy route"
        );
        assert_eq!(data["count"], 2346);
    }

    #[test]
    fn position_row_renames_side_and_the_anomalous_class_key() {
        let mut data = json!({
            "TF5GVO-T7ZZ2-6NBKBI": {
                "ordertxid": "OLWNFG-LLH4R-D6SFFP", "posstatus": "open",
                "pair": "XXBTZUSD", "class": "forex", "time": 1605280097.8294,
                "type": "buy", "ordertype": "limit",
                "cost": "104610.52842", "fee": "289.06565", "vol": "8.82412861",
                "vol_closed": "0.20200000", "margin": "20922.10568",
                "terms": "0.0100% per 4 hours", "rollovertm": "1616672637",
                "misc": "", "oflags": "", "net": "+5.32"
            }
        });
        position_rows(&mut data);
        let row = &data["TF5GVO-T7ZZ2-6NBKBI"];
        assert_eq!(row["side"], "buy");
        assert_eq!(row["asset_class"], "forex");
        assert_eq!(row["vol"], json!(8.82412861));
        assert_eq!(row["time"], "1605280097.8294");
        assert_eq!(row["rollovertm"], "1616672637", "rollovertm stays a string");
        assert_eq!(row["net"], json!(5.32), "docalcs P&L is numeric");
    }

    #[test]
    fn consolidated_position_arrays_pass_through_verbatim() {
        // No pinned schema exists for `--consolidation` output, so no
        // speculative reshape is applied.
        let original = json!([{ "pair": "XXBTZUSD", "type": "buy", "positions": "2" }]);
        let mut data = original.clone();
        position_rows(&mut data);
        assert_eq!(data, original);
    }

    #[test]
    fn ledger_row_normalizes_asset_and_renames_type() {
        let mut data = json!({
            "ledger": {
                "L4UESK-KG3EQ-UFO4T5": {
                    "refid": "TJKLXF-PGMUI-4NTLXU", "time": 1688464484.1787,
                    "type": "trade", "subtype": "", "aclass": "currency",
                    "asset": "ZGBP", "amount": "24.5000", "fee": "0.0490",
                    "balance": "459567.9171"
                }
            },
            "count": 1
        });
        ledger_rows(&mut data);
        let row = &data["ledger"]["L4UESK-KG3EQ-UFO4T5"];
        assert_eq!(row["ledger_type"], "trade");
        assert_eq!(row["asset"], "GBP");
        assert_eq!(row["amount"], json!(24.5));
        assert_eq!(row["time"], "1688464484.1787");
    }

    #[test]
    fn query_ledgers_bare_map_rows_are_reshaped_at_the_top_level() {
        let mut data = json!({
            "L4UESK-KG3EQ-UFO4T5": { "type": "trade", "asset": "XXBT", "amount": "1.1" }
        });
        ledger_rows(&mut data);
        assert_eq!(data["L4UESK-KG3EQ-UFO4T5"]["asset"], "BTC");
        assert_eq!(data["L4UESK-KG3EQ-UFO4T5"]["ledger_type"], "trade");
    }

    #[test]
    fn balance_map_gets_modern_keys_and_numeric_amounts() {
        let mut data = json!({ "ZUSD": "25435.21", "XXBT": "1219.1", "USDC": "5.00" });
        balance(&mut data);
        assert_eq!(data, json!({ "USD": 25435.21, "BTC": 1219.1, "USDC": 5.0 }));
    }

    // Pins the same schema the typed branch's wiremock test pins for
    // balance-ex: modern keys, numeric money, absent credit stays absent.
    #[test]
    fn extended_balance_matches_the_typed_balance_ex_schema() {
        let mut data = json!({
            "ZUSD": { "balance": "25435.21", "hold_trade": "8.0", "credit": "10000.00" },
            "XXBT": { "balance": "1219.1", "hold_trade": "0.5" }
        });
        extended_balance(&mut data);
        assert_eq!(data["USD"]["balance"], json!(25435.21));
        assert_eq!(data["USD"]["credit"], json!(10000.0));
        assert_eq!(data["BTC"]["hold_trade"], json!(0.5));
        assert!(
            data["BTC"].get("credit").is_none(),
            "absent credit must be omitted, not null: {data:#}"
        );
    }

    #[test]
    fn trade_volume_normalizes_currency_and_keeps_fee_schedules() {
        let mut data = json!({
            "currency": "ZUSD", "volume": "200709587.4223",
            "fees": { "XXBTZUSD": { "fee": "0.1000" } }
        });
        trade_volume(&mut data);
        assert_eq!(data["currency"], "USD");
        assert_eq!(data["volume"], json!(200709587.4223));
        assert!(
            data.get("fees").is_some(),
            "fee schedules survive on the legacy route"
        );
    }

    #[test]
    fn unparseable_money_stays_verbatim() {
        let mut data = json!({ "eb": "not-a-number" });
        trade_balance(&mut data);
        assert_eq!(data["equivalent_balance"], "not-a-number");
    }

    #[test]
    fn ohlc_arrays_become_named_candles_with_the_request_pair() {
        let mut data = json!({
            "XXBTZUSD": [[1688671200, "30306.1", "30306.2", "30305.7", "30305.7", "30306.1", "3.39243896", 23]],
            "last": 1688672160
        });
        ohlc(&mut data, "BTCUSD");
        assert_eq!(
            data,
            json!({
                "pair": "BTCUSD",
                "candles": [{
                    "time": 1688671200, "open": 30306.1, "high": 30306.2,
                    "low": 30305.7, "close": 30305.7, "vwap": 30306.1,
                    "volume": 3.39243896, "count": 23
                }],
                "last": 1688672160
            })
        );
    }

    #[test]
    fn spreads_rows_are_named_and_the_last_cursor_becomes_a_string() {
        let mut data = json!({
            "XXBTZUSD": [[1688671834, "30292.10000", "30297.50000"]],
            "last": 1688672106
        });
        spreads(&mut data, "BTCUSD");
        assert_eq!(
            data["spreads"][0],
            json!({ "time": 1688671834, "bid": 30292.1, "ask": 30297.5 })
        );
        assert_eq!(data["last"], "1688672106");
        assert_eq!(data["pair"], "BTCUSD");
    }

    #[test]
    fn trades_side_decodes_and_order_type_stays_the_wire_token() {
        let mut data = json!({
            "XXBTZUSD": [["30243.40000", "0.34499092", 1688669597.8277_f64, "b", "m", "", 61044952]],
            "last": "1688671969993150842"
        });
        trades(&mut data, "BTCUSD");
        assert_eq!(
            data["trades"][0],
            json!({
                "price": 30243.4, "volume": 0.34499092, "time": "1688669597.8277",
                "side": "buy", "order_type": "m", "misc": "", "trade_id": 61044952
            }),
            "side decodes b→buy; order_type keeps the raw wire token"
        );
        assert_eq!(
            data["last"], "1688671969993150842",
            "cursor already a string"
        );
    }

    #[test]
    fn ticker_positional_arrays_become_named_fields() {
        let mut data = json!({
            "XXBTZUSD": {
                "a": ["30300.10000", "1", "1.000"], "b": ["30300.00000", "1", "1.000"],
                "c": ["30303.20000", "0.00067643"], "v": ["4083.67001100", "4412.73601799"],
                "p": ["30706.77771", "30689.13205"], "t": [34619, 38907],
                "l": ["29868.30000", "29868.30000"], "h": ["31631.00000", "31631.00000"],
                "o": "30502.80000"
            }
        });
        ticker(&mut data);
        assert_eq!(
            data["XXBTZUSD"],
            json!({
                "ask_price": 30300.1, "ask_whole_lot_volume": 1.0, "ask_lot_volume": 1.0,
                "bid_price": 30300.0, "bid_whole_lot_volume": 1.0, "bid_lot_volume": 1.0,
                "last_price": 30303.2, "last_volume": 0.00067643,
                "volume_today": 4083.670011, "volume_24h": 4412.73601799,
                "vwap_today": 30706.77771, "vwap_24h": 30689.13205,
                "trades_today": 34619, "trades_24h": 38907,
                "low_today": 29868.3, "low_24h": 29868.3,
                "high_today": 31631.0, "high_24h": 31631.0,
                "open": 30502.8
            })
        );
    }

    #[test]
    fn orderbook_drops_the_pair_wrapper_and_names_levels() {
        let mut data = json!({
            "XXBTZUSD": {
                "asks": [["30384.10000", "2.059", 1688671659]],
                "bids": [["30297.40000", "2.500", 1688671258]]
            }
        });
        orderbook(&mut data);
        assert_eq!(
            data,
            json!({
                "asks": [{ "price": 30384.1, "volume": 2.059, "timestamp": 1688671659 }],
                "bids": [{ "price": 30297.4, "volume": 2.5, "timestamp": 1688671258 }]
            })
        );
    }

    #[test]
    fn asset_pairs_rebuild_modern_keys_and_strip_legacy_aliases() {
        let mut data = json!({
            "XXBTZUSD": {
                "altname": "XBTUSD", "wsname": "XBT/USD",
                "aclass_base": "currency", "base": "XXBT",
                "aclass_quote": "currency", "quote": "ZUSD", "lot": "unit",
                "tick_size": "0.10000", "ordermin": "0.00005", "costmin": "0.5",
                "pair_decimals": 1, "fees": [[0, 0.26], [50000, 0.24]],
                "fee_volume_currency": "ZUSD", "status": "online"
            }
        });
        asset_pairs(&mut data);
        let row = &data["BTC/USD"];
        assert!(
            !row.is_null(),
            "slashless key rebuilt from base/quote: {data:#}"
        );
        for legacy in ["altname", "wsname", "base", "quote", "lot"] {
            assert!(row.get(legacy).is_none(), "{legacy} must be stripped");
        }
        assert_eq!(row["tick_size"], json!(0.1));
        assert_eq!(
            row["fees"][1],
            json!({ "volume": 50000.0, "fee_pct": 0.24 })
        );
        assert_eq!(row["fee_volume_currency"], "USD");
        assert_eq!(row["pair_decimals"], 1);
    }

    #[test]
    fn already_slashed_pair_keys_stay_verbatim() {
        let mut data = json!({ "RENDER/USD": { "aclass_base": "currency", "tick_size": "0.001" } });
        asset_pairs(&mut data);
        assert_eq!(data["RENDER/USD"]["tick_size"], json!(0.001));
    }

    #[test]
    fn ws_balances_renames_entry_and_wallet_type_keys() {
        let mut frame = json!({
            "channel": "balances", "type": "snapshot",
            "data": [
                { "asset": "BTC", "balance": 1.5,
                  "wallets": [{ "type": "earn", "id": "flexible", "balance": 1.5 }] },
                { "asset": "USD", "balance": 100.0, "type": "trade", "category": "trade" }
            ],
            "sequence": 9
        });
        ws_balances(&mut frame);
        assert_eq!(frame["data"][0]["wallets"][0]["wallet_type"], "earn");
        assert!(frame["data"][0]["wallets"][0].get("type").is_none());
        assert_eq!(frame["data"][1]["ledger_type"], "trade");
        assert!(frame["data"][1].get("type").is_none());
        assert_eq!(
            frame["type"], "snapshot",
            "the frame-envelope type (snapshot/update) is untouched"
        );
    }

    #[test]
    fn unknown_wire_fields_pass_through_untouched() {
        let mut data = json!({ "eb": "1.0", "brand_new_wire_field": {"x": 1} });
        trade_balance(&mut data);
        assert_eq!(data["brand_new_wire_field"], json!({"x": 1}));
        assert_eq!(data["equivalent_balance"], json!(1.0));
    }
}
