//! The file that holds the secrets: one writer at a time, replaced
//! atomically, and private to its owner on unix.
use crate::error::{Error, Result};
use std::{
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// Locks the sidecar `<file>.lock`, which is never unlinked: unlinking a held
/// lock file lets a second writer lock a fresh inode. The kernel releases the
/// lock when the holder exits, so a crash leaves no stale lock.
pub(super) fn lock(path: &Path) -> Result<fs::File> {
    create_dir(path)?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(sibling(path, ".lock"))?;
    file.lock()?;
    Ok(file)
}

/// Writes a new temporary file, then renames it over `path`. The name is
/// random and the file must not exist, so a leftover file or a symlink is
/// never reused. On unix it is 0600 from creation.
pub(super) fn write(path: &Path, data: &[u8]) -> Result<()> {
    let suffix = getrandom::u64().map_err(Error::Random)?;
    let tmp_path = sibling(path, &format!(".{suffix:016x}.tmp"));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = options.open(&tmp_path).and_then(|mut file| {
        file.write_all(data)?;
        file.sync_all()?;
        fs::rename(&tmp_path, path)
    });
    if let Err(error) = written {
        if let Err(cleanup) = fs::remove_file(&tmp_path)
            && cleanup.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %tmp_path.display(), error = %cleanup, "could not remove a temporary file");
        }
        return Err(error.into());
    }
    Ok(())
}

/// An older release, or a user, can leave the file readable by others. The
/// next write replaces it with a 0600 file. Checked once per process: a
/// command can read the keys more than once.
#[cfg(unix)]
pub(super) fn warn_if_shared(path: &Path) {
    use std::{
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicBool, Ordering},
    };
    static CHECKED: AtomicBool = AtomicBool::new(false);
    if CHECKED.swap(true, Ordering::Relaxed) {
        return;
    }
    if let Ok(metadata) = fs::metadata(path)
        && metadata.permissions().mode() & 0o077 != 0
    {
        tracing::warn!(
            path = %path.display(),
            "config.toml holds API keys and other users can read it. Run `chmod 600` on it."
        );
    }
}

#[cfg(not(unix))]
pub(super) fn warn_if_shared(_path: &Path) {}

/// A directory that the CLI creates is 0700. An existing one keeps its mode:
/// it can be a shared `~/.config`.
fn create_dir(path: &Path) -> Result<()> {
    let Some(dir) = path.parent() else {
        return Ok(());
    };
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    Ok(())
}

/// `config.toml` + `suffix`, appended so `.toml` stays in the name.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    name.into()
}
