//! Shared validation for names that become filesystem paths.

/// Accepts one `[A-Za-z0-9._-]` path segment, excluding dot-only names.
///
/// Rejection preserves an injective, traversal-free name-to-path mapping.
pub fn is_safe_name_segment(s: &str) -> bool {
    let safe_bytes = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    safe_bytes && !s.bytes().all(|b| b == b'.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_single_safe_segments_and_rejects_traversal_shapes() {
        for good in ["btc-dip", "lab-m1-r2", "a.b_c-9", "v1.2"] {
            assert!(is_safe_name_segment(good), "{good}");
        }
        for bad in [
            "",
            ".",
            "..",
            "...",
            "a/b",
            "a b",
            "a\\b",
            "ünïcode",
            "a\0b",
        ] {
            assert!(!is_safe_name_segment(bad), "{bad:?}");
        }
    }
}
