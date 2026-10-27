use crate::error::{Error, Result};
use std::{fmt, str::FromStr};

/// The Kraken limit for an API key name.
const MAX_NAME_CHARS: usize = 32;
const GENERATED_PREFIX: &str = "kraken-cli-";

/// A later sign-in uses this name to find and delete the key that it replaces.
#[derive(
    Clone, Debug, PartialEq, Eq, serde_with::DeserializeFromStr, serde_with::SerializeDisplay,
)]
pub struct KeyName(String);

impl KeyName {
    /// `kraken-cli-<8 hex>`, so that users can identify the key in the Kraken
    /// UI.
    pub fn generate() -> Result<Self> {
        let suffix = getrandom::u32().map_err(Error::Random)?;
        Ok(Self(format!("{GENERATED_PREFIX}{suffix:08x}")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for KeyName {
    type Err = Error;

    /// Printable ASCII only: a name reaches the terminal, and every name that
    /// kraken-cli creates is ASCII.
    fn from_str(s: &str) -> Result<Self> {
        if !(1..=MAX_NAME_CHARS).contains(&s.len())
            || !s.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
        {
            return Err(Error::Invalid {
                what: "API key name",
                value: s.to_string(),
            });
        }
        Ok(Self(s.to_string()))
    }
}

impl fmt::Display for KeyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_names_fit_kraken_s_limit() {
        let name = KeyName::generate().unwrap();
        assert!(name.as_str().starts_with(GENERATED_PREFIX));
        assert!(name.as_str().parse::<KeyName>().is_ok());
    }

    #[test]
    fn a_name_longer_than_32_characters_is_invalid() {
        assert!("a".repeat(33).parse::<KeyName>().is_err());
        assert!("".parse::<KeyName>().is_err());
    }

    #[test]
    fn a_name_with_a_control_or_bidi_character_is_invalid() {
        for name in ["kraken-cli-\u{1b}[2J", "kraken-cli-\u{202e}gpj"] {
            assert!(name.parse::<KeyName>().is_err(), "{name:?}");
        }
    }
}
