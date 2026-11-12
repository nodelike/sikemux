//! How Claude Code's model names say a context of a million tokens: its own
//! `[1m]` suffix, which the APIs do not take, or a name such as "Opus (1M context)".

const LONG_SUFFIX: &str = "[1m]";

pub const LONG_WINDOW: u64 = 1_000_000;

/// `model` asked for with the long context.
pub fn long(model: &str) -> String {
    format!("{model}{LONG_SUFFIX}")
}

/// The model `name` asks for the long context of, when it does.
pub fn long_of(name: &str) -> Option<&str> {
    name.strip_suffix(LONG_SUFFIX)
}

/// The model as the APIs name it.
pub fn api_name(name: &str) -> &str {
    long_of(name).unwrap_or(name)
}

pub fn names_long_window(name: &str) -> bool {
    name.to_ascii_lowercase()
        .split(|char: char| !char.is_ascii_alphanumeric())
        .any(|word| word == "1m")
}
