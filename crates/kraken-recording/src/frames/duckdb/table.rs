//! Data-driven recording schema: a table described as data ([`TableSpec`]), with
//! its `CREATE TABLE` and `INSERT` SQL *generated from one column list* so the two
//! can never drift out of sync (the bug class a hand-written DDL/INSERT pair
//! invites).
//!
//! A channel persists itself by implementing [`Recordable`] in its channel submodule
//! of [`crate::frames::duckdb::tables`]: it declares its [`TableSpec`] and maps a frame to
//! [`Row`]s of [`Cell`]s — and back. The DuckDB sink consumes only this trait — it
//! generates SQL from the spec and binds the cells, knowing nothing about any one
//! channel's columns. [`Cell`] carries the storage type so unsigned wire ids land in
//! `UBIGINT`/`UINTEGER` (lossless, unlike a narrowing cast) and decimals bind as
//! their exact string form cast into `DECIMAL(38,18)`.
//!
//! The market-data tables are `NOT NULL` except where the wire field itself
//! is optional (OHLC's deprecated `timestamp`), declared via [`Column::opt`].

use std::collections::HashSet;
use std::sync::OnceLock;

use duckdb::types::{ToSql, ToSqlOutput, Value, ValueRef};
use duckdb::{Connection, params};
use kraken_core::ChannelMessage;
use kraken_core::subscribe::message::MessageType;
use rust_decimal::Decimal;

use crate::error::{Error, Result};

/// The storage type of a column: its DuckDB column type plus the `INSERT`
/// placeholder it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColType {
    /// `VARCHAR` — symbols, sides, RFC3339 timestamps (lossless; a reader casts).
    Text,
    /// `DECIMAL(38,18)` — prices/quantities, fed as an exact decimal string and cast.
    Decimal,
    /// `BIGINT` — the signed frame `seq`.
    Big,
    /// `UINTEGER` — small unsigned wire values (`checksum`, `interval`).
    UInt,
    /// `UBIGINT` — large unsigned wire ids/counts (`trade_id`, `trades`) — stored
    /// losslessly rather than narrowed into a signed `BIGINT`.
    UBig,
}

impl ColType {
    fn sql_type(self) -> &'static str {
        match self {
            ColType::Text => "VARCHAR",
            ColType::Decimal => "DECIMAL(38,18)",
            ColType::Big => "BIGINT",
            ColType::UInt => "UINTEGER",
            ColType::UBig => "UBIGINT",
        }
    }

    /// A decimal binds as its exact string form and casts inside the engine,
    /// so it lands in the `DECIMAL` column without a float on the way.
    fn placeholder(self) -> &'static str {
        match self {
            ColType::Decimal => "CAST(? AS DECIMAL(38,18))",
            _ => "?",
        }
    }
}

/// One column of a recording table.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Column {
    pub name: &'static str,
    pub ty: ColType,
    nullable: bool,
}

impl Column {
    /// A required (`NOT NULL`) column.
    pub(crate) const fn req(name: &'static str, ty: ColType) -> Self {
        Self {
            name,
            ty,
            nullable: false,
        }
    }

    /// A nullable column, for a wire field that is itself optional.
    pub(crate) const fn opt(name: &'static str, ty: ColType) -> Self {
        Self {
            name,
            ty,
            nullable: true,
        }
    }
}

/// A table described as data: its name and ordered columns. The DDL and the
/// `INSERT` are both generated from this one definition, so they cannot disagree.
#[derive(Debug)]
pub(crate) struct TableSpec {
    pub name: &'static str,
    pub columns: &'static [Column],
    /// Generated once per process: `prepare_cached` keys on this string, so
    /// rebuilding it per frame would pay the formatting on every insert.
    insert_sql: OnceLock<String>,
}

impl TableSpec {
    pub(crate) const fn new(name: &'static str, columns: &'static [Column]) -> Self {
        Self {
            name,
            columns,
            insert_sql: OnceLock::new(),
        }
    }

