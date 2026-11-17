//! Names of files in prose and between backticks, read the way the Mac app reads them. Each
//! is only a guess: a reader asks whether the file is there before drawing it as one.

use std::ops::Range;

use crate::{MdElement, MdNode};

/// The extensions a client treats as pictures.
pub const IMAGE_EXTENSIONS: [&str; 13] = [
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "avif", "tif", "tiff", "heic", "heif",
];

/// Whether `path`, on any computer, names a picture by its extension.
pub fn is_image_path(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, extension)| {
        IMAGE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
    })
}

/// Names a project spells out in full, which are files without an extension.
pub const EXTENSIONLESS: [&str; 9] = [
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

/// A reference longer than this is prose that happens to hold slashes.
const LONGEST: usize = 512;

/// A reference to a file, resolved far enough to open it.
#[derive(Clone, Debug, PartialEq)]
pub struct PathRef {
    /// Absolute, with `/` between its parts.
    pub path: String,
    /// Counted from 1, as a reader counts, when the reference named one.
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// `path:12`, `path:12:5` and `path#L12C5`, as a tool, a compiler and a code host each write
/// the same place.
fn split_place(raw: &str) -> (&str, Option<u32>, Option<u32>) {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    if let Some((body, place)) = raw.rsplit_once("#L").or_else(|| raw.rsplit_once("#l")) {
        let (line, column) = match place.split_once(['C', 'c']) {
            Some((line, column)) => (line, Some(column)),
            None => (place, None),
        };
        if digits(line) && column.is_none_or(digits) {
            return (
                body,
                line.parse().ok(),
                column.and_then(|column| column.parse().ok()),
            );
        }
    }
    let number = |text: &str| -> Option<u32> { digits(text).then(|| text.parse().ok()).flatten() };
    if let Some((rest, last)) = raw.rsplit_once(':') {
        if let Some(last) = number(last) {
            if let Some((body, line)) = rest.rsplit_once(':') {
                if let Some(line) = number(line) {
                    return (body, Some(line), Some(last));
                }
            }
            return (rest, Some(last), None);
        }
    }
    (raw, None, None)
}

pub fn normalized(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_owned()
}

/// A bare name only refers to a file when it carries an extension or is one a project
/// spells out in full; anything else needs a separator.
fn looks_relative(body: &str) -> bool {
    body.contains('/') || body.contains('\\') || body.starts_with('.') || is_file_name(body)
}

fn file_uri_path(uri: &str) -> Option<String> {
    let url = url::Url::parse(uri)
        .ok()
        .filter(|url| url.scheme() == "file")?;
    let path = percent_decoded(url.path())?;
    match url
        .host_str()
        .filter(|host| !host.eq_ignore_ascii_case("localhost"))
    {
        Some(host) => Some(format!("//{host}{path}")),
        None => match path.strip_prefix('/').filter(|rest| is_windows_drive(rest)) {
            Some(drive) => Some(drive.to_owned()),
            None => Some(path),
        },
    }
}

fn percent_decoded(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        if byte == b'%' {
            let hex = std::str::from_utf8(bytes.get(at + 1..at + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(byte);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Reads what an agent wrote as a file reference, or none when it is not shaped like one.
/// Whether it is there is a separate question.
pub fn parse(raw: &str, cwd: &str, home: Option<&str>) -> Option<PathRef> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > LONGEST {
        return None;
    }
    let (body, line, column) = split_place(trimmed);
    if is_file_uri(trimmed) {
        let path = file_uri_path(body)?;
        return Some(PathRef {
            path: normalized(&path),
            line,
            column,
        });
    }
    if body.is_empty() || (has_scheme(body) && !is_windows_drive(body)) {
        return None;
    }
    let expanded = match home {
        Some(home) if body == "~" => home.to_owned(),
        Some(home) => match body
            .strip_prefix('~')
            .filter(|rest| rest.starts_with(['/', '\\']))
        {
            Some(rest) => format!("{home}{rest}"),
            None => body.to_owned(),
        },
        None => body.to_owned(),
    };
    let absolute = expanded.starts_with('/') || is_windows_drive(&expanded);
    if !absolute && (body.contains(char::is_whitespace) || !looks_relative(&expanded)) {
        return None;
    }
    if !absolute && cwd.is_empty() {
        return None;
    }
    let path = if absolute {
        normalized(&expanded)
    } else {
        let rest = expanded.strip_prefix("./").unwrap_or(&expanded);
        let rest = rest.replace('\\', "/");
        [normalized(cwd), rest.trim_matches('/').to_owned()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    };
    (!path.is_empty()).then_some(PathRef { path, line, column })
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
pub fn path(raw: &str, code: bool) -> MdNode {
    let shown = if code {
        MdNode::Element(MdElement::Code {
            v: raw.to_owned(),
            math: false,
        })
    } else {
        MdNode::Text(raw.to_owned())
    };
    MdNode::Element(MdElement::Path {
        href: raw.to_owned(),
        code,
        c: vec![shown],
    })
}

/// `text` into `out`, each name in it that could be a file marked as a path.
pub fn mark_paths(text: String, out: &mut Vec<MdNode>) {
    let found = candidates(&text);
    if found.is_empty() {
        return out.push(MdNode::Text(text));
    }
    let mut cursor = 0;
    for range in found {
        if let Some(before) = text
            .get(cursor..range.start)
            .filter(|before| !before.is_empty())
        {
            out.push(MdNode::Text(before.to_owned()));
        }
        if let Some(name) = text.get(range.clone()) {
            out.push(path(name, false));
        }
        cursor = range.end;
    }
    if let Some(after) = text.get(cursor..).filter(|after| !after.is_empty()) {
        out.push(MdNode::Text(after.to_owned()));
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

    fn at(raw: &str) -> Option<String> {
        parse(raw, "/work/demo", Some("/Users/pat")).map(|found| found.path)
    }

    fn place(raw: &str) -> Option<(String, Option<u32>, Option<u32>)> {
        parse(raw, "/work/demo", Some("/Users/pat"))
            .map(|found| (found.path, found.line, found.column))
    }

    #[test]
    fn reads_references_as_the_mac_app_does() {
        assert_eq!(
            at("src/chat/AgentChatPane.tsx").as_deref(),
            Some("/work/demo/src/chat/AgentChatPane.tsx")
        );
        assert_eq!(at("/etc/hosts").as_deref(), Some("/etc/hosts"));
        assert_eq!(
            place("src/a.ts:42:7"),
            Some(("/work/demo/src/a.ts".into(), Some(42), Some(7)))
        );
        assert_eq!(place("src/a.ts:42").and_then(|found| found.1), Some(42));
        assert_eq!(
            place("file:///work/demo/src/a.ts#L9"),
            Some(("/work/demo/src/a.ts".into(), Some(9), None))
        );
        assert_eq!(
            at("file:///work/demo/my%20notes.md").as_deref(),
            Some("/work/demo/my notes.md")
        );
        assert_eq!(
            at("~/notes/todo.md").as_deref(),
            Some("/Users/pat/notes/todo.md")
        );
        assert_eq!(at("./src/a.ts").as_deref(), Some("/work/demo/src/a.ts"));
        assert_eq!(at("https://example.com/a/b.ts"), None);
        assert_eq!(at("mailto:pat@example.com"), None);
        assert_eq!(at("finally"), None);
        assert_eq!(at("run the build"), None);
        assert_eq!(
            at("package.json").as_deref(),
            Some("/work/demo/package.json")
        );
        assert_eq!(at("Makefile").as_deref(), Some("/work/demo/Makefile"));
        assert_eq!(
            at("/work/demo/Shipped Mon 5 + Wed 7.mp4").as_deref(),
            Some("/work/demo/Shipped Mon 5 + Wed 7.mp4")
        );
        assert_eq!(
            at("~/Movies/demo cut.mp4").as_deref(),
            Some("/Users/pat/Movies/demo cut.mp4")
        );
        assert_eq!(at("demo cut.mp4"), None);
        assert_eq!(
            at("C:\\work\\demo\\a.ts").as_deref(),
            Some("C:/work/demo/a.ts")
        );
        assert_eq!(
            place("src/a.ts#L9C4"),
            Some(("/work/demo/src/a.ts".into(), Some(9), Some(4)))
        );
        assert_eq!(
            place("file:///C:/work/a.ts:3"),
            Some(("C:/work/a.ts".into(), Some(3), None))
        );
        assert_eq!(at("file:///work/%E0%A4%A.ts"), None);
        assert_eq!(at("   "), None);
        assert_eq!(at(&format!("src/{}.ts", "a".repeat(600))), None);
        assert_eq!(at(":12"), None);
        assert_eq!(at(".env").as_deref(), Some("/work/demo/.env"));
        assert_eq!(at(".gitignore").as_deref(), Some("/work/demo/.gitignore"));
        assert_eq!(parse("src/a.ts", "", None), None);
        assert_eq!(
            parse("/etc/hosts", "", None)
                .map(|found| found.path)
                .as_deref(),
            Some("/etc/hosts")
        );
    }
}
