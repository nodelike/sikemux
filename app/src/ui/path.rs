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

/// `path` cut to about `most` letters by dropping folders from its middle, so where it starts
/// and the folders it ends in still show: `~/…/services/sikemux`.
pub fn middle(path: &str, most: usize) -> String {
    let count = path.chars().count();
    if count <= most {
        return path.to_owned();
    }
    let parts: Vec<&str> = path.split('/').collect();
    let head = match parts.first() {
        Some(&"") => "",
        Some(first) => first,
        None => return path.to_owned(),
    };
    let mut tail: Vec<&str> = Vec::new();
    let mut used = head.chars().count() + 2;
    for part in parts[1..].iter().rev() {
        let width = part.chars().count() + 1;
        if !tail.is_empty() && used + width > most {
            break;
        }
        used += width;
        tail.push(part);
    }
    if tail.len() + 1 >= parts.len() {
        return path.to_owned();
    }
    tail.reverse();
    format!("{head}/…/{}", tail.join("/"))
}

/// The last part of `path`, or all of it when it has none.
pub fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy())
        .into_owned()
}

/// The extensions of the pictures an agent can be shown.
pub use sikemux_markdown::paths::IMAGE_EXTENSIONS;
