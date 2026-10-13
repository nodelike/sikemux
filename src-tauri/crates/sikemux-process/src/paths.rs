use std::path::PathBuf;

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

/// `~/.config/sikemux`, where Sikemux keeps its own files.
pub fn config_dir() -> Option<PathBuf> {
    Some(home()?.join(".config").join("sikemux"))
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
