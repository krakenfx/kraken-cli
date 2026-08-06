//! The CLI's stream-side sink plumbing over [`kraken_recording`]'s storage.
//!
//! The write contract ([`Sink`](kraken_recording::Sink)) and the durable
//! backends live in the recording crate; this module keeps only what is
//! CLI-specific: [`driver`] feeds sinks from the WS broadcast and owns the
//! stream lifecycle, [`fanout`] composes durable backends with a best-effort
//! stdout tee, and [`stdout`] renders frames per `-o`/`--output`.

pub(crate) mod driver;
pub(crate) mod fanout;
pub(crate) mod stdout;

pub(crate) use stdout::print_result;
