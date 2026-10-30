//! `sikemux update`: installs the newest release on this host's channel and
//! hands the core over to it, keeping terminals and chats. Root puts the core
//! in place: the person's core asks the update service the installer set up,
//! and `sudo sikemux update` installs it itself. Only the person turns
//! updates off, from Sikemux or as root, since agents run as them too.

#[cfg(target_os = "linux")]
pub mod root;

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::json;
use sikemux_core::client::{probe, CoreClient};
use sikemux_core::host_update::{HostUpdate, UpdateStep};
use sikemux_core::protocol::BuildChannel;

use crate::headless;
use crate::style;

const HAND_OVER_TIMEOUT: Duration = Duration::from_secs(120);

pub fn run(args: &[String]) -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        style::note("On a Mac, the app updates itself and its core with it.");
        return Ok(());
    }
    let switch = match args {
        [] => None,
        [flag] if flag == "--on" => Some(true),
        [flag] if flag == "--off" => Some(false),
        _ => return Err("usage: sikemux update [--on | --off]".into()),
    };
    let root = as_person_of_sudo();
    match switch {
        Some(on) if root => set(on),
        Some(_) => {
            style::note("Only you can turn this host's updates on or off, so agents cannot:");
            style::note("  in Sikemux, from this host's ⋯ under Hosts, or Settings, Devices");
            style::note("  or here, with `sudo sikemux update --on` or `--off`");
            Err("updates were not changed".into())
        }
        None if root => install_as_root(),
        None => update_now(),
    }
}

/// Whether this runs as root. Under sudo, it then acts for the person who
/// ran sudo: their core, their switch.
fn as_person_of_sudo() -> bool {
    // SAFETY: geteuid takes nothing and cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        return false;
    }
    if let Some(home) = sudo_uid().and_then(home_of) {
        // Nothing else runs yet in this process, so no thread reads it meanwhile.
        std::env::set_var("HOME", home);
    }
    true
}

fn sudo_uid() -> Option<u32> {
    std::env::var("SUDO_UID").ok()?.parse().ok()
}

fn home_of(uid: u32) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: getpwuid's record is read at once, before anything else here
    // could overwrite it, and its home is copied out.
    unsafe {
        let user = libc::getpwuid(uid);
        if user.is_null() || (*user).pw_dir.is_null() {
            return None;
        }
        let home = std::ffi::CStr::from_ptr((*user).pw_dir);
        Some(std::ffi::OsStr::from_bytes(home.to_bytes()).into())
    }
}

async fn call(
    socket: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let (core, _events) = CoreClient::connect(socket)
        .await
        .map_err(|error| error.to_string())?;
    core.call(method, params)
        .await
        .map_err(|error| error.to_string())
}

fn update_now() -> Result<(), String> {
    let socket = headless::running()?;
    let updated: HostUpdate = headless::runtime()?.block_on(async {
        let updated = call(&socket, "updateHost", json!({})).await?;
        serde_json::from_value(updated).map_err(|error| error.to_string())
    })?;
    let mut steps = updated.steps.into_iter();
    let last = steps
        .next_back()
        .ok_or("the core said nothing about the update")?;
    steps.for_each(|step| show(&step));
    finish(last)?;
    match updated.handing_over {
        Some(version) => handed_over(&socket, &version),
        None => Ok(()),
    }
}

#[cfg(target_os = "linux")]
fn install_as_root() -> Result<(), String> {
    use sikemux_core::protocol::UpdateChannel;
    let channel = match BuildChannel::of_release(env!("CARGO_PKG_VERSION")) {
        BuildChannel::Nightly => UpdateChannel::Nightly,
        BuildChannel::Stable | BuildChannel::Dev => UpdateChannel::Stable,
    };
    let last = root::install(sudo_uid().unwrap_or(0), channel, &mut show);
    let installed = matches!(
        last,
        UpdateStep::Installed { .. } | UpdateStep::Current { .. }
    );
    finish(last)?;
    let socket = headless::launch()?.socket;
    if !installed || probe(&socket, headless::PROBE_TIMEOUT).is_err() {
        return Ok(());
    }
    update_now()
}

#[cfg(not(target_os = "linux"))]
fn install_as_root() -> Result<(), String> {
    unreachable!("only Linux hosts update through root")
}

fn set(on: bool) -> Result<(), String> {
    let socket = headless::launch()?.socket;
    if probe(&socket, headless::PROBE_TIMEOUT).is_err() {
        return Err("the core keeps the switch, and it is not running; start it with `sikemux start`, then try again".into());
    }
    headless::runtime()?.block_on(call(&socket, "setHostUpdates", json!({ "on": on })))?;
    match on {
        true => style::done("Updates on", "this host updates by itself"),
        false => style::done(
            "Updates off",
            &format!("stays on {}", env!("CARGO_PKG_VERSION")),
        ),
    }
    Ok(())
}

/// Waits for the core to come back as `version`, which it does in place.
fn handed_over(socket: &Path, version: &str) -> Result<(), String> {
    let deadline = Instant::now() + HAND_OVER_TIMEOUT;
    while Instant::now() < deadline {
        if probe(socket, headless::PROBE_TIMEOUT).is_ok_and(|hello| hello.build.version == version)
        {
            style::done("Restarted the core", "terminals and chats kept");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!(
        "the core has not come back as {version} yet; `sikemux status` shows why"
    ))
}

pub fn show(step: &UpdateStep) {
    match step {
        UpdateStep::Downloaded { bytes, .. } => style::done(
            "Downloaded the core",
            &format!("{:.1} MB", *bytes as f64 / 1_048_576.0),
        ),
        UpdateStep::Verified => style::done("Verified", "signed by Sikemux"),
        _ => {}
    }
}

fn finish(last: UpdateStep) -> Result<(), String> {
    let channel = |version: &str| BuildChannel::of_release(version).name();
    match last {
        UpdateStep::Installed { from, version } => style::done(
            &format!("Updated sikemux {from} → {version}"),
            channel(&version),
        ),
        UpdateStep::Current { version } => {
            style::done("Up to date", &format!("{version}, {}", channel(&version)))
        }
        UpdateStep::Off => style::note(
            "Updates are off for this host. Turn them on in Sikemux, or with `sudo sikemux update --on`.",
        ),
        UpdateStep::Failed { message } => return Err(message),
        UpdateStep::Downloaded { .. } | UpdateStep::Verified => {}
    }
    Ok(())
}
