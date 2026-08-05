//! The incoming method-reply model, mirroring the [`subscribe`](super::subscribe) side:
//! [`result`] holds the vocabulary and typed bodies (as `channel` and `message`'s
//! payloads do there), and [`envelope`] the shared reply frame. Frames are routed
//! here by [`Inbound`](crate::Inbound)'s `FromStr`.

mod envelope;
mod result;

pub use envelope::MethodResponse;
pub use result::{Method, MethodReply};
