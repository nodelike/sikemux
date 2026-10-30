//! Closes what a message still being written leaves open at its end, so a reader sees `**bold`
//! as bold rather than as its asterisks until the closing ones arrive.

const ZERO_WIDTH_SPACE: char = '\u{200b}';

/// The last block of a message being written, with its open emphasis, code and links closed and
/// a half-written tag cut, or `None` when it needs none of that. Fenced code stays as written.
pub(crate) fn mend(block: &str) -> Option<String> {
    let start = prose_start(block)?;
    let prose = after(block, start);
    let mended = mend_prose(prose);
    (mended != prose).then(|| format!("{}{mended}", before(block, start)))
}

/// Where the paragraph at the end of `block` starts: after its last blank line or closed fence.
/// `None` while a fence is still open, as all of its text is code.
fn prose_start(block: &str) -> Option<usize> {
    let mut fence: Option<(u8, usize)> = None;
    let mut start = 0;
    let mut at = 0;
    for line in block.split_inclusive('\n') {
        at += line.len();
        let trimmed = line.trim_start_matches(' ');
        let marker = trimmed.as_bytes().first().copied().unwrap_or(0);
        let width = trimmed.bytes().take_while(|byte| *byte == marker).count();
        let is_fence =
            line.len() - trimmed.len() <= 3 && matches!(marker, b'`' | b'~') && width >= 3;
        match fence {
            Some((opened, count)) => {
                if is_fence
                    && marker == opened
                    && width >= count
                    && after(trimmed, width).trim().is_empty()
                {
                    fence = None;
                    start = at;
                }
            }
            None if is_fence => fence = Some((marker, width)),
            None if line.trim().is_empty() => start = at,
            None => {}
        }
    }
    fence.is_none().then_some(start)
}

struct Bracket {
    at: usize,
    /// How many emphasis runs were open before it, which close inside it.
    open: usize,
}

fn mend_prose(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut runs: Vec<&str> = Vec::new();
    let mut brackets: Vec<Bracket> = Vec::new();
    let mut code: Option<(usize, usize)> = None;
    let mut kept = source.len();
    let mut closing = "";
    let mut at = 0;
    let char_len = |at: usize| after(source, at).chars().next().map_or(1, char::len_utf8);
    let run_of = |at: usize, byte: u8| {
        after(source, at)
            .bytes()
            .take_while(|run| *run == byte)
            .count()
    };
    while let Some(&byte) = bytes.get(at) {
        if byte == b'\\' {
            at += 1;
            if at < bytes.len() {
                at += char_len(at);
            }
            continue;
        }
        if byte == b'`' {
            let width = run_of(at, b'`');
            match code {
                Some((open, _)) if open == width => code = None,
                None => code = Some((width, at)),
                _ => {}
            }
            at += width;
            continue;
        }
        if code.is_some() {
            at += char_len(at);
            continue;
        }
        if byte == b'<' && half_written_tag(after(source, at)) {
            kept = at;
            break;
        }
        if byte == b'[' && bytes.get(at + 1) != Some(&b'^') {
            brackets.push(Bracket {
                at,
                open: runs.len(),
            });
        }
        if byte == b']' {
            if let Some(bracket) = brackets.pop() {
                let destination = match bytes.get(at + 1) {
                    Some(b'(') => destination_end(source, at + 2),
                    Some(_) => Some(at),
                    None => None,
                };
                match destination {
                    Some(end) => {
                        at = end + 1;
                        continue;
                    }
                    None if is_image(source, bracket.at) => {
                        kept = bracket.at - 1;
                        runs.truncate(bracket.open);
                    }
                    None if at + 1 < bytes.len() => {
                        kept = at + 2;
                        closing = ")";
                    }
                    None => closing = "()",
                }
                break;
            }
        }
        if matches!(byte, b'*' | b'_' | b'~') {
            let width = run_of(at, byte);
            let previous = before(source, at).chars().next_back();
            let next = after(source, at + width).chars().next();
            if let Some(run) = emphasis_run(source, at, width) {
                if previous.is_some_and(|c| !c.is_whitespace()) && runs.last() == Some(&run) {
                    runs.pop();
                } else if next.is_none() {
                    kept = at;
                    break;
                } else if next.is_some_and(|c| !c.is_whitespace()) {
                    runs.push(run);
                }
            }
            at += width;
            continue;
        }
        at += char_len(at);
    }
    let body = before(source, kept).trim_end();
    let tail = after(before(source, kept), body.len());
    let mut text = body.to_owned();
    text.push_str(closing);
    match code {
        Some((_, start)) if after(body, start).trim_matches('`').trim().is_empty() => {
            text.truncate(start)
        }
        Some((width, _)) => text.push_str(&"`".repeat(width)),
        None => {}
    }
    if let Some(bracket) = brackets.last().filter(|bracket| bracket.at < text.len()) {
        if is_image(source, bracket.at) {
            text.truncate(bracket.at - 1);
            runs.truncate(bracket.open);
        } else if after(&text, bracket.at + 1).trim().is_empty() {
            text.truncate(bracket.at);
            runs.truncate(bracket.open);
        } else {
            for run in runs.drain(bracket.open..).rev() {
                text.push_str(run);
            }
            text.push_str("]()");
        }
    }
    for run in runs.into_iter().rev() {
        text.push_str(run);
    }
    text.push_str(tail);
    let last_line = text.trim_end().rsplit('\n').next().unwrap_or_default();
    if matches!(last_line.trim(), "-" | "--" | "=" | "==") && text.trim_end().contains('\n') {
        // A paragraph's next line that so far reads `-` or `=` would turn it into a heading.
        text.insert(text.trim_end().len(), ZERO_WIDTH_SPACE);
    }
    text
}

