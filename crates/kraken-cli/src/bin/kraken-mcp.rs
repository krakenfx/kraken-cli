//! The `kraken-mcp` binary. It is built in this package so the release
//! installer ships it with `kraken`; the server is the `kraken-mcp` crate.

fn main() -> std::process::ExitCode {
    kraken_mcp::main()
}
