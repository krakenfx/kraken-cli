//! Shared helpers for driving the real binary against an isolated config
//! home, so no test can touch the developer's own state.

use assert_cmd::Command;

#[allow(deprecated)]
pub(crate) fn kraken() -> Command {
    Command::cargo_bin("kraken").unwrap()
}

pub(crate) fn kraken_in(home: &tempfile::TempDir) -> Command {
    let mut cmd = kraken();
    cmd.env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env_remove("KRAKEN_SESSION")
        .env_remove("KRAKEN_WORKSPACE");
    cmd
}

/// The config dir the binary resolves under [`kraken_in`]'s env:
/// `dirs::config_dir()` follows HOME on macOS and XDG_CONFIG_HOME elsewhere.
pub(crate) fn config_dir_in(home: &tempfile::TempDir) -> std::path::PathBuf {
    if cfg!(target_os = "macos") {
        home.path()
            .join("Library")
            .join("Application Support")
            .join("kraken")
    } else {
        home.path().join(".config").join("kraken")
    }
}
