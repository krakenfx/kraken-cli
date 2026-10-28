use super::{KeyPair, credential::has_expired};
use crate::{
    error::{Error, Result},
    oauth::{ApiKey, KeyName},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One of the two stored Spot keys, named after how it was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum KeySlot {
    /// The API key that you set with `kraken auth set`.
    ApiKey,
    /// The sign-in key that `kraken auth login` created.
    SignIn,
}

impl KeySlot {
    /// The order the CLI uses when no key is chosen: the API key first.
    const DEFAULT_ORDER: [Self; 2] = [Self::ApiKey, Self::SignIn];

    /// The slot in prose; `Display` gives the `auth switch` value.
    pub fn label(self) -> &'static str {
        match self {
            Self::ApiKey => "API key",
            Self::SignIn => "sign-in key",
        }
    }
}

/// Which Spot key the user chose, as `config.toml` holds it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Choice {
    /// No choice yet: the API key comes before the sign-in key.
    #[default]
    Unset,
    Slot(KeySlot),
    /// A value this build cannot read. It stays on disk until a new choice.
    Unreadable,
}

impl Choice {
    pub(crate) fn slot(self) -> Option<KeySlot> {
        match self {
            Self::Slot(slot) => Some(slot),
            Self::Unset | Self::Unreadable => None,
        }
    }
}

/// The Spot key that `kraken auth login` created.
#[derive(Clone, Debug)]
pub struct LoginKey {
    pub pair: KeyPair,
    /// The next login deletes the key with this name.
    pub name: KeyName,
    pub valid_until: DateTime<Utc>,
}

impl LoginKey {
    pub fn is_expired(&self) -> bool {
        has_expired(self.valid_until)
    }
}

impl From<&ApiKey> for LoginKey {
    fn from(key: &ApiKey) -> Self {
        Self {
            pair: KeyPair::new(key.key().clone(), key.secret().clone()),
            name: key.name().clone(),
            valid_until: key.valid_until(),
        }
    }
}

/// Every stored key, and which Spot key the CLI signs with. A new key never
/// replaces the other slot, and a change of the active key is explicit.
#[derive(Clone, Debug, Default)]
pub struct StoredAuth {
    pub(crate) own: Option<KeyPair>,
    pub(crate) login: Option<LoginKey>,
    pub(crate) futures: Option<KeyPair>,
    pub(crate) chosen: Choice,
}

impl StoredAuth {
    pub fn own(&self) -> Option<&KeyPair> {
        self.own.as_ref()
    }

    pub fn login(&self) -> Option<&LoginKey> {
        self.login.as_ref()
    }

    pub fn futures(&self) -> Option<&KeyPair> {
        self.futures.as_ref()
    }

    pub fn set_own(&mut self, pair: KeyPair) {
        self.keep_active(KeySlot::ApiKey);
        self.own = Some(pair);
    }

    pub fn set_futures(&mut self, pair: KeyPair) {
        self.futures = Some(pair);
    }

    pub fn clear_futures(&mut self) {
        self.futures = None;
    }

    /// Returns the sign-in key that this one replaces.
    pub fn set_login(&mut self, key: LoginKey) -> Option<LoginKey> {
        self.keep_active(KeySlot::SignIn);
        self.login.replace(key)
    }

    pub(crate) fn has(&self, slot: KeySlot) -> bool {
        match slot {
            KeySlot::ApiKey => self.own.is_some(),
            KeySlot::SignIn => self.login.is_some(),
        }
    }

    /// The chosen slot. Without a choice, your API key comes before the login
    /// key. A choice whose key is gone, or that this build cannot read, gives
    /// no key: falling back could sign with a wider key than the one the user
    /// picked.
    pub fn active(&self) -> Option<KeySlot> {
        match self.chosen {
            Choice::Slot(slot) => self.has(slot).then_some(slot),
            Choice::Unset => KeySlot::DEFAULT_ORDER
                .into_iter()
                .find(|&slot| self.has(slot)),
            Choice::Unreadable => None,
        }
    }

    pub fn activate(&mut self, slot: KeySlot) -> Result<()> {
        if !self.has(slot) {
            return Err(Error::KeyNotStored(slot));
        }
        self.chosen = Choice::Slot(slot);
        Ok(())
    }

    /// The key in `slot` is stored, but the CLI signs with the other key.
    pub fn is_shadowed(&self, slot: KeySlot) -> bool {
        self.has(slot) && self.active() != Some(slot)
    }

