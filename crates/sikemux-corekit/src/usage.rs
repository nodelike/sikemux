//! What decides whether an install reports its use, and who it reports as.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// What was installed: the desktop app, or the core alone on a host with no screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Install {
    App,
    Host,
}

impl Install {
    pub fn name(self) -> &'static str {
        match self {
            Install::App => "app",
            Install::Host => "host",
        }
    }

    /// Built by the release workflow, which sets `SIKEMUX_USAGE_REPORTING` to
    /// the kind's name. Only such a build reports usage or updates itself.
    pub fn published(self) -> bool {
        !cfg!(debug_assertions) && option_env!("SIKEMUX_USAGE_REPORTING") == Some(self.name())
    }
}

/// `DO_NOT_TRACK` is set in the environment, or `CI` is, as every CI service sets it, which
/// turns off everything that reports. A build that runs on CI machines is never a person's install.
pub fn do_not_track() -> bool {
    ["DO_NOT_TRACK", "CI"].into_iter().any(|name| {
        std::env::var_os(name)
            .is_some_and(|value| !value.is_empty() && value != "0" && value != "false")
    })
}

/// Where the core at `socket` keeps what it knows of its devices and its account.
pub fn remote_file(socket: &Path) -> PathBuf {
    let mut path = socket.as_os_str().to_owned();
    path.push(".remote.json");
    PathBuf::from(path)
}

/// The account the core at `socket` was last signed in to, read without the core.
fn signed_in_account(socket: &Path) -> Option<String> {
    let file: serde_json::Value =
        serde_json::from_slice(&std::fs::read(remote_file(socket)).ok()?).ok()?;
    file.get("owner")?.as_str().map(str::to_owned)
}

/// The person the core at `socket` is signed in as, named as the usage ping names them.
pub fn signed_in_user(socket: &Path) -> Option<String> {
    signed_in_account(socket).map(|account| account_hash(&account))
}

/// An account as the usage ping names it: only this hash leaves the machine.
pub fn account_hash(account: &str) -> String {
    hex::encode(Sha256::digest(format!("sikemux-usage|{account}")))
}