    pub(crate) fn create_sql(&self) -> String {
        let cols = self
            .columns
            .iter()
            .map(|c| {
                let null = if c.nullable { "" } else { " NOT NULL" };
                format!("{} {}{null}", c.name, c.ty.sql_type())
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("CREATE TABLE IF NOT EXISTS {} ({cols})", self.name)
    }

    pub(crate) fn insert_sql(&self) -> &str {
        self.insert_sql.get_or_init(|| {
            let names = self
                .columns
                .iter()
                .map(|c| c.name)
                .collect::<Vec<_>>()
                .join(", ");
            let holes = self
                .columns
                .iter()
                .map(|c| c.ty.placeholder())
                .collect::<Vec<_>>()
                .join(", ");
            format!("INSERT INTO {} ({names}) VALUES ({holes})", self.name)
        })
    }

    /// Migrations for columns the spec gained after tapes already existed:
    /// `CREATE TABLE IF NOT EXISTS` never alters an existing table, so each
    /// nullable column also gets an idempotent `ADD COLUMN IF NOT EXISTS`.
    /// Only nullable columns can appear retroactively — existing rows read
    /// them back as NULL, which is what the wire's absent field means.
    pub(crate) fn add_column_sql(&self) -> impl Iterator<Item = String> + '_ {
        self.columns.iter().filter(|c| c.nullable).map(|c| {
            format!(
                "ALTER TABLE {} ADD COLUMN IF NOT EXISTS {} {}",
                self.name,
                c.name,
                c.ty.sql_type()
            )
        })
    }
}

/// One bound value, tagged by how it stores. A decimal carries the payload's
/// `Decimal`, rendered via `Display` at bind time (exact, non-finite unrepresentable).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Cell {
    Text(String),
    Decimal(Decimal),
    Big(i64),
    UInt(u32),
    UBig(u64),
    /// An absent optional wire field, bound into a [`Column::opt`] column.
    Null,
}

/// One table row: cells positionally matching a [`TableSpec`]'s columns.
pub(crate) type Row = Vec<Cell>;

impl ToSql for Cell {
    fn to_sql(&self) -> duckdb::Result<ToSqlOutput<'_>> {
        let out = match self {
            // Borrow text straight from the cell — no clone on the bind path.
            Cell::Text(s) => ToSqlOutput::Borrowed(ValueRef::Text(s.as_bytes())),
            Cell::Big(i) => ToSqlOutput::Owned(Value::BigInt(*i)),
            Cell::UInt(u) => ToSqlOutput::Owned(Value::UInt(*u)),
            Cell::UBig(u) => ToSqlOutput::Owned(Value::UBigInt(*u)),
            // The column is `DECIMAL`; feed the exact decimal string for the cast.
            Cell::Decimal(d) => ToSqlOutput::Owned(Value::Text(d.to_string())),
            Cell::Null => ToSqlOutput::Owned(Value::Null),
        };
        Ok(out)
    }
}

/// A channel payload that persists itself into the recording schema, data-driven:
/// it declares its [`TableSpec`] and maps a frame's entries to [`Row`]s — and back.
/// Implemented on each channel's entry struct in one submodule per channel in
/// [`crate::frames::duckdb::tables`], so adding a channel is a self-contained table module
/// rather than an edit to a central SQL block, and a table's spec, encode, and
/// decode live side by side where they can't drift apart.
pub(crate) trait Recordable: Sized {
    /// The table this payload writes (its schema, as data). An associated function
    /// so the sink can gather every channel's schema without an instance.
    fn table() -> &'static TableSpec;

    /// Expand one frame's entries into rows. The envelope fields a table persists are
    /// passed explicitly: `message_type` distinguishes snapshot from update rows, `seq`
    /// is the per-frame id shared by every row of the frame (so a multi-row frame stays
    /// groupable), and `recv_ts` is the local capture instant.
    fn rows(entries: &[Self], message_type: MessageType, seq: i64, recv_ts: &str) -> Vec<Row>;

    /// Read every stored frame back in write order, regrouped into whole frames —
    /// the inverse of [`rows`](Self::rows). An absent table reads back empty;
    /// unknown vocabulary follows `drift` (the recording stamp's verdict).
    fn frames(conn: &Connection, drift: DriftPolicy) -> Result<Vec<Stored>>;
}

/// One frame read back from a channel table: [`Recordable::frames`]'s unit,
/// carrying the write-time frame order the timeline sorts on.
#[derive(Debug)]
pub(crate) struct Stored {
    pub seq: i64,
    pub frame: ChannelMessage,
}

