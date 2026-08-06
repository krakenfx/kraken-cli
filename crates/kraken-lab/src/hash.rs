//! The lab's SHA-256 vocabulary: the `sha256:<hex>` content identity that
//! seals a spec (over its canonical JSON bytes) and pins a replay tape (over
//! its file bytes). One home for the primitive so the domain and the store
//! hash identically.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::Result;

/// `sha256:<hex>` over a file's bytes — the content identity a sealed run
/// plan pins, so a re-recorded tape never satisfies a frozen plan.
/// Streamed: recorded tapes can be large, and hashing must not hold one in
/// memory.
pub fn sha256_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_prefixed(&hasher.finalize()))
}

/// `sha256:<hex>` over in-memory bytes — the spec seal's primitive.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex_prefixed(&Sha256::digest(bytes))
}

fn hex_prefixed(digest: &[u8]) -> String {
    let mut hex = String::with_capacity(7 + digest.len() * 2);
    hex.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write;
        // Infallible: writing hex pairs into a String cannot fail.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}
