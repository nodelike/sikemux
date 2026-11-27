//! Whether an app build is already on an iOS simulator, so installing it again can be skipped.

use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

/// Whether the device `udid` holds an app with the same files as the `.app` at `app`.
pub(crate) fn already_installed(udid: &str, app: &Path) -> bool {
    let Some(name) = app.file_name() else {
        return false;
    };
    let Some(wanted) = fingerprint(app) else {
        return false;
    };
    let installed = super::load::devices_folder().map(|devices| {
        devices
            .join(udid)
            .join("data/Containers/Bundle/Application")
    });
    let Some(Ok(entries)) = installed.map(std::fs::read_dir) else {
        return false;
    };
    entries
        .flatten()
        .map(|entry| entry.path().join(name))
        .filter(|copy| copy.is_dir())
        .any(|copy| fingerprint(&copy) == Some(wanted))
}

/// Every file's path and bytes in the bundle.
fn fingerprint(app: &Path) -> Option<[u8; 20]> {
    let mut files = Vec::new();
    collect(app, PathBuf::new(), &mut files).ok()?;
    files.sort();
    let mut hash = Sha1::new();
    for path in &files {
        let mut file = std::fs::File::open(app.join(path)).ok()?;
        hash.update(path.as_os_str().as_encoded_bytes());
        hash.update(file.metadata().ok()?.len().to_le_bytes());
        std::io::copy(&mut file, &mut hash).ok()?;
    }
    Some(hash.finalize().into())
}

fn collect(root: &Path, inside: PathBuf, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root.join(&inside))? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = inside.join(entry.file_name());
        if kind.is_dir() {
            collect(root, path, files)?;
        } else if kind.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

/// The app's bundle id, as its Info.plist names it.
pub(crate) fn bundle_id(app: &Path) -> Option<String> {
    let output = sikemux_process::user_environment::command("/usr/bin/plutil")
        .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
        .arg(app.join("Info.plist"))
        .stdin(sikemux_process::command::Stdio::null())
        .stderr(sikemux_process::command::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!id.is_empty()).then_some(id)
}

#[cfg(any(target_os = "macos", all(test, unix)))]
pub(crate) fn url_types_handle_scheme(value: &serde_json::Value, scheme: &str) -> bool {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("CFBundleURLSchemes")?.as_array())
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .any(|declared| declared.eq_ignore_ascii_case(scheme))
}

/// Whether any user-installed app on the iOS simulator `udid` declares `scheme` in its
/// Info.plist `CFBundleURLTypes`. Returns `None` when the simulator's data directory is
/// unavailable (such as non-macOS hosts or fake test devices).
#[cfg(target_os = "macos")]
pub(crate) fn installed_handles_scheme(udid: &str, scheme: &str) -> Option<bool> {
    let data_dir = super::load::devices_folder()?.join(udid).join("data");
    if !data_dir.is_dir() {
        return None;
    }
    let apps_dir = data_dir.join("Containers/Bundle/Application");
    let Ok(containers) = std::fs::read_dir(&apps_dir) else {
        return Some(false);
    };
    for container in containers.flatten() {
        let Ok(entries) = std::fs::read_dir(container.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("app") || !path.is_dir() {
                continue;
            }
            let output = sikemux_process::user_environment::command("/usr/bin/plutil")
                .args(["-extract", "CFBundleURLTypes", "json", "-o", "-"])
                .arg(path.join("Info.plist"))
                .stdin(sikemux_process::command::Stdio::null())
                .stderr(sikemux_process::command::Stdio::null())
                .output()
                .ok()
                .filter(|output| output.status.success());
            if let Some(output) = output {
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                    if url_types_handle_scheme(&value, scheme) {
                        return Some(true);
                    }
                }
            }
        }
    }
    Some(false)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn installed_handles_scheme(_udid: &str, _scheme: &str) -> Option<bool> {
    None
}
