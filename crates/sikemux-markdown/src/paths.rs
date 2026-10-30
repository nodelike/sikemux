//! Names of files in prose and between backticks, read the way the Mac app reads them. Each
//! is only a guess: a reader asks whether the file is there before drawing it as one.

use std::ops::Range;

use crate::{Element, Node};

/// Names a project spells out in full, which are files without an extension.
const EXTENSIONLESS: [&str; 9] = [
    "makefile",
    "dockerfile",
    "license",
    "readme",
    "changelog",
    "procfile",
    "justfile",
    "rakefile",
    "gemfile",
];

fn is_leading_noise(c: char) -> bool {
    matches!(c, '(' | '\'' | '"' | '`' | '[' | '{' | '<')
}

fn is_trailing_noise(c: char) -> bool {
    matches!(
        c,
        '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '`' | ')' | ']' | '}' | '>'
    )
}

/// A bare name that is a file's: one with an extension, or one a project spells out in full.
pub fn is_file_name(text: &str) -> bool {
    has_extension(text) || EXTENSIONLESS.contains(&text.to_lowercase().as_str())
}

fn has_extension(text: &str) -> bool {
    text.rsplit_once('.').is_some_and(|(_, extension)| {
        (1..=8).contains(&extension.len()) && extension.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

/// Starts with a drive letter, as `C:\\` or `C:/` does.
pub fn is_windows_drive(text: &str) -> bool {
    let mut bytes = text.bytes();
    matches!(
        (bytes.next(), bytes.next(), bytes.next()),
        (Some(letter), Some(b':'), Some(b'/' | b'\\')) if letter.is_ascii_alphabetic()
    )
}

/// Starts with a scheme such as `http:`; a drive letter is one character, so a scheme needs two.
pub fn has_scheme(text: &str) -> bool {
    text.split_once(':').is_some_and(|(scheme, _)| {
        let mut bytes = scheme.bytes();
        scheme.len() >= 2
            && bytes
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic())
            && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'))
    })
}

fn is_file_uri(text: &str) -> bool {
    text.get(..7)
        .is_some_and(|start| start.eq_ignore_ascii_case("file://"))
}

/// The part of a word that could name a file, without the sentence's punctuation around it.
fn candidate(word: &str, at: usize) -> Option<Range<usize>> {
    let unled = word.trim_start_matches(is_leading_noise);
    let leading = word.len() - unled.len();
    let inner = unled.trim_end_matches(is_trailing_noise);
    if inner.is_empty() {
        return None;
    }
    let separated = inner.contains('/') || inner.contains('\\');
    if !separated && !is_file_name(inner) {
        return None;
    }
    if has_scheme(inner) && !is_file_uri(inner) && !is_windows_drive(inner) {
        return None;
    }
    Some(at + leading..at + leading + inner.len())
}

/// Every stretch of prose that could name a file, in order, by its bytes.
fn candidates(text: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut start = None;
    for (at, c) in text.char_indices().chain([(text.len(), ' ')]) {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(at),
            (true, Some(from)) => {
                found.extend(text.get(from..at).and_then(|word| candidate(word, from)));
                start = None;
            }
            _ => {}
        }
    }
    found
}

/// Whether the text between a pair of backticks names one file as a whole. Backticks mark
/// where a name starts and ends, so a full path between them may hold spaces.
pub fn names_whole_path(text: &str) -> bool {
    if candidates(text)
        .first()
        .is_some_and(|only| *only == (0..text.len()))
    {
        return true;
    }
    let mut chars = text.chars();
    let absolute = match (chars.next(), chars.next()) {
        (Some('/'), Some(next)) => !next.is_whitespace(),
        (Some('~'), Some('/')) => chars.next().is_some_and(|next| !next.is_whitespace()),
        _ => {
            is_windows_drive(text)
                && text
                    .get(3..)
                    .and_then(|rest| rest.chars().next())
                    .is_some_and(|next| !next.is_whitespace())
        }
    };
    !text.contains('\n') && absolute
}

/// A name in prose, as a path holding its own words.
pub fn path(raw: &str, code: bool) -> Node {
    let shown = if code {
        Node::Element(Element::Code { v: raw.to_owned() })
    } else {
        Node::Text(raw.to_owned())
    };
    Node::Element(Element::Path {
        href: raw.to_owned(),
        code,
        c: vec![shown],
    })
}

/// `text` into `out`, each name in it that could be a file marked as a path.
pub fn mark_paths(text: String, out: &mut Vec<Node>) {
    let found = candidates(&text);
    if found.is_empty() {
        return out.push(Node::Text(text));
    }
    let mut cursor = 0;
    for range in found {
        if let Some(before) = text
            .get(cursor..range.start)
            .filter(|before| !before.is_empty())
        {
            out.push(Node::Text(before.to_owned()));
        }
        if let Some(name) = text.get(range.clone()) {
            out.push(path(name, false));
        }
        cursor = range.end;
    }
    if let Some(after) = text.get(cursor..).filter(|after| !after.is_empty()) {
        out.push(Node::Text(after.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raws(text: &str) -> Vec<&str> {
        candidates(text)
            .into_iter()
            .filter_map(|range| text.get(range))
            .collect()
    }

    #[test]
    fn reads_names_as_the_mac_app_does() {
        assert!(raws(r#"well ... ("") ok"#).is_empty());
        assert_eq!(raws("open <file:///tmp/a.ts>"), ["file:///tmp/a.ts"]);
        assert_eq!(raws("I edited src/a.ts."), ["src/a.ts"]);
        assert_eq!(raws("see (src/a.ts)"), ["src/a.ts"]);
        assert_eq!(raws("src/a.ts:42 fails"), ["src/a.ts:42"]);
        assert_eq!(raws("moved src/a.ts to src/b.ts"), ["src/a.ts", "src/b.ts"]);
        assert_eq!(raws("read it and/or write it, either way"), ["and/or"]);
        assert!(raws("it either works or it does not").is_empty());
        assert!(raws("see https://example.com/a/b for more").is_empty());
    }

    #[test]
    fn takes_whole_names_between_backticks_as_the_mac_app_does() {
        assert!(names_whole_path("src/a.ts"));
        assert!(!names_whole_path("npm run build"));
        assert!(names_whole_path("/work/demo/Shipped Mon 5 + Wed 7.mp4"));
        assert!(names_whole_path("~/Movies/demo cut.mp4"));
        assert!(!names_whole_path("src/demo cut.mp4"));
        assert!(!names_whole_path("/ a"));
    }
}
