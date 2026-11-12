use std::path::PathBuf;

use sikemux_process::{android_sdk, paths, user_environment};

/// Puts the Android SDK's `adb` and `emulator` at the end of the PATH in `env`,
/// an agent's variables, and names the SDK in `ANDROID_HOME` for Gradle when
/// the person's shell names none.
pub fn extend(env: &mut Vec<(String, String)>) {
    let Some(root) = android_sdk::root(paths::config_dir().as_deref()) else {
        return;
    };
    if let Some((_, path)) = env.iter_mut().find(|(key, _)| key == "PATH") {
        let mut folders: Vec<PathBuf> = std::env::split_paths(path.as_str()).collect();
        for folder in android_sdk::tool_folders(&root) {
            if folder.is_dir() && !folders.contains(&folder) {
                folders.push(folder);
            }
        }
        if let Ok(joined) = std::env::join_paths(folders) {
            *path = joined.to_string_lossy().into_owned();
        }
    }
    let named = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .into_iter()
        .any(|name| user_environment::shell_var(name).is_some());
    if !named {
        env.push((
            "ANDROID_HOME".to_owned(),
            root.to_string_lossy().into_owned(),
        ));
    }
}
