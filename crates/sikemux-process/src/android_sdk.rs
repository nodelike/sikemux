use std::path::{Path, PathBuf};

use crate::{paths, user_environment};

/// The Android SDK to use: the one the person's shell names, else one Android
/// Studio installed where it does by default, else the one Sikemux installed
/// in `data_dir`.
pub fn root(data_dir: Option<&Path>) -> Option<PathBuf> {
    let named = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .into_iter()
        .filter_map(user_environment::shell_var)
        .map(|path| paths::expand_home(&path));
    let home = paths::home();
    let studio = [
        home.as_ref().map(|home| home.join("Library/Android/sdk")),
        home.as_ref().map(|home| home.join("Android/Sdk")),
        user_environment::shell_var("LOCALAPPDATA")
            .map(|local| PathBuf::from(local).join("Android").join("Sdk")),
    ];
    let own = data_dir.map(own_root);
    named
        .chain(studio.into_iter().flatten())
        .chain(own)
        .find(|root| is_sdk(root))
}

/// Where Sikemux installs an SDK of its own.
pub fn own_root(data_dir: &Path) -> PathBuf {
    data_dir.join("android").join("sdk")
}

/// The folders of `root` holding `adb` and `emulator`.
pub fn tool_folders(root: &Path) -> [PathBuf; 2] {
    [root.join("platform-tools"), root.join("emulator")]
}

fn is_sdk(root: &Path) -> bool {
    tool_folders(root).iter().any(|folder| folder.is_dir())
}
