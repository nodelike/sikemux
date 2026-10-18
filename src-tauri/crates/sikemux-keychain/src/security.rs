//! The login Keychain, reached through the `security` tool so no secret ever
//! sits on a command line.

use std::time::Duration;

use crate::{KeychainError, KeychainResult};

const TIMEOUT: Duration = Duration::from_secs(10);
const OUTPUT_LIMIT: usize = 64 * 1024;
/// What `security` exits with when there is simply no such entry.
const NOT_FOUND: i32 = 44;

fn security(args: &[&str], input: Option<&[u8]>) -> KeychainResult<std::process::Output> {
    sikemux_process::run(
        sikemux_process::user_environment::command("/usr/bin/security").args(args),
        input,
        TIMEOUT,
        OUTPUT_LIMIT,
        None,
    )
    .map_err(|error| KeychainError::Failed(error.to_string()))
}

pub(crate) fn read(service: &str, account: &str) -> KeychainResult<Option<String>> {
    let output = security(
        &["find-generic-password", "-s", service, "-a", account, "-w"],
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
pub(crate) fn write(service: &str, account: &str, secret: &str) -> KeychainResult<()> {
    let line = format!("add-generic-password -U -s {service} -a {account} -w {secret}\n");
    let output = security(&["-i"], Some(line.as_bytes()))?;
    if !output.status.success() {
        return Err(KeychainError::Failed(
            "the keychain refused to save it".into(),
        ));
    }
    Ok(())
}

pub(crate) fn delete(service: &str, account: &str) -> KeychainResult<()> {
    security(
        &["delete-generic-password", "-s", service, "-a", account],
        None,
    )?;
    Ok(())
}
