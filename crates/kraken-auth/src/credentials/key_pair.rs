use secrecy::{ExposeSecret, SecretString};

/// An API key and its secret. `Debug` redacts both, and both are zeroized on
/// drop.
#[derive(Clone, Debug)]
pub struct KeyPair {
    pub key: SecretString,
    pub secret: SecretString,
}

impl KeyPair {
    pub fn new(key: impl Into<SecretString>, secret: impl Into<SecretString>) -> Self {
        Self {
            key: key.into(),
            secret: secret.into(),
        }
    }

    /// The last four characters of the key, like `****jedl`. It identifies
    /// the key but is not usable. A key of four characters or less is fully
    /// masked.
    pub fn masked_key(&self) -> String {
        let key = self.key.expose_secret();
        let count = key.chars().count();
        if count <= 4 {
            return "****".to_string();
        }
        let tail: String = key.chars().skip(count - 4).collect();
        format!("****{tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_masked_key_shows_only_the_last_four_characters() {
        assert_eq!(KeyPair::new("abcdefghjedl", "s").masked_key(), "****jedl");
    }

    #[test]
    fn a_short_key_is_fully_masked() {
        assert_eq!(KeyPair::new("abcd", "s").masked_key(), "****");
    }

    #[test]
    fn debug_never_prints_the_key_or_the_secret() {
        let rendered = format!("{:?}", KeyPair::new("abcdefghijklmnop", "supersecret12345"));
        assert!(!rendered.contains("mnop") && !rendered.contains("supersecret"));
    }
}
