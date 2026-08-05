//! Directory-entry durability. `File::sync_all` makes a file's *content*
//! durable; a create or rename is only durable once the parent directory's
//! entry is fsync'd too — otherwise a power loss can legally roll the
//! operation back (POSIX leaves rename/create durability to the directory).

use std::path::Path;

/// Fsync `path`'s parent directory, making a just-created or just-renamed
/// entry durable. No-op on non-unix: Windows directory handles don't support
/// flush semantics, and NTFS journals metadata itself.
pub(crate) fn fsync_parent(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        // An empty parent means a bare relative filename: the CWD, "." opens it.
        let dir = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        std::fs::File::open(dir)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
