//! Instant parsing for recorded timestamps, shared by every layer that
//! orders them.

use chrono::{DateTime, SecondsFormat, Utc};

/// Recorded stamps mix offset styles (`+00:00` from older builds, `Z` from
/// the wire), so ordering must compare parsed instants — string comparison
/// sorts them wrong.
pub fn parse_instant(ts: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// The one rendering for instants at non-serde boundaries (DuckDB `_meta`
/// TEXT values): `Z`-suffixed RFC3339, byte-identical to chrono's serde
/// output, so a stamp reads the same no matter which path wrote it.
pub fn format_instant(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}
