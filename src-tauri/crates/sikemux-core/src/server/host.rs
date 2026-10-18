//! The computer this core runs on, as a paired device shows it.

use std::sync::OnceLock;

use crate::accounts::protocol::{ChannelVersions, MinimumVersions, Platform};
use crate::protocol::{BuildChannel, HostInfo};

/// Read once: asking System Information for the model takes a moment.
pub fn info() -> HostInfo {
    static HOST: OnceLock<HostInfo> = OnceLock::new();
    HOST.get_or_init(|| HostInfo {
        name: os::name().unwrap_or_else(|| "Host".into()),
        model: os::model().unwrap_or_else(|| "Host".into()),
        version: VERSION.into(),
        channel: channel(cfg!(debug_assertions), VERSION),
    })
    .clone()
}

pub const PLATFORM: Platform = os::PLATFORM;

pub(crate) fn minimum_version(versions: &MinimumVersions) -> &ChannelVersions {
    match PLATFORM {
        Platform::Linux => &versions.linux,
        _ => &versions.macos,
    }
}

const VERSION: &str = env!("SIKEMUX_VERSION");

fn channel(debug: bool, version: &str) -> BuildChannel {
    if debug {
        BuildChannel::Dev
    } else if version.contains("-nightly") {
        BuildChannel::Nightly
    } else {
        BuildChannel::Stable
    }
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(target_os = "macos")]
mod os {
    use sikemux_process::user_environment;

    use super::{non_empty, Platform};

    pub(super) const PLATFORM: Platform = Platform::Macos;

    fn output(program: &str, args: &[&str]) -> Option<String> {
        let output = user_environment::command(program)
            .args(args)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub(super) fn name() -> Option<String> {
        non_empty(&output("/usr/sbin/scutil", &["--get", "ComputerName"])?)
    }

    pub(super) fn model() -> Option<String> {
        model_from_profile(&output(
            "/usr/sbin/system_profiler",
            &["SPHardwareDataType"],
        )?)
    }

    /// `system_profiler` names the model ("MacBook Pro"); `hw.model` only gives an
    /// identifier such as "Mac17,2" that says nothing about the shape.
    pub(super) fn model_from_profile(profile: &str) -> Option<String> {
        profile
            .lines()
            .find_map(|line| line.trim().strip_prefix("Model Name:"))
            .and_then(non_empty)
    }
}

#[cfg(target_os = "linux")]
mod os {
    use std::fs;

    use super::{non_empty, Platform};

    pub(super) const PLATFORM: Platform = Platform::Linux;

    fn read(path: &str) -> Option<String> {
        non_empty(fs::read_to_string(path).ok()?.trim_end_matches('\0'))
    }

    pub(super) fn name() -> Option<String> {
        read("/proc/sys/kernel/hostname")
    }

    /// The board's product name when the firmware fills it in, the board model on
    /// single-board computers, and otherwise the distribution.
    pub(super) fn model() -> Option<String> {
        read("/sys/devices/virtual/dmi/id/product_name")
            .filter(|name| !is_placeholder(name))
            .or_else(|| read("/proc/device-tree/model"))
            .or_else(|| pretty_name(&fs::read_to_string("/etc/os-release").ok()?))
    }

    /// Firmware that was never filled in by the board maker reports these.
    fn is_placeholder(name: &str) -> bool {
        let name = name.to_ascii_lowercase();
        [
            "to be filled",
            "system product name",
            "default string",
            "not specified",
            "o.e.m.",
        ]
        .iter()
        .any(|junk| name.contains(junk))
    }

    fn pretty_name(os_release: &str) -> Option<String> {
        os_release
            .lines()
            .find_map(|line| line.strip_prefix("PRETTY_NAME="))
            .and_then(|value| non_empty(value.trim_matches(['"', '\''])))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod os {
    use super::Platform;

    pub(super) const PLATFORM: Platform = Platform::Unknown;

    pub(super) fn name() -> Option<String> {
        None
    }

    pub(super) fn model() -> Option<String> {
        None
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::os::model_from_profile;

    #[test]
    fn the_model_name_comes_from_the_hardware_overview() {
        let profile = "Hardware:\n\n    Hardware Overview:\n\n      Model Name: MacBook Pro\n      Model Identifier: Mac17,2\n";
        assert_eq!(model_from_profile(profile).as_deref(), Some("MacBook Pro"));
        assert_eq!(model_from_profile("nothing useful"), None);
    }
}
