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
pub(crate) fn url_schemes(value: &serde_json::Value) -> Vec<String> {
    let mut schemes: Vec<String> = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("CFBundleURLSchemes")?.as_array())
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter(|scheme| !scheme.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    schemes.sort();
    schemes.dedup();
    schemes
}

#[cfg(all(test, unix))]
pub(crate) fn url_types_handle_scheme(value: &serde_json::Value, scheme: &str) -> bool {
    url_schemes(value)
        .iter()
        .any(|declared| declared.eq_ignore_ascii_case(scheme))
}

#[cfg(target_os = "macos")]
pub(crate) fn bundle_schemes(app: &Path) -> Vec<String> {
    let output = sikemux_process::user_environment::command("/usr/bin/plutil")
        .args(["-extract", "CFBundleURLTypes", "json", "-o", "-"])
        .arg(app.join("Info.plist"))
        .stdin(sikemux_process::command::Stdio::null())
        .stderr(sikemux_process::command::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success());
    let Some(output) = output else {
        return Vec::new();
    };
    serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .map(|value| url_schemes(&value))
        .unwrap_or_default()
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn bundle_schemes(_app: &Path) -> Vec<String> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn installed_apps_with_schemes(udid: &str) -> Option<Vec<(String, Vec<String>)>> {
    let data_dir = super::load::devices_folder()?.join(udid).join("data");
    if !data_dir.is_dir() {
        return None;
    }
    let apps_dir = data_dir.join("Containers/Bundle/Application");
    let Ok(containers) = std::fs::read_dir(&apps_dir) else {
        return Some(Vec::new());
    };
    let mut apps = Vec::new();
    for container in containers.flatten() {
        let Ok(entries) = std::fs::read_dir(container.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("app") || !path.is_dir() {
                continue;
            }
            let schemes = bundle_schemes(&path);
            if schemes.is_empty() {
                continue;
            }
            let id = bundle_id(&path).unwrap_or_else(|| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_owned()
            });
            apps.push((id, schemes));
        }
    }
    Some(apps)
}

/// Every user-installed app on the iOS simulator `udid` that declares `scheme` in its
/// Info.plist `CFBundleURLTypes`. Returns `None` when the simulator's data directory is
/// unavailable (such as non-macOS hosts or fake test devices).
#[cfg(target_os = "macos")]
pub(crate) fn installed_scheme_claimants(udid: &str, scheme: &str) -> Option<Vec<String>> {
    let apps = installed_apps_with_schemes(udid)?;
    let mut claimants: Vec<String> = apps
        .into_iter()
        .filter(|(_, schemes)| schemes.iter().any(|s| s.eq_ignore_ascii_case(scheme)))
        .map(|(id, _)| id)
        .collect();
    claimants.sort();
    claimants.dedup();
    Some(claimants)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn installed_scheme_claimants(_udid: &str, _scheme: &str) -> Option<Vec<String>> {
    None
}

/// For each scheme in `schemes`, returns `(scheme, sorted_bundle_ids)` whenever more than one
/// installed app on `udid` claims that scheme.
#[cfg(target_os = "macos")]
pub(crate) fn duplicate_scheme_conflicts(
    udid: &str,
    bundle_id: &str,
    schemes: &[String],
) -> Vec<(String, Vec<String>)> {
    if schemes.is_empty() {
        return Vec::new();
    }
    let Some(apps) = installed_apps_with_schemes(udid) else {
        return Vec::new();
    };
    duplicate_scheme_conflicts_from(&apps, bundle_id, schemes)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn duplicate_scheme_conflicts(
    _udid: &str,
    _bundle_id: &str,
    _schemes: &[String],
) -> Vec<(String, Vec<String>)> {
    Vec::new()
}

#[cfg(any(target_os = "macos", all(test, unix)))]
pub(crate) fn duplicate_scheme_conflicts_from(
    installed: &[(String, Vec<String>)],
    bundle_id: &str,
    schemes: &[String],
) -> Vec<(String, Vec<String>)> {
    let mut conflicts = Vec::new();
    for scheme in schemes {
        let mut claimants: Vec<String> = installed
            .iter()
            .filter(|(_, app_schemes)| app_schemes.iter().any(|s| s.eq_ignore_ascii_case(scheme)))
            .map(|(id, _)| id.clone())
            .collect();
        if !bundle_id.is_empty() && !claimants.iter().any(|id| id == bundle_id) {
            claimants.push(bundle_id.to_owned());
        }
        claimants.sort();
        claimants.dedup();
        if claimants.len() > 1 {
            conflicts.push((scheme.to_ascii_lowercase(), claimants));
        }
    }
    conflicts
}

#[cfg(any(target_os = "macos", all(test, unix)))]
pub(crate) fn app_bundle_ancestor(path: &Path) -> Option<&Path> {
    path.ancestors()
        .find(|ancestor| ancestor.extension().and_then(|ext| ext.to_str()) == Some("app"))
}

#[cfg(target_os = "macos")]
pub(crate) fn bundle_id_for_pid(pid: i64) -> Option<String> {
    let pid = u32::try_from(pid).ok()?;
    let cmd = sikemux_process::procs::command_line(pid)?;
    let executable = cmd.first()?;
    let app = app_bundle_ancestor(Path::new(executable))?;
    bundle_id(app)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn bundle_id_for_pid(_pid: i64) -> Option<String> {
    None
}
