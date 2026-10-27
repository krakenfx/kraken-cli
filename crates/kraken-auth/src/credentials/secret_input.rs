use crate::error::{Error, Result};
use secrecy::SecretString;
use std::{fs, path::PathBuf};

/// Where one secret can come from. [`read_secret`] tries stdin, then the file,
/// then the inline value.
pub struct SecretSource {
    pub stdin: bool,
    pub file: Option<PathBuf>,
    pub inline: Option<SecretString>,
    /// The flag that carries `inline`, named in the warning that the secret
    /// shows in process listings.
    pub inline_flag: &'static str,
}

/// `None` when no source is set. An empty stdin or file is an error, so a
/// missing pipe is clear.
pub fn read_secret(source: SecretSource) -> Result<Option<SecretString>> {
    if source.stdin {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        return non_empty(&line, Error::EmptySecretStdin).map(Some);
    }
    if let Some(path) = source.file {
        let text = fs::read_to_string(&path)?;
        return non_empty(&text, Error::EmptySecretFile(path)).map(Some);
    }
    Ok(source.inline.inspect(|_| {
        tracing::warn!(
            flag = source.inline_flag,
            "a secret on the command line shows in process listings. Prefer the -stdin or -file \
             form of the flag."
        );
    }))
}

fn non_empty(text: &str, empty: Error) -> Result<SecretString> {
    let secret = text.trim();
    if secret.is_empty() {
        return Err(empty);
    }
    Ok(secret.to_string().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    fn file_source(path: PathBuf) -> SecretSource {
        SecretSource {
            stdin: false,
            file: Some(path),
            inline: Some("ignored".into()),
            inline_flag: "--api-secret",
        }
    }

    #[test]
    fn a_secret_file_is_trimmed_and_wins_over_the_inline_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        fs::write(&path, "  c2VjcmV0\n").unwrap();
        let secret = read_secret(file_source(path)).unwrap().unwrap();
        assert_eq!(secret.expose_secret(), "c2VjcmV0");
    }

    #[test]
    fn an_empty_secret_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        fs::write(&path, "\n").unwrap();
        assert!(matches!(
            read_secret(file_source(path)),
            Err(Error::EmptySecretFile(_))
        ));
    }

    #[test]
    fn no_source_reads_nothing() {
        let none = SecretSource {
            stdin: false,
            file: None,
            inline: None,
            inline_flag: "--api-secret",
        };
        assert!(read_secret(none).unwrap().is_none());
    }
}
