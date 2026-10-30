//! Folders as the person writes them.

/// This Mac's home folder.
pub fn this_home() -> Option<String> {
    static HOME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        sikemux_process::paths::home().map(|home| home.to_string_lossy().into_owned())
    })
    .clone()
}

/// `path` under `~` when it sits in `home`, the home folder of the host the path is on.
pub fn tilde(path: &str, home: Option<&str>) -> String {
    let rest = home
        .filter(|home| !home.is_empty())
        .and_then(|home| path.strip_prefix(home.trim_end_matches('/')));
    match rest {
        Some("") => "~".into(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_owned(),
    }
}
