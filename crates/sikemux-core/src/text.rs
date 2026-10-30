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
