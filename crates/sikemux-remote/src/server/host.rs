//! The computer this core runs on, as a paired device shows it.

use crate::server::remote_state::RemoteParts;
use std::sync::OnceLock;

use crate::accounts::protocol::Platform;
use sikemux_view::device::HostInfo;
use sikemux_wire::accounts::protocol::{ChannelVersions, MinimumVersions};
use sikemux_wire::protocol::BuildChannel;

pub fn info() -> HostInfo {
    static MODEL: OnceLock<String> = OnceLock::new();
    HostInfo {
        name: known_name().unwrap_or_else(name),
        model: MODEL
            .get_or_init(|| os::model().unwrap_or_else(|| "Host".into()))
            .clone(),
        version: VERSION.into(),
        channel: channel(cfg!(debug_assertions), VERSION),
        home: sikemux_process::paths::home().unwrap_or_default(),
        access: None,
    }
}

static NAME: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// What the computer was called when [`name`] last read it.
pub fn known_name() -> Option<String> {
    (NAME.lock())
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// What the computer is called now, read afresh since the person may rename it while the core runs.
pub fn name() -> String {
    let name = os::name().unwrap_or_else(|| "Host".into());
    *NAME.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(name.clone());
    name
}

/// The computer's new name when the person renamed it since [`name`] last read it.
pub fn renamed() -> Option<String> {
    let Some(before) = known_name() else {
        name();
        return None;
    };
    if !os::may_be_renamed() {
        return None;
    }
    let now = name();
    (now != before).then_some(now)
}

/// What the account calls this host, once the core has heard it.
static ACCOUNT_NAME: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Keeps the name the account has for this host, as the account last said; none once the
/// account no longer lists it.
pub fn note_account_name(name: Option<String>) {
    *ACCOUNT_NAME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = name;
}

/// What this host is called where the person sees it: its name on the account, which the
/// person may have chosen, else what the computer is called.
pub fn shown_name(core: &super::Core) -> String {
    let named = (ACCOUNT_NAME.lock())
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .filter(|_| core.remote().owner().is_some());
    named.unwrap_or_else(|| info().name)
}

pub const PLATFORM: Platform = os::PLATFORM;

pub fn minimum_version(versions: &MinimumVersions) -> &ChannelVersions {
    match PLATFORM {
        Platform::Linux => &versions.linux,
        _ => &versions.macos,
    }
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn channel(debug: bool, version: &str) -> BuildChannel {
    if debug {
        BuildChannel::Dev
    } else {
        BuildChannel::of_release(version)
    }
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(target_os = "macos")]
mod os {
    use std::sync::Mutex;
    use std::time::SystemTime;

    use sikemux_process::user_environment;

    use super::{non_empty, Platform};

    pub const PLATFORM: Platform = Platform::Macos;

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

    pub fn name() -> Option<String> {
        non_empty(&output("/usr/sbin/scutil", &["--get", "ComputerName"])?)
    }

    /// Whether the system preferences, which hold the computer's name, were written since last
    /// asked, so the name is read again only then.
    pub fn may_be_renamed() -> bool {
        static WRITTEN: Mutex<Option<SystemTime>> = Mutex::new(None);
        let written =
            std::fs::metadata("/Library/Preferences/SystemConfiguration/preferences.plist")
                .and_then(|meta| meta.modified())
                .ok();
        let mut seen = WRITTEN
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let changed = written.is_none() || *seen != written;
        *seen = written;
        changed
    }

    pub fn model() -> Option<String> {
        model_from_profile(&output(
            "/usr/sbin/system_profiler",
            &["SPHardwareDataType"],
        )?)
    }

    /// `system_profiler` names the model ("MacBook Pro"); `hw.model` only gives an
    /// identifier such as "Mac17,2" that says nothing about the shape.
    pub fn model_from_profile(profile: &str) -> Option<String> {
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

    pub const PLATFORM: Platform = Platform::Linux;

    fn read(path: &str) -> Option<String> {
        non_empty(fs::read_to_string(path).ok()?.trim_end_matches('\0'))
    }

    pub fn name() -> Option<String> {
        read("/proc/sys/kernel/hostname")
    }

    pub fn may_be_renamed() -> bool {
        true
    }

    /// The board's product name when the firmware fills it in, the board model on
    /// single-board computers, and otherwise the distribution.
    pub fn model() -> Option<String> {
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
    use super::{non_empty, Platform};

    pub const PLATFORM: Platform = Platform::Unknown;

    pub fn name() -> Option<String> {
        non_empty(&std::env::var("COMPUTERNAME").ok()?)
    }

    pub fn may_be_renamed() -> bool {
        false
    }

    pub fn model() -> Option<String> {
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

#[cfg(target_os = "macos")]
pub fn os_version() -> Option<String> {
    let mut buffer = [0u8; 32];
    let mut length = buffer.len();
    // SAFETY: the name is nul-terminated and sysctl writes at most `length`
    // bytes into `buffer`, which has that many.
    let status = unsafe {
        libc::sysctlbyname(
            c"kern.osproductversion".as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return None;
    }
    std::ffi::CStr::from_bytes_until_nul(&buffer[..length.min(buffer.len())])
        .ok()?
        .to_str()
        .ok()
        .map(str::to_owned)
}

#[cfg(not(target_os = "macos"))]
pub fn os_version() -> Option<String> {
    None
}
