/// `text` trimmed, cut to at most `max` characters with an ellipsis when it
/// is longer.
pub(crate) fn clipped(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(max.saturating_sub(1)).collect();
    clipped.push('…');
    clipped
}

/// The first line of `text` with anything in it, [`clipped`].
pub(crate) fn clipped_line(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    clipped(line, max)
}

/// Text someone gave to keep, trimmed; refused when empty or longer than
/// `max` characters, naming `key`.
pub(crate) fn to_keep(text: &str, key: &str, max: usize) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(format!("{key} must say something"));
    }
    if text.chars().count() > max {
        return Err(format!(
            "{key} holds at most {max} characters; keep it to a sentence or two"
        ));
    }
    Ok(text.to_owned())
}

/// `text` with every run of white space, line breaks among them, a single space.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
