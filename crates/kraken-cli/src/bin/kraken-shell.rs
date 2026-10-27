//! The `kraken-shell` binary. It is built in this package so the release
//! installer ships it with `kraken`; the REPL is the `kraken-shell` crate.

fn main() -> std::process::ExitCode {
    kraken_shell::main()
}
