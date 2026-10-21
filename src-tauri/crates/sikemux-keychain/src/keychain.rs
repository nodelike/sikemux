//! The data protection keychain, in the access group Sikemux's provisioning
//! profiles grant only to its own app and core. No other program can read,
//! replace or delete an item there, and an item never leaves this host.

use std::collections::HashSet;
use std::sync::Mutex;

use security_framework::access_control::{ProtectionMode, SecAccessControl};
use security_framework::base::Error;
use security_framework::passwords::{self, PasswordOptions};

use crate::{KeychainError, KeychainResult};

const ACCESS_GROUP: &str = "D577WD6Z5U.com.nodelike.sikemux";
const NOT_FOUND: i32 = -25300;
const MISSING_ENTITLEMENT: i32 = -34018;

fn failed(error: Error) -> KeychainError {
    if error.code() == MISSING_ENTITLEMENT {
        return KeychainError::Failed(
            "this copy of Sikemux is not signed to reach its keychain; install it again".into(),
        );
    }
    KeychainError::Failed(format!("the keychain would not do it: {error}"))
}

fn options(service: &str, account: &str) -> PasswordOptions {
    let mut options = PasswordOptions::new_generic_password(service, account);
    options.use_protected_keychain();
    options.set_access_group(ACCESS_GROUP);
    options
}

pub(crate) fn read(service: &str, account: &str) -> KeychainResult<Option<String>> {
    match passwords::generic_password(options(service, account)) {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).trim().to_owned())
            .filter(|secret| !secret.is_empty())),
        Err(error) if error.code() == NOT_FOUND => legacy::take(service, account),
        Err(error) => Err(failed(error)),
    }
}

pub(crate) fn write(service: &str, account: &str, secret: &str) -> KeychainResult<()> {
    let mut options = options(service, account);
    let protection = SecAccessControl::create_with_protection(
        Some(ProtectionMode::AccessibleAfterFirstUnlockThisDeviceOnly),
        0,
    )
    .map_err(failed)?;
    options.set_access_control(protection);
    passwords::set_generic_password_options(secret.as_bytes(), options).map_err(failed)?;
    legacy::forget(service, account)
}

pub(crate) fn delete(service: &str, account: &str) -> KeychainResult<()> {
    match passwords::delete_generic_password_options(options(service, account)) {
        Err(error) if error.code() != NOT_FOUND => Err(failed(error)),
        _ => legacy::forget(service, account),
    }
}

/// Builds before 0.5.3 kept secrets in the login keychain through the
/// `security` tool, where any program the person runs could read them. Each
/// moves here the first time it is read, and any copy left there goes when
/// it is written or deleted.
mod legacy {
    use super::{HashSet, Mutex};
    use crate::{KeychainError, KeychainResult};
    use sikemux_process::sync::Locked;

    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_secs(10);
    const OUTPUT_LIMIT: usize = 64 * 1024;
    /// What `security` exits with when there is no such entry.
    const NOT_FOUND: i32 = 44;

    /// Entries already looked for in the login keychain by this process.
    static LOOKED: Mutex<Option<HashSet<(String, String)>>> = Mutex::new(None);

    fn first_look(service: &str, account: &str) -> bool {
        LOOKED
            .locked()
            .get_or_insert_with(HashSet::new)
            .insert((service.to_owned(), account.to_owned()))
    }

    fn security(args: &[&str]) -> KeychainResult<std::process::Output> {
        sikemux_process::run(
            sikemux_process::user_environment::command("/usr/bin/security").args(args),
            None,
            TIMEOUT,
            OUTPUT_LIMIT,
            None,
        )
        .map_err(|error| KeychainError::Failed(error.to_string()))
    }

    pub(super) fn take(service: &str, account: &str) -> KeychainResult<Option<String>> {
        if !first_look(service, account) {
            return Ok(None);
        }
        let output = security(&["find-generic-password", "-s", service, "-a", account, "-w"])?;
        if output.status.code() == Some(NOT_FOUND) {
            return Ok(None);
        }
        if !output.status.success() {
            return Err(KeychainError::Failed(format!(
                "could not read the saved token: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let secret = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if secret.is_empty() {
            return Ok(None);
        }
        super::write(service, account, &secret)?;
        remove(service, account)?;
        Ok(Some(secret))
    }

    pub(super) fn forget(service: &str, account: &str) -> KeychainResult<()> {
        if first_look(service, account) {
            remove(service, account)
        } else {
            Ok(())
        }
    }

    fn remove(service: &str, account: &str) -> KeychainResult<()> {
        let output = security(&["delete-generic-password", "-s", service, "-a", account])?;
        if output.status.success() || output.status.code() == Some(NOT_FOUND) {
            Ok(())
        } else {
            Err(KeychainError::Failed(
                "the old copy in the login keychain could not be removed".into(),
            ))
        }
    }
}
