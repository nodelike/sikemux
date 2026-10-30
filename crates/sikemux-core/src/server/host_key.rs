//! The host's key, the digest of the devices, links and account it trusts,
//! and the account it belongs to. They live in the person's secret store, out
//! of reach of the agents that can edit `<socket>.remote.json`, so a file this
//! host did not write is caught when the core starts.

use std::path::{Path, PathBuf};

use iroh::SecretKey;
use sha2::{Digest, Sha256};

use super::{CoreError, CoreResult};

const SERVICE: &str = if cfg!(debug_assertions) {
    "sikemux-host-dev"
} else {
    "sikemux-host"
};

#[derive(Clone)]
pub(super) struct HostKey {
    pub secret: SecretKey,
    pub trusted: String,
    /// Kept through signing out, so a program here can sign the host back in
    /// only to the account that let it go.
    pub account: Option<String>,
    /// The key this host kept in `<socket>.remote.json` while on an account,
    /// until the account has let that host go.
    pub retired: Option<SecretKey>,
    /// The digest of the file as last written, while `trusted` is ahead of
    /// it: a write that failed, or the core stopping between the two.
    pub written: Option<String>,
}

pub(super) enum Keeper {
    /// The secret store's entry, named for the socket's whole path so a core
    /// started with another HOME never shares it.
    Store { account: String },
    /// A private file beside the socket, for a core run by tests.
    File(PathBuf),
}

impl Keeper {
    pub(super) fn new(socket: &Path, secret_store: bool) -> CoreResult<Self> {
        if !secret_store {
            let mut path = socket.as_os_str().to_owned();
            path.push(".host");
            return Ok(Self::File(path.into()));
        }
        let digest = Sha256::digest(socket.as_os_str().as_encoded_bytes());
        Ok(Self::Store {
            account: format!("{}-{}", socket_name(socket)?, hex::encode(&digest[..8])),
        })
    }

    /// The entry format 1 data folders kept, named for the socket's file name
    /// alone, which every core with a socket of that name shared.
    pub(super) fn named_by_file(socket: &Path) -> CoreResult<Self> {
        Ok(Self::Store {
            account: socket_name(socket)?.to_owned(),
        })
    }

    pub(super) fn load(&self) -> CoreResult<Option<HostKey>> {
        let text = match self {
            Self::Store { account } => {
                sikemux_keychain::read(SERVICE, account).map_err(|error| error.to_string())?
            }
            Self::File(path) => match std::fs::read_to_string(path) {
                Ok(text) => Some(text),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            },
        };
        Ok(text.as_deref().and_then(decode))
    }

    pub(super) fn save(&self, key: &HostKey) -> CoreResult<()> {
        let text = format!(
            "{}.{}.{}.{}.{}",
            hex::encode(key.secret.to_bytes()),
            key.trusted,
            key.account.as_deref().unwrap_or_default(),
            key.retired
                .as_ref()
                .map(|retired| hex::encode(retired.to_bytes()))
                .unwrap_or_default(),
            key.written.as_deref().unwrap_or_default()
        );
        match self {
            Self::Store { account } => sikemux_keychain::write(SERVICE, account, &text)
                .map_err(|error| CoreError::from(error.to_string())),
            Self::File(path) => {
                sikemux_process::files::write_atomic(path, text.as_bytes(), true)?;
                Ok(())
            }
        }
    }
}

fn socket_name(socket: &Path) -> CoreResult<&str> {
    socket
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CoreError::from("the core's socket has no name"))
}

pub(super) fn secret_from_hex(text: &str) -> Option<SecretKey> {
    let bytes: [u8; 32] = hex::decode(text).ok()?.try_into().ok()?;
    Some(SecretKey::from_bytes(&bytes))
}

fn decode(text: &str) -> Option<HostKey> {
    let mut parts = text.trim().splitn(5, '.');
    let secret = secret_from_hex(parts.next()?)?;
    let trusted = parts.next()?.to_owned();
    let account = parts.next()?;
    Some(HostKey {
        secret,
        trusted,
        account: (!account.is_empty()).then(|| account.to_owned()),
        retired: parts.next().and_then(secret_from_hex),
        written: parts
            .next()
            .filter(|written| !written.is_empty())
            .map(str::to_owned),
    })
}
