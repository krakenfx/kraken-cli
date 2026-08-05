// A test-only crate: unwrap/panic are its assertion idiom, exempt from the workspace deny.
#![allow(clippy::unwrap_used, clippy::panic)]

mod integration;