/// The read query for a spec: `DECIMAL`s selected as VARCHAR for [`read_decimal`],
/// ordered by `(seq, rowid)` so a frame's rows regroup in insertion order.
/// A nullable column missing from `physical` — a reader never migrates, so a
/// tape written before the column existed still lacks it — is selected as
/// a typed NULL, reading back as the absent wire field it stands for.
pub(crate) fn select_sql(spec: &TableSpec, physical: &HashSet<String>) -> String {
    let cols = spec
        .columns
        .iter()
        .map(|c| match (physical.contains(c.name), &c.ty) {
            (true, ColType::Decimal) => {
                format!("CAST({name} AS VARCHAR) AS {name}", name = c.name)
            }
            (true, _) => c.name.to_string(),
            // Decimals read back through VARCHAR, so their NULL matches.
            (false, ColType::Decimal) => format!("CAST(NULL AS VARCHAR) AS {}", c.name),
            (false, ty) => format!("CAST(NULL AS {}) AS {}", ty.sql_type(), c.name),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("SELECT {cols} FROM {} ORDER BY seq, rowid", spec.name)
}

/// Why one row failed to decode.
#[derive(Debug)]
pub(crate) enum RowError {
    /// An enum value this build doesn't recognize (e.g., a newer trade side):
    /// a newer same-MAJOR writer's vocabulary — or corruption. Which one is
    /// decided by the recording's stamp ([`DriftPolicy`]).
    Vocabulary(String),
    /// A structural value the current schema says must decode but didn't:
    /// corruption under any stamp, always fails the read.
    Damage(Error),
}

/// How a read treats vocabulary this build doesn't know — the DuckDB half of
/// the one drift rule (the JSONL source's `ensure_tolerable_drift`): only a
/// *newer-minor* recording stamp can legitimately carry unknown values, so
/// only there is skipping tolerance rather than swallowed damage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DriftPolicy {
    /// Same-or-older stamp: an unknown value is a corrupted row.
    FailLoud,
    /// Newer-minor stamp: skip the whole frame carrying the unknown value.
    TolerateNewerVocabulary,
}

/// A missing column, type mismatch, or `NULL` is structural drift, never
/// newer vocabulary.
impl From<duckdb::Error> for RowError {
    fn from(err: duckdb::Error) -> Self {
        RowError::Damage(Error::Engine(err))
    }
}

/// Parse a `DECIMAL`'s VARCHAR selection ([`select_sql`]) back to the writer's
/// `Decimal` exactly; a DuckDB `DOUBLE` cast would round through a float.
pub(crate) fn read_decimal(
    row: &duckdb::Row<'_>,
    column: &str,
) -> std::result::Result<Decimal, RowError> {
    let raw: String = row.get(column)?;
    // DuckDB renders the column's full 18-digit scale ("13588.200000000000000000").
    // Plain `from_str`, not `from_str_exact`: a wide integral part can push those
    // trailing zeros past Decimal's 96-bit coefficient — the value fits, its
    // rendering rounds. `normalize` then strips the scale padding: serde-float
    // double-rounds an oversized scale-18 mantissa (13588.2 would re-serialize as
    // 13588.199999999999), and wire Decimals arrive through f64, which carries no
    // trailing zeros to preserve — so the normalized value is the written one.
    raw.parse::<Decimal>().map(|d| d.normalize()).map_err(|_| {
        RowError::Damage(Error::Damaged(format!(
            "unreadable decimal '{raw}' in column {column}"
        )))
    })
}

/// Run the spec's read query, decoding each row through `entry`.
/// [`Vocabulary`](RowError::Vocabulary) failures follow `drift`;
/// [`Damage`](RowError::Damage) always fails the read before any consumer
/// sees a partial stream (the [`Source::read`](crate::source::Source::read)
/// contract); query failures propagate.
pub(crate) fn read_rows<E>(
    conn: &Connection,
    spec: &'static TableSpec,
    drift: DriftPolicy,
    entry: impl Fn(&duckdb::Row<'_>) -> std::result::Result<E, RowError>,
) -> Result<Vec<E>> {
    let physical = physical_columns(conn, spec.name)?;
    if physical.is_empty() {
        // No columns means no table: a channel this tape never recorded
        // reads back empty (one catalog probe serves existence and shape).
        return Ok(Vec::new());
    }
    if let Some(missing) = spec
        .columns
        .iter()
        .find(|c| !c.nullable && !physical.contains(c.name))
    {
        // A compatible stamp guarantees the required columns; surface the
        // impossibility as damage, not a raw engine binder error.
        return Err(Error::Damaged(format!(
            "recording table '{}' lacks required column '{}'",
            spec.name, missing.name
        )));
    }
    let mut stmt = conn.prepare(&select_sql(spec, &physical))?;
    let mut rows = stmt.query([])?;
    // Entries carry their frame's seq so a tolerated unknown value can
    // poison its *whole* frame — rows of that seq decoded before or after
    // the bad one must go with it, or a multi-entry frame would read back
    // partially (the JSONL skip drops whole lines, i.e. whole frames).
    let mut entries: Vec<(i64, E)> = Vec::new();
    let mut skipped: HashSet<i64> = HashSet::new();
    while let Some(row) = rows.next()? {
        let seq: i64 = row.get("seq")?;
        match entry(row) {
            Ok(e) => entries.push((seq, e)),
            Err(RowError::Vocabulary(what)) => match drift {
                DriftPolicy::TolerateNewerVocabulary => {
                    if skipped.insert(seq) {
                        tracing::warn!(
                            table = spec.name,
                            seq,
                            what,
                            "skipping a newer-minor writer's frame outside this build's vocabulary"
                        );
                    }
                }
                DriftPolicy::FailLoud => {
                    return Err(Error::Damaged(format!(
                        "damaged recording table '{}': undecodable row ({what})",
                        spec.name
                    )));
                }
            },
            Err(RowError::Damage(err)) => {
                return Err(Error::Damaged(format!(
                    "damaged recording table '{}': {err}",
                    spec.name
                )));
            }
        }
    }
    Ok(entries
        .into_iter()
        .filter(|(seq, _)| !skipped.contains(seq))
        .map(|(_, e)| e)
        .collect())
}

pub(crate) fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let mut stmt =
        conn.prepare("SELECT count(*) FROM information_schema.tables WHERE table_name = ?")?;
    let count: i64 = stmt.query_row(params![table], |row| row.get(0))?;
    Ok(count > 0)
}

/// The columns `table` physically has, for [`select_sql`]'s missing-column
/// tolerance. Scoped to the tape's own catalog and schema, so a same-named
/// table elsewhere can't leak columns in.
fn physical_columns(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT column_name FROM information_schema.columns \
         WHERE table_catalog = current_database() AND table_schema = 'main' \
           AND table_name = ?",
    )?;
    let cols = stmt
        .query_map(params![table], |row| row.get(0))?
        .collect::<std::result::Result<HashSet<String>, _>>()?;
    Ok(cols)
}

