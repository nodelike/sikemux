//! Secrets kept in the system's keychain, by service and account. On macOS
//! this is the login Keychain, reached through the `security` tool so no
//! secret ever sits on a command line.

use std::fmt;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);
const OUTPUT_LIMIT: usize = 64 * 1024;
/// What `security` exits with when there is simply no such entry.
const NOT_FOUND: i32 = 44;

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
    if value.is_empty() || value.len() > 256 || !value.chars().all(allowed) {
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

fn security(args: &[&str], input: Option<&[u8]>) -> KeychainResult<std::process::Output> {
    if !cfg!(target_os = "macos") {
        return Err(KeychainError::Failed(
            "there is no keychain on this system yet".into(),
        ));
    }
    sikemux_process::run(
        sikemux_process::user_environment::command("security").args(args),
        input,
        TIMEOUT,
        OUTPUT_LIMIT,
        None,
    )
    .map_err(|error| KeychainError::Failed(error.to_string()))
}

/// Nothing saved reads as `None`; a keychain that will not answer is an error.
pub fn read(service: &str, account: &str) -> KeychainResult<Option<String>> {
    let service = name_like(service, "service")?;
    let account = name_like(account, "account")?;
    let output = security(
        &[
            "find-generic-password",
            "-s",
            &service,
            "-a",
            &account,
            "-w",
        ],
        None,
    )?;
    if output.status.code() == Some(NOT_FOUND) {
        return Ok(None);
    }
    if !output.status.success() {
        return Err(KeychainError::Failed(format!(
            "could not read the saved token: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let secret = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!secret.is_empty()).then_some(secret))
}

/// `security -i` reads the command from stdin, so the secret never shows up
/// in the process list the way an argument would.
pub fn write(service: &str, account: &str, secret: &str) -> KeychainResult<()> {
    let service = name_like(service, "service")?;
    let account = name_like(account, "account")?;
    let secret = secret_like(secret)?;
    let line = format!("add-generic-password -U -s {service} -a {account} -w {secret}\n");
    let output = security(&["-i"], Some(line.as_bytes()))?;
    if !output.status.success() {
        return Err(KeychainError::Failed(
            "the keychain refused to save it".into(),
        ));
    }
    Ok(())
}

/// Deleting an entry that is not there is not an error.
pub fn delete(service: &str, account: &str) -> KeychainResult<()> {
    let service = name_like(service, "service")?;
    let account = name_like(account, "account")?;
    security(
        &["delete-generic-password", "-s", &service, "-a", &account],
        None,
    )?;
    Ok(())
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

    #[cfg(target_os = "macos")]
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
