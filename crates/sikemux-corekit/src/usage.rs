//! What decides whether an install reports its use, and who it reports as.

use std::path::{Path, PathBuf};

use serde::Deserialize;
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
        std::env::var_os(name).is_some_and(|value| {
            let value = value.to_string_lossy().trim().to_ascii_lowercase();
            !["", "0", "false", "no", "off"].contains(&value.as_str())
        })
    })
}

/// Where an install stands on reporting its use and its crashes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sharing {
    On,
    /// The person has not been shown the choice yet.
    Unasked,
    Off,
}

impl Sharing {
    /// The app's first run shows the switch; a host with no screen says so as it is signed in.
    pub fn of(install: Install, shares: bool, onboarded: bool, signed_in: bool) -> Self {
        let asked = match install {
            Install::App => onboarded,
            Install::Host => signed_in,
        };
        match (shares && !do_not_track(), asked) {
            (false, _) => Sharing::Off,
            (true, false) => Sharing::Unasked,
            (true, true) => Sharing::On,
        }
    }

    /// As the core's files say, read without the core.
    pub fn saved(install: Install, data_dir: &Path, socket: Option<&Path>) -> Self {
        #[derive(Default, Deserialize)]
        #[serde(rename_all = "camelCase", default)]
        struct Saved {
            share_usage_data: Option<bool>,
            onboarding_complete: bool,
        }
        let saved = match std::fs::read(data_dir.join("preferences.json")) {
            Ok(bytes) => match serde_json::from_slice::<Saved>(&bytes) {
                Ok(saved) => saved,
                Err(_) => return Sharing::Off,
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Saved::default(),
            Err(_) => return Sharing::Off,
        };
        Self::of(
            install,
            saved.share_usage_data.unwrap_or(true),
            saved.onboarding_complete,
            install == Install::Host && socket.and_then(signed_in_account).is_some(),
        )
    }
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
