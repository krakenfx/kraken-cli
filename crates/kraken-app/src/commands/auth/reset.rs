use super::active_key::EXPIRY_FORMAT;
use crate::{AppContext, commands::Execute, config, errors::Result, output::CommandOutput};
use kraken_auth::{CredentialStore, LoginKey};
use kraken_contract::auth::Reset;

impl Execute for Reset {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        let store = config::credential_store()?;
        let left = left_on_kraken(&store);
        store.reset()?;
        Ok(render(&left))
    }
}

/// A reset needs no browser sign-in, so it cannot revoke the sign-in key on
/// Kraken. The output names what stays valid there, because the key can trade.
enum LeftOnKraken {
    Nothing,
    SignInKey(LoginKey),
    /// The keys could not be read, so a sign-in key may still be valid.
    Unknown,
}

/// Reset is the repair for a malformed `[auth]` table, so a failed read costs
/// only the warning.
fn left_on_kraken(store: &CredentialStore) -> LeftOnKraken {
    match store.load() {
        Ok(stored) => stored
            .login()
            .filter(|login| !login.is_expired())
            .cloned()
            .map_or(LeftOnKraken::Nothing, LeftOnKraken::SignInKey),
        Err(error) => {
            tracing::warn!(%error, "could not read the stored keys before the reset");
            LeftOnKraken::Unknown
        }
    }
}

fn render(left: &LeftOnKraken) -> CommandOutput {
    let (cell, still_valid) = match left {
        // Only the sign-in key is checked: a key from `auth set` stays valid too.
        LeftOnKraken::Nothing => ("no sign-in key".to_string(), serde_json::Value::Null),
        LeftOnKraken::SignInKey(login) => (
            format!(
                "sign-in key {} until {}. Delete it in the Kraken UI to revoke it now.",
                login.name,
                login.valid_until.format(EXPIRY_FORMAT)
            ),
            serde_json::json!({
                "name": login.name.as_str(),
                "valid_until": login.valid_until.to_rfc3339(),
            }),
        ),
        LeftOnKraken::Unknown => (
            "unknown: the stored keys could not be read. A sign-in key stays valid until it \
             expires."
                .to_string(),
            serde_json::Value::Null,
        ),
    };
    let json = serde_json::json!({
        "message": "Credentials deleted.",
        "still_valid_sign_in_key": still_valid,
        "stored_keys_read": !matches!(left, LeftOnKraken::Unknown),
    });
    CommandOutput::key_value(
        vec![
            ("Status".into(), "Credentials deleted.".into()),
            ("Still Valid On Kraken".into(), cell),
        ],
        json,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_auth::KeyPair;

    #[test]
    fn a_live_sign_in_key_is_named_in_the_output() {
        let login = LoginKey {
            pair: KeyPair::new("k", "c2VjcmV0"),
            name: "kraken-cli-0000beef".parse().unwrap(),
            valid_until: chrono::Utc::now() + chrono::Duration::days(1),
        };
        let shown = render(&LeftOnKraken::SignInKey(login));
        assert_eq!(
            shown.data["still_valid_sign_in_key"]["name"],
            "kraken-cli-0000beef"
        );
    }

    #[test]
    fn an_unreadable_store_says_the_sign_in_key_is_unknown() {
        let shown = render(&LeftOnKraken::Unknown);
        assert_eq!(shown.data["stored_keys_read"], false);
    }
}
