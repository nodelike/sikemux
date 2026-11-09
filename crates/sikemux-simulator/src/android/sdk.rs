//! Where the Android SDK's tools, virtual devices and running emulators are.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sikemux_process::{android_sdk, paths, user_environment};

pub(crate) fn root(data_dir: Option<&Path>) -> Option<PathBuf> {
    android_sdk::root(data_dir)
}

pub(crate) fn tool(root: &Path, folder: &str, name: &str) -> PathBuf {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    root.join(folder).join(file)
}

pub(crate) fn adb(root: &Path) -> PathBuf {
    tool(root, "platform-tools", "adb")
}

pub(crate) fn emulator(root: &Path) -> PathBuf {
    tool(root, "emulator", "emulator")
}

/// Where virtual devices live: `ANDROID_AVD_HOME`, else `.android/avd` under
/// `ANDROID_USER_HOME` or the home folder.
pub(crate) fn avd_home() -> Option<PathBuf> {
    if let Some(home) = user_environment::shell_var("ANDROID_AVD_HOME") {
        return Some(paths::expand_home(&home));
    }
    let user = user_environment::shell_var("ANDROID_USER_HOME")
        .map(|home| paths::expand_home(&home))
        .or_else(|| paths::home().map(|home| home.join(".android")))?;
    Some(user.join("avd"))
}

/// A virtual device, as its `.ini` and `config.ini` describe it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Avd {
    pub id: String,
    pub name: String,
    pub api: Option<String>,
}

pub(crate) fn avds() -> Vec<Avd> {
    let Some(home) = avd_home() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&home) else {
        return Vec::new();
    };
    let mut found: Vec<Avd> = entries
        .flatten()
        .filter_map(|entry| {
            let file = entry.path();
            if file.extension().and_then(|ext| ext.to_str()) != Some("ini") {
                return None;
            }
            let id = file.file_stem()?.to_str()?.to_owned();
            let pointer = ini(&std::fs::read_to_string(&file).ok()?);
            let folder = pointer
                .get("path")
                .map(PathBuf::from)
                .filter(|folder| folder.is_dir())
                .unwrap_or_else(|| home.join(format!("{id}.avd")));
            let config = ini(&std::fs::read_to_string(folder.join("config.ini")).ok()?);
            let name = config
                .get("avd.ini.displayname")
                .cloned()
                .unwrap_or_else(|| id.replace('_', " "));
            let api = pointer
                .get("target")
                .or_else(|| config.get("target"))
                .and_then(|target| target.strip_prefix("android-"))
                .map(str::to_owned);
            Some(Avd { id, name, api })
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

pub(crate) fn ini(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect()
}

/// An emulator that is running now, as it describes itself in its `pid_*.ini`.
#[derive(Clone, Debug)]
pub(crate) struct Running {
    pub avd: String,
    pub serial: String,
    pub grpc: Option<(u16, String)>,
}

/// The folders where running emulators leave their `pid_*.ini`.
fn running_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if cfg!(target_os = "macos") {
        if let Some(home) = paths::home() {
            folders.push(home.join("Library/Caches/TemporaryItems/avd/running"));
        }
    } else if cfg!(windows) {
        if let Some(local) = user_environment::shell_var("LOCALAPPDATA") {
            folders.push(
                PathBuf::from(local)
                    .join("Temp")
                    .join("avd")
                    .join("running"),
            );
        }
    } else {
        if let Some(runtime) = user_environment::shell_var("XDG_RUNTIME_DIR") {
            folders.push(PathBuf::from(runtime).join("avd/running"));
        }
        #[cfg(unix)]
        {
            // SAFETY: getuid has no preconditions and cannot fail.
            let uid = unsafe { libc::getuid() };
            folders.push(PathBuf::from(format!("/run/user/{uid}/avd/running")));
        }
        if let Some(user) = user_environment::shell_var("USER") {
            folders.push(PathBuf::from(format!("/tmp/android-{user}/avd/running")));
        }
    }
    folders
}

pub(crate) fn running() -> Vec<Running> {
    let mut found = Vec::new();
    for folder in running_folders() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let file = entry.path();
            let Some(pid) = file
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("pid_")?.strip_suffix(".ini"))
                .and_then(|pid| pid.parse::<u32>().ok())
            else {
                continue;
            };
            if !alive(pid) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let fields = ini(&text);
            let (Some(avd), Some(port)) = (fields.get("avd.id"), fields.get("port.serial")) else {
                continue;
            };
            let grpc = fields
                .get("grpc.port")
                .and_then(|port| port.parse().ok())
                .zip(fields.get("grpc.token").cloned());
            found.push(Running {
                avd: avd.clone(),
                serial: format!("emulator-{port}"),
                grpc,
            });
        }
    }
    found
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only asks whether the process exists.
    let answer = unsafe { libc::kill(pid as libc::pid_t, 0) };
    answer == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn alive(_: u32) -> bool {
    true
}
