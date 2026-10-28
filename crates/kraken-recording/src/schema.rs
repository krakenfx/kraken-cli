//! The recording schema's cross-cutting pieces: the version stamp, its
//! compatibility rule, and the `_meta` table. The per-channel tables are
//! defined data-driven next to their codecs (`frames::duckdb::tables`).
//!
//! Storage conventions across the channel tables: numeric columns are exact
//! `DECIMAL(38,18)` (fed as decimal strings, never floats); unsigned wire
//! ids/counts use `UBIGINT`/`UINTEGER` (lossless); timestamps stay RFC3339
//! `VARCHAR` — a microsecond `TIMESTAMP` would truncate the wire's
//! nanoseconds. `recv_ts` is the local write time; a monotonic per-frame
//! `seq` (high-water mark in `_meta.last_seq`) groups a frame's rows and
//! orders frames across reopen.

/// Schema version stamp (SemVer; a differing MAJOR is incompatible, with no
/// migration path — old tapes are rejected and re-recorded). 1.0 is this
/// crate's first stamp: frames on kraken-core's typed envelope with exact
/// decimals, and a capture summary carrying `frames_unparsed`.
pub const SCHEMA_VERSION: &str = "1.0";

/// The capture source recorded in `_meta`.
pub const SOURCE: &str = "kraken-spot-ws-v2";

/// The key/value metadata table every tape carries: version/source/symbols/
/// channels/window, the final lag summary, and the resumable `last_seq`.
#[cfg(feature = "duckdb")]
pub const META_DDL: &str =
    "CREATE TABLE IF NOT EXISTS _meta (key VARCHAR PRIMARY KEY, value VARCHAR)";

/// Whether an on-disk schema version is compatible with what this build writes
/// — true iff they share a MAJOR component. Ungated: the jsonl sidecar carries
/// the same stamp, so version checks don't need the `duckdb` feature.
pub fn is_compatible(found: &str) -> bool {
    major(found) == major(SCHEMA_VERSION)
}

/// Whether a stamped version is a newer MINOR of this build's MAJOR — the one
/// case where undecodable content is a newer writer's vocabulary rather than
/// damage, so a reader may skip it instead of failing loud.
pub(crate) fn is_newer_minor(found: &str) -> bool {
    is_compatible(found) && minor(found) > minor(SCHEMA_VERSION)
}

/// The one incompatible-stamp rejection, shared by every backend's gate so
/// the rule and its wording cannot drift. `action` names the refused
/// operation ("append", "read").
pub(crate) fn ensure_compatible(found: &str, action: &str) -> crate::error::Result<()> {
    if is_compatible(found) {
        return Ok(());
    }
    Err(crate::error::Error::Rejected(format!(
        "recording schema version {found} is incompatible with this build (v{SCHEMA_VERSION}); \
         refusing to {action}"
    )))
}

fn major(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

/// An absent or unparseable minor reads as 0: never "newer", so damage
/// tolerance stays off for a malformed stamp.
fn minor(version: &str) -> u64 {
    version
        .split('.')
        .nth(1)
        .and_then(|m| m.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compatibility_is_by_major() {
        assert!(is_compatible("1.0"));
        assert!(is_compatible("1.7")); // newer minor, same major
        assert!(!is_compatible("0.9"));
        assert!(!is_compatible("2.0"));
    }

    #[test]
    fn newer_minor_is_same_major_and_greater_minor_only() {
        assert!(is_newer_minor("1.7"));
        assert!(!is_newer_minor(SCHEMA_VERSION), "own stamp is not newer");
        assert!(!is_newer_minor("2.1"), "different major is incompatible");
        assert!(!is_newer_minor("1.oops"), "malformed minor is not newer");
    }
}
