//! Streaming channel args → their `WsSubscription`, infallible: clap pins every
//! per-flag bound, so the conversion cannot fail.
//!
//! Each `From` impl destructures without `..`, so a newly added flag won't
//! compile until it is mapped onto the wire type.
//!
//! The symbol-scoped channels require at least one pair: an empty list would
//! send no subscribe frame and idle forever — the server answers an
//! empty-symbol subscribe with silence, not a rejection. `instrument` differs
//! by design: no pairs means the whole catalogue.
