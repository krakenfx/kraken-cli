//! Temporary facade: the session-layer contracts (manifest, decision log,
//! timeline, run artifact) live in [`kraken_session`]; the binary's
//! `crate::session::` references ride this glob until a later cleanup
//! rewrites them.

pub(crate) use kraken_session::*;
