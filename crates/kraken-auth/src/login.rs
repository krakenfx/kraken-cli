use crate::{
    credentials::{CredentialStore, LoginKey, StoredAuth},
    error::Result,
    oauth::{Grant, KeyLifetime, KeyName, Permissions},
};
use chrono::{DateTime, Utc};

/// The key that a sign-in saved.
#[derive(Clone, Debug)]
pub struct SavedLogin {
    pub name: KeyName,
    pub valid_until: DateTime<Utc>,
    pub permissions: Permissions,
    pub replaced: Option<ReplacedKey>,
    /// Every stored key after the save, so the caller can offer the switch
    /// without a second read.
    pub stored: StoredAuth,
}

/// The sign-in key that a new one replaced, and whether Kraken deleted it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplacedKey {
    /// A stream or session still signing with it stops working.
    Deleted(KeyName),
    /// The delete failed: the key stays on the account until it expires.
    Kept(KeyName),
}

impl ReplacedKey {
    pub fn name(&self) -> &KeyName {
        match self {
            Self::Deleted(name) | Self::Kept(name) => name,
        }
    }

    pub fn is_deleted(&self) -> bool {
        matches!(self, Self::Deleted(_))
    }
}

/// Creates a key with `grant`, saves it as the sign-in key, then deletes the
/// sign-in key that it replaces. Saving first means a failure cannot remove
/// the only key that works, and an unsaved key is deleted again. The replaced
/// key is read under the store's lock, so two sign-ins at once leave none
/// behind. Store I/O runs inline: a sign-in is a one-shot interactive command.
pub async fn save_login_key(
    grant: &Grant,
    store: &CredentialStore,
    lifetime: KeyLifetime,
) -> Result<SavedLogin> {
    let name = KeyName::generate()?;
    let key = match grant.create_key(&name, lifetime).await {
        Ok(key) => key,
        Err(error) => {
            if let Some(held) = error.key_on_account() {
                delete_key(grant, held, NOT_HELD).await;
            }
            return Err(error);
        }
    };
    let (stored, replaced) = match store.update(|stored| Ok(stored.set_login(LoginKey::from(&key))))
    {
        Ok(saved) => saved,
        Err(error) => {
            delete_key(grant, key.name(), NOT_HELD).await;
            return Err(error);
        }
    };
    let replaced = match replaced {
        Some(replaced) => Some(retire(grant, replaced.name).await),
        None => None,
    };
    Ok(SavedLogin {
        name: key.name().clone(),
        valid_until: key.valid_until(),
        permissions: grant.permissions().clone(),
        replaced,
        stored,
    })
}

const NOT_HELD: &str =
    "a key that kraken-cli does not hold may be left on your account. Delete it in the Kraken UI.";
const REPLACED: &str =
    "could not delete the replaced sign-in key. It expires, or you can delete it in the Kraken UI.";

/// A failed delete is not fatal: `hint` tells the user what is left.
async fn delete_key(grant: &Grant, name: &KeyName, hint: &str) {
    if let Err(error) = grant.delete_key(name).await {
        tracing::warn!(key = %name, %error, "{hint}");
    }
}