/// The run of `*`, `_` or `~` at `at`, when it can open or close emphasis rather than being a
/// list's bullet, a rule or a word's underscore.
fn emphasis_run(text: &str, at: usize, width: usize) -> Option<&str> {
    let byte = text.as_bytes().get(at).copied()?;
    let line_start = before(text, at)
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .trim()
        .is_empty();
    let previous = before(text, at).chars().next_back();
    let next = after(text, at + width).chars().next();
    let rule = width >= 3
        && line_start
        && after(text, at)
            .split('\n')
            .next()
            .unwrap_or_default()
            .chars()
            .all(|c| c == byte as char || c.is_whitespace());
    let bullet = line_start && width == 1 && byte == b'*' && next.is_none_or(char::is_whitespace);
    let in_word = byte == b'_'
        && previous.is_some_and(char::is_alphanumeric)
        && next.is_some_and(char::is_alphanumeric);
    let counts = !rule && !bullet && !in_word && (byte != b'~' || width == 2) && width <= 3;
    text.get(at..at + width).filter(|_| counts)
}

/// Only a tag on the line still being written is cut, so a `<` that never starts one hides at
/// most the rest of that line.
fn half_written_tag(rest: &str) -> bool {
    !rest.contains(['>', '\n'])
        && after(rest, 1)
            .trim_start_matches('/')
            .chars()
            .next()
            .is_none_or(|c| c.is_ascii_alphabetic())
}

fn is_image(text: &str, bracket: usize) -> bool {
    bracket
        .checked_sub(1)
        .and_then(|at| text.as_bytes().get(at))
        == Some(&b'!')
}

fn before(text: &str, at: usize) -> &str {
    text.get(..at).unwrap_or_default()
}

fn after(text: &str, at: usize) -> &str {
    text.get(at..).unwrap_or_default()
}

/// The `)` that closes a link's destination opened just before `from`.
fn destination_end(text: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 1;
    let mut at = from;
    while let Some(byte) = bytes.get(at) {
        match byte {
            b'\\' => at += 1,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}
