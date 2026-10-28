//! Which stored Spot key is active, and the prompt that changes it.
use crate::errors::{KrakenError, Result};
use kraken_auth::{CredentialSource, CredentialStore, KeySlot, LoginKey, StoredAuth};
use std::io::IsTerminal;

/// Asks whether a new key replaces the active key, and saves the answer. The
/// new key is already saved, so a failure here only warns: the active key
/// stays. Returns the active key.
pub(crate) fn offer_new_key(
    store: &CredentialStore,
    stored: StoredAuth,
    new: KeySlot,
) -> Option<KeySlot> {
    let active = stored.active();
    if !stored.is_shadowed(new) {
        return active;
    }
    let switch = format!("kraken auth switch {new}");
    if !can_prompt() {
        tracing::warn!(%switch, "the new key is stored, but it is not active");
        return active;
    }
    choose(&stored, new)
        .and_then(|choice| apply_choice(store, choice))
        .map_or_else(
            |error| {
                tracing::warn!(%error, %switch, "the active key did not change");
                active
            },
            |stored| stored.active(),
        )
}

/// The prompt shows a read from before it, so the keys are read again here: a
/// change made during the prompt is kept, and Esc (`None`) writes nothing.
pub(super) fn apply_choice(store: &CredentialStore, choice: Option<KeySlot>) -> Result<StoredAuth> {
    match choice {
        Some(slot) => Ok(store.update(|current| current.activate(slot))?.0),
        None => Ok(store.load()?),
    }
}

/// A pipe cannot answer a prompt, and the prompt draws on stderr.
pub(super) fn can_prompt() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Lists the stored keys with `preferred` first, as the default. `None` when
/// the user presses Esc.
pub(super) fn choose(stored: &StoredAuth, preferred: KeySlot) -> Result<Option<KeySlot>> {
    let mut options: Vec<(KeySlot, String)> = Vec::new();
    if let Some(pair) = stored.own() {
        options.push((
            KeySlot::ApiKey,
            format!("the API key ({})", pair.masked_key()),
        ));
    }
    if let Some(login) = stored.login() {
        options.push((
            KeySlot::SignIn,
            format!("the sign-in key ({})", login_label(login)),
        ));
    }
    options.sort_by_key(|(slot, _)| *slot != preferred);
    match options.as_slice() {
        [] => {
            return Err(KrakenError::Validation(
                "No key is stored. Run `kraken auth login` or `kraken auth set`.".into(),
            ));
        }
        [(only, _)] => return Ok(Some(*only)),
        _ => {}
    }

    let labels: Vec<&str> = options.iter().map(|(_, label)| label.as_str()).collect();
    let picked = dialoguer::Select::new()
        .with_prompt("Which Spot key do you want kraken-cli to use?")
        .items(&labels)
        .default(0)
        .interact_opt()
        .map_err(|e| KrakenError::Config(format!("Prompt error: {e}")))?;
    picked
        .map(|index| {
            options.get(index).map(|(slot, _)| *slot).ok_or_else(|| {
                KrakenError::Config(format!("Prompt error: no key at index {index}"))
            })
        })
        .transpose()
}

/// A sign-in key can live for an hour, so its expiry shows the time too.
pub(super) const EXPIRY_FORMAT: &str = "%Y-%m-%d %H:%M UTC";

/// `kraken-cli-0a1b2c3d, expires 2026-01-13 09:30 UTC`.
pub(super) fn login_label(login: &LoginKey) -> String {
    let state = if login.is_expired() {
        "expired"
    } else {
        "expires"
    };
    format!(
        "{}, {state} {}",
        login.name,
        login.valid_until.format(EXPIRY_FORMAT)
    )
}

/// `source`, when it is a flag or env key, signs instead of the active key.
/// It warns, so a user never takes the stored key for the one in use.
pub(super) fn report_override(
    active: Option<KeySlot>,
    source: Option<CredentialSource>,
) -> Option<String> {
    let source = source.filter(|source| source.is_override())?;
    tracing::warn!(
        active = %active_cell(active),
        overridden_by = %source,
        "a key that is not stored overrides the active key"
    );
    Some(source.to_string())
}

/// The table cell for the active key.
pub(super) fn active_cell(active: Option<KeySlot>) -> String {
    active.map_or_else(|| "(none)".into(), |slot| slot.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choosing_with_no_stored_key_refuses_without_a_prompt() {
        let error = choose(&StoredAuth::default(), KeySlot::ApiKey).unwrap_err();
        assert!(matches!(error, KrakenError::Validation(_)), "{error}");
    }

    #[test]
    fn choosing_with_one_stored_key_needs_no_prompt() {
        let mut stored = StoredAuth::default();
        stored.set_own(kraken_auth::KeyPair::new("own-key", "b3du"));
        assert_eq!(
            choose(&stored, KeySlot::SignIn).unwrap(),
            Some(KeySlot::ApiKey)
        );
    }

    #[test]
    fn esc_in_the_prompt_writes_no_choice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let unreadable =
            "[auth]\napi_key = \"own-key\"\napi_secret = \"b3du\"\nactive = \"newer\"\n";
        std::fs::write(&path, unreadable).unwrap();

        let stored = apply_choice(&CredentialStore::new(&path), None).unwrap();

        assert_eq!(stored.active(), None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), unreadable);
    }
}
