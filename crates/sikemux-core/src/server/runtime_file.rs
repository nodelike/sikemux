//! `<socket>.runtime.json`: which core runs here, on which account, under which name. Another
//! host reads it over SSH to know this one without finding the `sikemux` command first.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sikemux_wire::protocol::{BuildChannel, PROTOCOL_VERSION};

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Runtime {
    pub pid: u32,
    pub core_id: Option<String>,
    pub version: String,
    pub protocol: u32,
    pub channel: String,
    pub account: Option<String>,
    pub name: String,
}

pub(crate) fn channel_name(channel: BuildChannel) -> &'static str {
    match channel {
        BuildChannel::Stable => "stable",
        BuildChannel::Dev => "dev",
        _ => "nightly",
    }
}

/// The runtime file beside `<socket>.remote.json`.
fn beside(remote_file: &Path) -> Option<PathBuf> {
    let name = remote_file.file_name()?.to_str()?;
    let socket = name.strip_suffix(".remote.json")?;
    Some(remote_file.with_file_name(format!("{socket}.runtime.json")))
}

pub(crate) fn write(remote_file: &Path, account: Option<String>, core_id: Option<String>) {
    let Some(path) = beside(remote_file) else {
        return;
    };
    let host = super::host::info();
    let runtime = Runtime {
        pid: std::process::id(),
        core_id,
        version: host.version,
        protocol: PROTOCOL_VERSION,
        channel: channel_name(host.channel).to_owned(),
        account,
        name: host.name,
    };
    let written = serde_json::to_vec(&runtime)
        .map_err(std::io::Error::other)
        .and_then(|bytes| sikemux_process::files::write_atomic(&path, &bytes, true));
    if let Err(error) = written {
        eprintln!("sikemux core: could not write {}: {error}", path.display());
    }
}

/// Taken away as the core stops, so nothing reads a core that is gone as running.
pub(crate) fn remove(remote_file: &Path) {
    if let Some(path) = beside(remote_file) {
        let _ = std::fs::remove_file(path);
    }
}
