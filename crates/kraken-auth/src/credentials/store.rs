use super::{
    StoredAuth,
    auth_table::{self, AUTH_TABLE},
    private_file,
};
use crate::error::{Error, Result};
use serde::Serialize;
use std::{fs, path::PathBuf};

/// The `[auth]` table of the CLI config file. Every write runs under one
/// cross-process lock, keeps the other tables of the file and replaces the
/// file atomically with mode 0600.
#[derive(Clone, Debug)]
pub struct CredentialStore {
    path: PathBuf,
}

impl CredentialStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// A missing file means that no key is stored.
    pub fn load(&self) -> Result<StoredAuth> {
        let mut document = self.read_document()?;
        private_file::warn_if_shared(&self.path);
        auth_table::take(&mut document)
    }

    /// Reads the keys, applies `change` and writes them back under the lock,
    /// so a concurrent writer's key is never lost. Nothing is written when
    /// `change` fails. Returns the keys as written.
    pub fn update<T>(
        &self,
        change: impl FnOnce(&mut StoredAuth) -> Result<T>,
    ) -> Result<(StoredAuth, T)> {
        let _lock = private_file::lock(&self.path)?;
        let mut document = self.read_document()?;
        let mut auth = auth_table::read(&document)?;
        let outcome = change(&mut auth)?;
        auth_table::merge(&mut document, &auth)?;
        self.write_document(&document)?;
        Ok((auth, outcome))
    }

    #[cfg(test)]
    pub(crate) fn save(&self, auth: &StoredAuth) -> Result<()> {
        self.update(|stored| {
            *stored = auth.clone();
            Ok(())
        })
        .map(drop)
    }

    /// Writes another table of the same file, such as the CLI's `[settings]`,
    /// with the same 0600 atomic write. It cannot replace `[auth]`.
    pub fn replace_table(&self, name: &str, table: &impl Serialize) -> Result<()> {
        if name == AUTH_TABLE {
            return Err(Error::Invalid {
                what: "table name",
                value: name.into(),
            });
        }
        let _lock = private_file::lock(&self.path)?;
        let mut document = self.read_document()?;
        document.insert(name.into(), toml::Value::try_from(table)?);
        self.write_document(&document)
    }

    /// Deletes every stored key. The other tables of the file stay.
    pub fn reset(&self) -> Result<()> {
        let _lock = private_file::lock(&self.path)?;
        let mut document = self.read_document()?;
        if document.remove(AUTH_TABLE).is_some() {
            self.write_document(&document)?;
        }
        Ok(())
    }

    fn read_document(&self) -> Result<toml::Table> {
        match fs::read_to_string(&self.path) {
            Ok(text) => Ok(text.parse()?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(toml::Table::new()),
            Err(error) => Err(error.into()),
        }
    }

    /// Callers hold the lock, which created the directory.
    fn write_document(&self, document: &toml::Table) -> Result<()> {
        private_file::write(&self.path, toml::to_string_pretty(document)?.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KeyPair, KeySlot, LoginKey};
    use secrecy::ExposeSecret;

    fn store_with(text: &str) -> (tempfile::TempDir, CredentialStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, text).unwrap();
        (dir, CredentialStore::new(path))
    }

    #[test]
    fn a_config_file_from_an_older_release_loads_unchanged() {
        let (_dir, store) = store_with(
            "[auth]\napi_key = \"own-key\"\napi_secret = \"c2VjcmV0\"\n\n[settings]\ndefault_pair \
             = \"XBTUSD\"\n",
        );
        let auth = store.load().unwrap();
        assert_eq!(auth.own().unwrap().key.expose_secret(), "own-key");
        assert_eq!(auth.active(), Some(KeySlot::ApiKey));
    }

    #[test]
    fn a_syntax_error_never_quotes_the_secret() {
        let (_dir, store) = store_with("[auth]\napi_secret = \"c2VjcmV0\nmore\n");
        let error = store.load().unwrap_err();
        assert!(matches!(error, Error::ConfigSyntax(_)));
        assert!(!error.to_string().contains("c2VjcmV0"), "{error}");
        assert!(!format!("{error:?}").contains("c2VjcmV0"), "{error:?}");
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn a_damaged_login_key_never_costs_your_own_key() {
        let (_dir, store) = store_with(
            "[auth]\napi_key = \"own-key\"\napi_secret = \"c2VjcmV0\"\n\n[auth.sign-in]\napi_key \
             = \"k\"\n",
        );
        let stored = store.load().unwrap();
        assert_eq!(stored.own().unwrap().key.expose_secret(), "own-key");
        assert!(stored.login().is_none());
        assert_eq!(stored.active(), Some(KeySlot::ApiKey));
    }

    #[test]
    fn a_damaged_chosen_login_key_never_falls_back_to_your_own_key() {
        let (_dir, store) = store_with(
            "[auth]\napi_key = \"own-key\"\napi_secret = \"c2VjcmV0\"\nactive = \
             \"sign-in\"\n\n[auth.sign-in]\napi_key = \"k\"\n",
        );
        let stored = store.load().unwrap();
        assert!(stored.own().is_some());
        assert_eq!(stored.active(), None);
    }

    const OWN_KEY_WITH_A_DAMAGED_CHOICE: &str = "[auth]\napi_key = \"own-key\"\napi_secret = \
                                                 \"c2VjcmV0\"\nactive = \"from-a-newer-release\"\n";

    #[test]
    fn a_damaged_choice_never_falls_back_to_your_own_key() {
        let (_dir, store) = store_with(OWN_KEY_WITH_A_DAMAGED_CHOICE);
        let stored = store.load().unwrap();
        assert!(stored.own().is_some());
        assert_eq!(stored.active(), None);
    }

    #[test]
    fn a_write_that_makes_no_choice_keeps_a_damaged_one() {
        let (_dir, store) = store_with(OWN_KEY_WITH_A_DAMAGED_CHOICE);
        store
            .update(|stored| {
                stored.set_futures(KeyPair::new("futures-key", "ZnV0dXJlcw=="));
                Ok(())
            })
            .unwrap();
        let text = fs::read_to_string(store.path()).unwrap();
        assert!(text.contains("from-a-newer-release"), "{text}");
        assert_eq!(store.load().unwrap().active(), None);
    }

    #[test]
    fn a_reset_repairs_a_malformed_auth_table() {
        let (_dir, store) = store_with("[auth]\napi_key = 5\n\n[settings]\noutput = \"table\"\n");
        assert!(store.load().is_err());
        store.reset().unwrap();
        assert!(store.load().unwrap().active().is_none());
        assert!(fs::read_to_string(store.path()).unwrap().contains("output"));
    }

    #[test]
    fn another_table_cannot_replace_the_keys() {
        let (_dir, store) = store_with("");
        assert!(store.replace_table("auth", &toml::Table::new()).is_err());
    }

    #[test]
    fn a_save_keeps_the_other_tables_of_the_file() {
        let (_dir, store) = store_with("[settings]\ndefault_pair = \"XBTUSD\"\n");
        let mut auth = StoredAuth::default();
        auth.set_own(KeyPair::new("own-key", "c2VjcmV0"));
        store.save(&auth).unwrap();

        let text = fs::read_to_string(store.path()).unwrap();
        assert!(text.contains("default_pair = \"XBTUSD\""), "{text}");
        assert!(text.contains("api_key = \"own-key\""), "{text}");
    }

    #[test]
    fn every_stored_key_survives_a_round_trip() {
        let (_dir, store) = store_with("");
        let valid_until = chrono::DateTime::from_timestamp(1_900_000_000, 0).unwrap();
        let mut auth = StoredAuth::default();
        auth.set_own(KeyPair::new("own-key", "b3du"));
        auth.set_futures(KeyPair::new("futures-key", "ZnV0dXJlcw=="));
        auth.set_login(LoginKey {
            pair: KeyPair::new("login-key", "bG9naW4="),
            name: "kraken-cli-0000beef".parse().unwrap(),
            valid_until,
        });
        auth.activate(KeySlot::SignIn).unwrap();
        store.save(&auth).unwrap();

        let loaded = store.load().unwrap();
        let halves = |pair: &KeyPair| {
            (
                pair.key.expose_secret().to_owned(),
                pair.secret.expose_secret().to_owned(),
            )
        };
        assert_eq!(loaded.active(), Some(KeySlot::SignIn));
        assert_eq!(
            halves(loaded.own().unwrap()),
            ("own-key".into(), "b3du".into())
        );
        assert_eq!(
            halves(loaded.futures().unwrap()),
            ("futures-key".into(), "ZnV0dXJlcw==".into())
        );
        let login = loaded.login().unwrap();
        assert_eq!(halves(&login.pair), ("login-key".into(), "bG9naW4=".into()));
        assert_eq!(login.name.as_str(), "kraken-cli-0000beef");
        assert_eq!(login.valid_until, valid_until);
    }

    #[test]
    fn another_table_is_written_beside_the_keys() {
        let (_dir, store) = store_with("[auth]\napi_key = \"own-key\"\napi_secret = \"b3du\"\n");
        let settings: toml::Table = toml::from_str("output = \"json\"").unwrap();
        store.replace_table("settings", &settings).unwrap();

        let text = fs::read_to_string(store.path()).unwrap();
        assert!(text.contains("output = \"json\""), "{text}");
        assert_eq!(
            store.load().unwrap().own().unwrap().key.expose_secret(),
            "own-key"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(store.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_reset_deletes_the_keys_and_keeps_the_settings() {
        let (_dir, store) = store_with(
            "[auth]\napi_key = \"own-key\"\napi_secret = \"c2VjcmV0\"\n\n[settings]\noutput = \
             \"json\"\n",
        );
        store.reset().unwrap();
        let text = fs::read_to_string(store.path()).unwrap();
        assert!(
            !text.contains("own-key") && text.contains("output = \"json\""),
            "{text}"
        );
    }

    #[test]
    fn a_missing_file_means_no_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path().join("absent.toml"));
        assert!(store.load().unwrap().active().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, store) = store_with("");
        store.save(&StoredAuth::default()).unwrap();
        let mode = fs::metadata(store.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn a_world_readable_file_from_an_older_release_is_replaced_by_a_private_one() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, store) = store_with("[settings]\noutput = \"json\"\n");
        fs::set_permissions(store.path(), fs::Permissions::from_mode(0o644)).unwrap();
        store.save(&StoredAuth::default()).unwrap();
        let mode = fs::metadata(store.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn a_new_config_directory_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path().join("kraken").join("config.toml"));
        store.save(&StoredAuth::default()).unwrap();
        let mode = fs::metadata(dir.path().join("kraken"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn an_update_holds_the_lock_that_every_writer_takes() {
        let (dir, store) = store_with("");
        store
            .update(|_| {
                let lock = fs::File::open(dir.path().join("config.toml.lock")).unwrap();
                assert!(matches!(lock.try_lock(), Err(fs::TryLockError::WouldBlock)));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_save_keeps_an_entry_it_cannot_read_or_does_not_know() {
        let (_dir, store) = store_with(
            "[auth]\nfrom_a_newer_release = \"x\"\n\n[auth.sign-in]\napi_key = \"k\"\nname =              \"kraken-cli-0000beef\"\n",
        );
        store
            .update(|stored| {
                stored.set_own(KeyPair::new("own-key", "c2VjcmV0"));
                Ok(())
            })
            .unwrap();
        let text = fs::read_to_string(store.path()).unwrap();
        assert!(text.contains("from_a_newer_release"), "{text}");
        assert!(text.contains("kraken-cli-0000beef"), "{text}");
        assert!(text.contains("own-key"), "{text}");
    }

    #[test]
    fn a_cleared_futures_key_leaves_the_file() {
        let (_dir, store) = store_with(
            "[auth]\napi_key = \"own-key\"\napi_secret = \"c2VjcmV0\"\nfutures_api_key = \
             \"old-futures\"\nfutures_api_secret = \"ZnV0dXJlcw==\"\n",
        );
        store
            .update(|stored| {
                stored.clear_futures();
                Ok(())
            })
            .unwrap();
        let text = fs::read_to_string(store.path()).unwrap();
        assert!(!text.contains("futures"), "{text}");
        assert!(text.contains("own-key"), "{text}");
    }

    #[test]
    fn a_change_that_fails_writes_nothing() {
        let (_dir, store) =
            store_with("[auth]\napi_key = \"own-key\"\napi_secret = \"c2VjcmV0\"\n");
        let failed = store.update(|stored| {
            stored.set_futures(KeyPair::new("futures-key", "ZnV0dXJlcw=="));
            stored.activate(KeySlot::SignIn)
        });
        assert!(matches!(failed, Err(Error::KeyNotStored(KeySlot::SignIn))));
        assert!(store.load().unwrap().futures().is_none());
    }

    #[test]
    fn a_write_leaves_no_temporary_file() {
        let (dir, store) = store_with("");
        store.save(&StoredAuth::default()).unwrap();
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}
