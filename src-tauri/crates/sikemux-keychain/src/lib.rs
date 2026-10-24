//! Secrets kept by service and account, where no agent can reach them: in
//! the data protection keychain for a build Sikemux's team signed on macOS,
//! and in the guarded store for a core Sikemux's installer set up on Linux. A
//! development build, or the try-out built with its own secrets folder, keeps
//! them in files only the person can read. Any other build keeps none, rather
//! than keep them where agents could read them.

use std::fmt;

mod file;
#[cfg(target_os = "linux")]
mod guarded;
#[cfg(target_os = "macos")]
mod keychain;
mod signature;

pub use signature::{signed_by_team, TEAM_REQUIREMENT};

/// This is the try-out, built to keep its secrets in a folder of its own.
pub const TRYOUT: bool = file::TRYOUT_FOLDER.is_some();

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

/// Run first thing in `main`, before any thread starts. `core` says this run
/// is the core, the only one that keeps the means to reach a guarded store.
pub fn settle(core: bool) {
    #[cfg(target_os = "linux")]
    guarded::settle(core);
    #[cfg(not(target_os = "linux"))]
    let _ = core;
}

enum Store {
    #[cfg(target_os = "macos")]
    Keychain,
    #[cfg(target_os = "linux")]
    Guarded,
    Files,
    Unprotected,
}

fn store() -> Store {
    #[cfg(target_os = "macos")]
    if signed_by_team() {
        return Store::Keychain;
    }
    #[cfg(target_os = "linux")]
    if guarded::active() {
        return Store::Guarded;
    }
    if cfg!(debug_assertions) || TRYOUT {
        Store::Files
    } else {
        Store::Unprotected
    }
}

fn unprotected() -> KeychainError {
    KeychainError::Failed(if cfg!(target_os = "macos") {
        "this copy of Sikemux is not signed by Sikemux, so it keeps no secrets".into()
    } else {
        "Sikemux keeps no secrets on this host until its installer sets the core up to guard them: run `curl -fsSL https://sikemux.com/install | sh`".into()
    })
}

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

fn secret_like(raw: &str) -> KeychainResult<String> {
    let secret = raw.trim();
    if secret.is_empty() || secret.len() > 8192 || secret.chars().any(char::is_control) {
        return Err(KeychainError::Invalid(
            "that does not look like a token".into(),
        ));
    }
    Ok(secret.to_string())
}

/// Nothing saved reads as `None`; a keychain that will not answer is an error.
pub fn read(service: &str, account: &str) -> KeychainResult<Option<String>> {
    let (service, account) = (
        name_like(service, "service")?,
        name_like(account, "account")?,
    );
    match store() {
        #[cfg(target_os = "macos")]
        Store::Keychain => keychain::read(&service, &account),
        #[cfg(target_os = "linux")]
        Store::Guarded => guarded::run(move |root| file::read(root, &service, &account)),
        Store::Files => file::read(&file::home_root()?, &service, &account),
        Store::Unprotected => Err(unprotected()),
    }
}

pub fn write(service: &str, account: &str, secret: &str) -> KeychainResult<()> {
    let (service, account) = (
        name_like(service, "service")?,
        name_like(account, "account")?,
    );
    let secret = secret_like(secret)?;
    match store() {
        #[cfg(target_os = "macos")]
        Store::Keychain => keychain::write(&service, &account, &secret),
        #[cfg(target_os = "linux")]
        Store::Guarded => guarded::run(move |root| file::write(root, &service, &account, &secret)),
        Store::Files => file::write(&file::home_root()?, &service, &account, &secret),
        Store::Unprotected => Err(unprotected()),
    }
}

/// Deleting an entry that is not there is not an error.
pub fn delete(service: &str, account: &str) -> KeychainResult<()> {
    let (service, account) = (
        name_like(service, "service")?,
        name_like(account, "account")?,
    );
    match store() {
        #[cfg(target_os = "macos")]
        Store::Keychain => keychain::delete(&service, &account),
        #[cfg(target_os = "linux")]
        Store::Guarded => guarded::run(move |root| file::delete(root, &service, &account)),
        Store::Files => file::delete(&file::home_root()?, &service, &account),
        Store::Unprotected => Err(unprotected()),
    }
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
    fn a_secret_is_one_line_of_text() {
        assert!(secret_like("two words; \"quoted\"").is_ok());
        for secret in ["", "   ", "a\nb", "a\u{7}b"] {
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