    /// The active key is the sign-in key, and it expired.
    pub fn is_active_key_expired(&self) -> bool {
        self.active() == Some(KeySlot::SignIn)
            && self.login.as_ref().is_some_and(LoginKey::is_expired)
    }

    /// Pins the active key before the key in `new` is stored. Without a
    /// choice, the default order would let your API key take over in silence.
    /// An expired or missing active key is not kept: the new key replaces it.
    fn keep_active(&mut self, new: KeySlot) {
        let kept = self.active().filter(|_| !self.is_active_key_expired());
        self.chosen = Choice::Slot(kept.unwrap_or(new));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login_key() -> LoginKey {
        LoginKey {
            pair: KeyPair::new("login-key", "c2VjcmV0"),
            name: "kraken-cli-0000beef".parse().unwrap(),
            valid_until: Utc::now() + chrono::Duration::days(1),
        }
    }

    fn both_keys() -> StoredAuth {
        let mut stored = StoredAuth::default();
        stored.set_own(KeyPair::new("own-key", "c2VjcmV0"));
        stored.set_login(login_key());
        stored
    }

    #[test]
    fn without_a_choice_your_own_key_is_active() {
        assert_eq!(both_keys().active(), Some(KeySlot::ApiKey));
    }

    #[test]
    fn a_choice_selects_the_active_key() {
        let mut stored = both_keys();
        stored.activate(KeySlot::SignIn).unwrap();
        assert_eq!(stored.active(), Some(KeySlot::SignIn));
    }

    #[test]
    fn a_key_that_is_not_stored_cannot_be_activated() {
        let mut stored = StoredAuth::default();
        stored.set_login(login_key());
        assert!(matches!(
            stored.activate(KeySlot::ApiKey),
            Err(Error::KeyNotStored(KeySlot::ApiKey))
        ));
    }

    #[test]
    fn a_new_login_key_is_shadowed_by_your_own_key() {
        let stored = both_keys();
        assert!(stored.is_shadowed(KeySlot::SignIn));
        assert!(!stored.is_shadowed(KeySlot::ApiKey));
    }

    #[test]
    fn a_new_own_key_does_not_replace_the_active_login_key() {
        let mut stored = StoredAuth::default();
        stored.set_login(login_key());
        stored.set_own(KeyPair::new("own-key", "c2VjcmV0"));
        assert_eq!(stored.active(), Some(KeySlot::SignIn));
        assert!(stored.is_shadowed(KeySlot::ApiKey));
    }

    #[test]
    fn a_new_own_key_replaces_an_expired_active_login_key() {
        let mut stored = StoredAuth::default();
        stored.set_login(LoginKey {
            valid_until: Utc::now() - chrono::Duration::days(1),
            ..login_key()
        });
        stored.activate(KeySlot::SignIn).unwrap();
        stored.set_own(KeyPair::new("own-key", "c2VjcmV0"));
        assert_eq!(stored.active(), Some(KeySlot::ApiKey));
    }

    #[test]
    fn a_new_login_key_replaces_its_expired_predecessor_as_the_active_key() {
        let mut stored = both_keys();
        stored.set_login(LoginKey {
            valid_until: Utc::now() - chrono::Duration::days(1),
            ..login_key()
        });
        stored.activate(KeySlot::SignIn).unwrap();
        stored.set_login(login_key());
        assert_eq!(stored.active(), Some(KeySlot::SignIn));
        assert!(!stored.is_shadowed(KeySlot::SignIn));
    }

    #[test]
    fn a_chosen_key_that_is_gone_gives_no_key_until_a_new_one_is_stored() {
        let mut stored = StoredAuth::default();
        stored.set_own(KeyPair::new("own-key", "c2VjcmV0"));
        stored.chosen = Choice::Slot(KeySlot::SignIn);
        assert_eq!(stored.active(), None);

        stored.set_login(login_key());
        assert_eq!(stored.active(), Some(KeySlot::SignIn));
    }

    #[test]
    fn an_unreadable_choice_gives_no_key_until_a_new_one_is_stored() {
        let mut stored = StoredAuth::default();
        stored.set_own(KeyPair::new("own-key", "c2VjcmV0"));
        stored.chosen = Choice::Unreadable;
        assert_eq!(stored.active(), None);

        stored.set_login(login_key());
        assert_eq!(stored.active(), Some(KeySlot::SignIn));
    }

    #[test]
    fn the_only_stored_key_is_never_shadowed() {
        let mut stored = StoredAuth::default();
        stored.set_login(login_key());
        assert!(!stored.is_shadowed(KeySlot::SignIn));
        assert_eq!(stored.active(), Some(KeySlot::SignIn));
    }
}
