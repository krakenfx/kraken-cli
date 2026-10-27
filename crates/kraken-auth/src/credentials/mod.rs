//! The keys that the CLI signs with: where they are stored, which one is
//! active, and how a flag or an env var overrides them.

mod auth_table;
mod credential;
mod engine;
mod key_pair;
mod private_file;
mod resolve;
mod secret_input;
mod store;
mod stored;

pub(crate) use credential::has_expired;
pub use credential::{CredentialSource, Credentials};
pub use engine::Engine;
pub use key_pair::KeyPair;
pub use resolve::{Supplied, resolve_stored, resolve_supplied};
pub use secret_input::{SecretSource, read_secret};
pub use store::CredentialStore;
pub use stored::{KeySlot, LoginKey, StoredAuth};
