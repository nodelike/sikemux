//! Secrets kept by service and account: in the login Keychain on macOS, and
//! elsewhere in files only the person can read.

use std::fmt;

#[cfg(not(target_os = "macos"))]
mod file;
#[cfg(target_os = "macos")]
mod security;

#[cfg(not(target_os = "macos"))]
use file as store;
#[cfg(target_os = "macos")]
use security as store;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeychainError {
    /// The service, account or secret has a character the keychain cannot take safely.
    Invalid(String),
    /// The keychain could not be reached or would not do it, such as when it is locked.
    Failed(String),
}

impl fmt::Display for KeychainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) | Self::Failed(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for KeychainError {}

pub type KeychainResult<T> = Result<T, KeychainError>;

fn name_like(raw: &str, what: &str) -> KeychainResult<String> {
    let value = raw.trim();
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '@');
    let dots = value.chars().all(|c| c == '.');
    if dots || value.len() > 256 || !value.chars().all(allowed) {
        return Err(KeychainError::Invalid(format!(
            "the keychain {what} is letters, digits and . _ - : @"
        )));
    }
    Ok(value.to_string())
}

/// Secrets reach `security -i` on a command line it splits on spaces, so
/// anything that could end the value early is refused rather than escaped.
fn secret_like(raw: &str) -> KeychainResult<String> {
    let secret = raw.trim();
    let allowed =
        |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '~' | '=' | '+' | '/');
    if secret.is_empty() || secret.len() > 8192 || !secret.chars().all(allowed) {
        return Err(KeychainError::Invalid(
            "that does not look like a token".into(),
        ));
    }
    Ok(secret.to_string())
}

/// Nothing saved reads as `None`; a keychain that will not answer is an error.
pub fn read(service: &str, account: &str) -> KeychainResult<Option<String>> {
    store::read(
        &name_like(service, "service")?,
        &name_like(account, "account")?,
    )
}

pub fn write(service: &str, account: &str, secret: &str) -> KeychainResult<()> {
    store::write(
        &name_like(service, "service")?,
        &name_like(account, "account")?,
        &secret_like(secret)?,
    )
}

/// Deleting an entry that is not there is not an error.
pub fn delete(service: &str, account: &str) -> KeychainResult<()> {
    store::delete(
        &name_like(service, "service")?,
        &name_like(account, "account")?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_keep_the_characters_hosts_put_in_them() {
        for secret in [
            "ghp_abc123",
            "ATATT3xFfGF0abc_def-ghi=A1B2",
            "a+b/c==",
            "x.y.z~",
        ] {
            assert!(secret_like(secret).is_ok(), "{secret}");
        }
    }

    #[test]
    fn anything_that_could_end_the_command_early_is_refused() {
        for secret in ["", "   ", "two words", "a\nb", "a;b", "a\"b", "a'b"] {
            assert!(secret_like(secret).is_err(), "{secret:?}");
        }
        for account in ["", "a b", "a/b", "a\nb", "{uuid}"] {
            assert!(name_like(account, "account").is_err(), "{account:?}");
        }
        assert!(name_like("github.com:nodelike", "account").is_ok());
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn a_saved_secret_reads_back_and_goes_on_delete() {
        let service = "sikemux-keychain-test";
        let account = format!("probe-{}", std::process::id());
        write(service, &account, "s3cret-value").expect("saves");
        assert_eq!(
            read(service, &account).expect("reads").as_deref(),
            Some("s3cret-value")
        );
        delete(service, &account).expect("deletes");
        assert_eq!(read(service, &account).expect("reads"), None);
    }
}
