use std::path::{Path, PathBuf};

/// The folders macOS keeps its own tools in. Claude Code reads its sign-in
/// with `/usr/bin/security`, so a PATH without these signs every agent out.
#[cfg(unix)]
pub const SYSTEM_PATH: [&str; 4] = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"];

/// The user's home folder: `HOME`, or `USERPROFILE` on Windows.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|home| !home.is_empty()))
        .map(PathBuf::from)
}

/// `path` with a leading `~`, `~/` or `~\` replaced by the home folder.
pub fn expand_home(path: &str) -> PathBuf {
    let rest = if path == "~" {
        Some("")
    } else {
        path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
    };
    match (rest, home()) {
        (Some(""), Some(home)) => home,
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

/// The path the environment variable `name` holds.
pub fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// `~/.config/sikemux`, where Sikemux keeps its own files, or the folder `SIKEMUX_CONFIG_DIR`
/// names for a Sikemux kept apart from the person's own.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = env_path(crate::vars::CONFIG_DIR) {
        return Some(dir);
    }
    Some(home()?.join(".config").join("sikemux"))
}

/// The endpoint file of the core that started this program, when one handed it over.
pub fn handed_cli_endpoint() -> Option<PathBuf> {
    std::env::var_os(crate::vars::CLI_ENDPOINT)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// A file in `~/.config/sikemux`, named `<stem>.dev.<extension>` in debug builds so a
/// development run never touches an installed build's file.
pub fn config_file(stem: &str, extension: &str) -> Option<PathBuf> {
    let mut name = stem.to_owned();
    if cfg!(debug_assertions) {
        name.push_str(".dev");
    }
    if !extension.is_empty() {
        name.push('.');
        name.push_str(extension);
    }
    Some(config_dir()?.join(name))
}

/// Where a macOS release keeps its core, relative to the app bundle. It is an
/// app of its own because only an app can carry the provisioning profile that
/// lets it use the keychain only Sikemux can read.
pub const BUNDLED_CORE: &str = "Contents/Helpers/Sikemux Core.app/Contents/MacOS/sikemux-host";

/// The app bundle a release's core runs inside; None for any other build.
pub fn app_bundle_of_core(core: &Path) -> Option<&Path> {
    if !core.ends_with(BUNDLED_CORE) {
        return None;
    }
    core.ancestors()
        .nth(Path::new(BUNDLED_CORE).components().count())
}