#[cfg(test)]
mod tests {
    use super::*;

    static SPEC: TableSpec = TableSpec::new(
        "demo",
        &[
            Column::req("symbol", ColType::Text),
            Column::req("price", ColType::Decimal),
            Column::req("seq", ColType::Big),
            Column::opt("note", ColType::Text),
        ],
    );

    #[test]
    fn create_sql_renders_types_and_nullability() {
        assert_eq!(
            SPEC.create_sql(),
            "CREATE TABLE IF NOT EXISTS demo (symbol VARCHAR NOT NULL, \
             price DECIMAL(38,18) NOT NULL, seq BIGINT NOT NULL, note VARCHAR)"
        );
    }

    #[test]
    fn insert_sql_casts_decimals_only() {
        assert_eq!(
            SPEC.insert_sql(),
            "INSERT INTO demo (symbol, price, seq, note) \
             VALUES (?, CAST(? AS DECIMAL(38,18)), ?, ?)"
        );
    }

    #[test]
    fn insert_placeholder_count_matches_columns() {
        // The generated INSERT must bind exactly as many values as the table has
        // columns — the property a hand-written DDL/INSERT pair can silently break.
        assert_eq!(SPEC.insert_sql().matches('?').count(), SPEC.columns.len());
    }

    fn all_columns() -> HashSet<String> {
        SPEC.columns.iter().map(|c| c.name.to_string()).collect()
    }

    #[test]
    fn select_sql_casts_decimals_to_varchar_in_spec_order() {
        assert_eq!(
            select_sql(&SPEC, &all_columns()),
            "SELECT symbol, CAST(price AS VARCHAR) AS price, seq, note \
             FROM demo ORDER BY seq, rowid"
        );
    }

    #[test]
    fn select_sql_projects_null_for_a_missing_nullable_column() {
        // A reader never migrates: a tape from before `note` existed must
        // still read, with the field absent rather than a binder error.
        let mut physical = all_columns();
        physical.remove("note");
        assert_eq!(
            select_sql(&SPEC, &physical),
            "SELECT symbol, CAST(price AS VARCHAR) AS price, seq, \
             CAST(NULL AS VARCHAR) AS note FROM demo ORDER BY seq, rowid"
        );
    }

    #[test]
    fn add_column_sql_migrates_only_nullable_columns() {
        // Required columns existed from the table's birth; only retroactive
        // nullable columns need the idempotent ALTER.
        assert_eq!(
            SPEC.add_column_sql().collect::<Vec<_>>(),
            ["ALTER TABLE demo ADD COLUMN IF NOT EXISTS note VARCHAR"]
        );
    }
}
