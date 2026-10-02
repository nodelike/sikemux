//! The computer this core runs on, as a paired device shows it.

use std::sync::OnceLock;

use sikemux_process::user_environment;

use crate::protocol::{BuildChannel, HostInfo};

/// Read once: asking System Information for the model takes a moment.
pub(crate) fn info() -> HostInfo {
    static HOST: OnceLock<HostInfo> = OnceLock::new();
    HOST.get_or_init(|| HostInfo {
        name: computer_name().unwrap_or_else(|| "Mac".into()),
        model: model_name().unwrap_or_else(|| "Mac".into()),
        version: VERSION.into(),
        channel: channel(cfg!(debug_assertions), VERSION),
    })
    .clone()
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

fn computer_name() -> Option<String> {
    let name = output("scutil", &["--get", "ComputerName"])?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

fn model_name() -> Option<String> {
    model_from_profile(&output("system_profiler", &["SPHardwareDataType"])?)
}

/// `system_profiler` names the model ("MacBook Pro"); `hw.model` only gives an
/// identifier such as "Mac17,2" that says nothing about the shape.
fn model_from_profile(profile: &str) -> Option<String> {
    profile
        .lines()
        .find_map(|line| line.trim().strip_prefix("Model Name:"))
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_name_comes_from_the_hardware_overview() {
        let profile = "Hardware:\n\n    Hardware Overview:\n\n      Model Name: MacBook Pro\n      Model Identifier: Mac17,2\n";
        assert_eq!(model_from_profile(profile).as_deref(), Some("MacBook Pro"));
        assert_eq!(model_from_profile("nothing useful"), None);
    }

    #[test]
    fn the_channel_follows_the_build() {
        assert_eq!(channel(true, "0.4.3"), BuildChannel::Dev);
        assert_eq!(channel(false, "0.4.3-nightly.5"), BuildChannel::Nightly);
        assert_eq!(channel(false, "0.4.3"), BuildChannel::Stable);
    }
}