/// The new key is saved by now, so a failed delete leaves the old one to
/// expire rather than failing the sign-in.
async fn retire(grant: &Grant, name: KeyName) -> ReplacedKey {
    match grant.delete_key(&name).await {
        Ok(()) => ReplacedKey::Deleted(name),
        Err(error) => {
            tracing::warn!(key = %name, %error, "{REPLACED}");
            ReplacedKey::Kept(name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Error, KeyMismatch, KeyPair, StoredAuth,
        oauth::test_support::{
            grant, grant_timing_out, mount_create, mount_create_reply, mount_delete,
            mount_delete_reply,
        },
    };
    use chrono::Duration;
    use wiremock::{MockServer, ResponseTemplate};

    /// Long enough for a loopback reply, short enough to keep the test fast.
    const CREATE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

    async fn deleted_name(server: &MockServer) -> String {
        let requests = server.received_requests().await.unwrap();
        let delete = requests
            .iter()
            .find(|r| r.method.as_str() == "DELETE")
            .unwrap();
        String::from_utf8_lossy(&delete.body).into_owned()
    }

    fn store_in(dir: &tempfile::TempDir) -> CredentialStore {
        CredentialStore::new(dir.path().join("config.toml"))
    }

    async fn save(grant: &Grant, store: &CredentialStore) -> Result<SavedLogin> {
        save_login_key(grant, store, KeyLifetime::default()).await
    }

    #[tokio::test]
    async fn a_key_that_cannot_be_saved_is_deleted_again() {
        let server = MockServer::start().await;
        let grant = grant(&server).await;
        mount_create(&server, |_| {}).await;
        mount_delete(&server).await;
        // A file where the config directory must be fails the write, even for
        // root, which ignores permissions.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("not-a-dir"), "").unwrap();
        let store = CredentialStore::new(dir.path().join("not-a-dir").join("config.toml"));

        assert!(save(&grant, &store).await.is_err());
        assert!(deleted_name(&server).await.contains("kraken-cli-"));
    }

    #[tokio::test]
    async fn a_key_with_an_unrequested_permission_is_deleted_and_never_stored() {
        let server = MockServer::start().await;
        let grant = grant(&server).await;
        mount_create(&server, |result| {
            result["permissions"]["funds_withdraw"] = true.into();
        })
        .await;
        mount_delete(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);

        let saved = save(&grant, &store).await;

        assert!(
            matches!(&saved, Err(Error::UnexpectedKey { mismatch: KeyMismatch::Permissions(flags), .. }) if flags == &["funds_withdraw"]),
            "{saved:?}"
        );
        assert!(deleted_name(&server).await.contains("kraken-cli-"));
        assert!(store.load().unwrap().login().is_none());
    }

    #[tokio::test]
    async fn a_key_created_under_another_name_is_deleted_by_that_name() {
        let server = MockServer::start().await;
        let grant = grant(&server).await;
        mount_create(&server, |result| {
            result["api_key_name"] = "someone-elses-key".into();
        })
        .await;
        mount_delete(&server).await;
        let dir = tempfile::tempdir().unwrap();

        let saved = save(&grant, &store_in(&dir)).await;

        assert!(
            matches!(saved, Err(Error::UnexpectedKey { .. })),
            "{saved:?}"
        );
        assert!(deleted_name(&server).await.contains("someone-elses-key"));
    }

    fn stored_login_key() -> LoginKey {
        LoginKey {
            pair: KeyPair::new("other", "b3RoZXI="),
            name: "kraken-cli-0therkey".parse().unwrap(),
            valid_until: Utc::now() + Duration::days(1),
        }
    }

    #[tokio::test]
    async fn a_sign_in_that_finished_meanwhile_is_the_one_replaced() {
        let server = MockServer::start().await;
        let grant = grant(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);
        // A second sign-in saves its key while Kraken creates this one.
        let other = store.clone();
        mount_create(&server, move |_| {
            let mut stored = StoredAuth::default();
            stored.set_login(stored_login_key());
            other.save(&stored).unwrap();
        })
        .await;
        mount_delete(&server).await;

        let saved = save(&grant, &store).await.unwrap();

        assert!(deleted_name(&server).await.contains("kraken-cli-0therkey"));
        assert_eq!(
            saved.replaced,
            Some(ReplacedKey::Deleted(stored_login_key().name))
        );
        assert_eq!(store.load().unwrap().login().unwrap().name, saved.name);
    }

    #[tokio::test]
    async fn a_replaced_key_whose_delete_fails_is_reported_as_kept() {
        let server = MockServer::start().await;
        let grant = grant(&server).await;
        mount_create(&server, |_| {}).await;
        mount_delete_reply(
            &server,
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "error": ["EService:Unavailable"] })),
        )
        .await;
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);
        let mut stored = StoredAuth::default();
        stored.set_login(stored_login_key());
        store.save(&stored).unwrap();

        let saved = save(&grant, &store).await.unwrap();

        assert_eq!(
            saved.replaced,
            Some(ReplacedKey::Kept(stored_login_key().name))
        );
        assert_eq!(store.load().unwrap().login().unwrap().name, saved.name);
    }

    /// Each reply leaves the outcome open, so the key is deleted by the name
    /// that was requested.
    #[tokio::test]
    async fn an_unconfirmed_create_deletes_the_key_by_name() {
        for reply in [
            ResponseTemplate::new(502).set_body_string("<html>bad gateway</html>"),
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "error": ["EService:Unavailable"] })),
        ] {
            let server = MockServer::start().await;
            let grant = grant(&server).await;
            mount_create_reply(&server, reply).await;
            mount_delete(&server).await;
            let dir = tempfile::tempdir().unwrap();

            let saved = save(&grant, &store_in(&dir)).await;

            assert!(matches!(saved, Err(Error::Unconfirmed { .. })), "{saved:?}");
            assert!(deleted_name(&server).await.contains("kraken-cli-"));
        }
    }

    #[tokio::test]
    async fn a_create_whose_reply_times_out_deletes_the_key_by_name() {
        let server = MockServer::start().await;
        let grant = grant_timing_out(&server, CREATE_TIMEOUT).await;
        mount_create_reply(
            &server,
            ResponseTemplate::new(200).set_delay(CREATE_TIMEOUT * 10),
        )
        .await;
        mount_delete(&server).await;
        let dir = tempfile::tempdir().unwrap();

        let saved = save(&grant, &store_in(&dir)).await;

        assert!(
            matches!(&saved, Err(Error::Unconfirmed { cause, .. }) if matches!(**cause, Error::Http(_))),
            "{saved:?}"
        );
        let requests = server.received_requests().await.unwrap();
        let create: serde_json::Value = serde_json::from_slice(
            &requests
                .iter()
                .find(|r| r.method.as_str() == "POST" && r.url.path() == "/oauth/fast-api-key")
                .unwrap()
                .body,
        )
        .unwrap();
        let deleted: serde_json::Value =
            serde_json::from_str(&deleted_name(&server).await).unwrap();
        assert_eq!(deleted["api_key_name"], create["api_key_name"]);
    }

    #[tokio::test]
    async fn a_refusal_in_an_envelope_leaves_nothing_to_delete() {
        let server = MockServer::start().await;
        let grant = grant(&server).await;
        mount_create_reply(
            &server,
            ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "error": ["EOAuth:Max fast API keys reached"] }),
            ),
        )
        .await;
        let dir = tempfile::tempdir().unwrap();

        assert!(save(&grant, &store_in(&dir)).await.is_err());
        let requests = server.received_requests().await.unwrap();
        assert!(requests.iter().all(|r| r.method.as_str() != "DELETE"));
    }

    #[tokio::test]
    async fn a_key_whose_reply_is_unreadable_is_deleted_by_name() {
        let server = MockServer::start().await;
        let grant = grant(&server).await;
        mount_create(&server, |result| {
            result.as_object_mut().unwrap().remove("secret");
        })
        .await;
        mount_delete(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);

        let saved = save(&grant, &store).await;

        assert!(
            matches!(&saved, Err(Error::Unconfirmed { cause, .. }) if matches!(**cause, Error::Incomplete(_)))
        );
        assert!(deleted_name(&server).await.contains("kraken-cli-"));
        assert!(store.load().unwrap().login().is_none());
    }
}
