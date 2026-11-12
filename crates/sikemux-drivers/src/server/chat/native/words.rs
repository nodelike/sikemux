//! How a tool's names and paths read in the chat, whichever agent ran it.

use std::path::Path;

/// A tool's name read as words, such as `cron_create` as "Cron create".
pub fn spoken(tool: &str) -> String {
    let words = tool.replace(['_', '-'], " ");
    let mut characters = words.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => "Tool".to_owned(),
    }
}

pub fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub fn absolute(path: &str, cwd: &str) -> String {
    if path.starts_with('/') || cwd.is_empty() {
        path.to_owned()
    } else {
        Path::new(cwd).join(path).to_string_lossy().into_owned()
    }
}
