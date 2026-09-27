//! Bare web and mail addresses in prose become links, the way GitHub reads them.
//! The first pass follows `micromark-extension-gfm-autolink-literal`, which
//! finds most of them while reading; the second follows the clean-up in
//! `mdast-util-gfm-autolink-literal`, which catches the rest.

use crate::{Element, Node};

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn run_len(text: &str, from: usize, accept: impl Fn(u8) -> bool) -> usize {
    text.as_bytes().get(from..).map_or(0, |rest| {
        rest.iter().take_while(|&&byte| accept(byte)).count()
    })
}

fn starts_with_ignoring_case(text: &str, at: usize, prefix: &str) -> bool {
    text.as_bytes()
        .get(at..at + prefix.len())
        .is_some_and(|slice| slice.eq_ignore_ascii_case(prefix.as_bytes()))
}

/// A link may only start a text, or follow a space or a punctuation mark.
fn may_follow(text: &str, at: usize) -> bool {
    match text.get(..at).and_then(|before| before.chars().next_back()) {
        None => true,
        Some(previous) => !previous.is_alphanumeric(),
    }
}

fn next_boundary(text: &str, at: usize) -> usize {
    text.get(at..)
        .and_then(|rest| rest.chars().next())
        .map_or(text.len(), |c| at + c.len_utf8())
}

fn has_valid_label(label: &str) -> bool {
    !label.contains('_') && label.bytes().any(|byte| byte.is_ascii_alphanumeric())
}

fn is_correct_domain(domain: &str) -> bool {
    let parts: Vec<&str> = domain.split('.').collect();
    if parts.len() < 2 {
        return false;
    }
    parts
        .iter()
        .rev()
        .take(2)
        .all(|part| part.is_empty() || has_valid_label(part))
}

const TRAILING: &[char] = &[
    '!', '"', '&', '\'', ')', ',', '.', ':', ';', '<', '>', '?', ']', '}',
];

/// Punctuation that ends a sentence is not part of the address, except a
/// closing bracket that pairs with one inside it.
fn split_trail(url: &str) -> (String, String) {
    let kept = url.trim_end_matches(TRAILING);
    let mut link = kept.to_owned();
    let mut trail = url.get(kept.len()..).unwrap_or_default().to_owned();
    let opening = link.matches('(').count();
    let mut closing = link.matches(')').count();
    while opening > closing {
        let Some(at) = trail.find(')') else { break };
        let rest = trail.split_off(at + 1);
        link.push_str(&trail);
        trail = rest;
        closing += 1;
    }
    (link, trail)
}

struct Found {
    end: usize,
    href: String,
    label: String,
    trail: String,
}

fn web_address(text: &str, at: usize) -> Option<Found> {
    let (protocol_len, www) = if starts_with_ignoring_case(text, at, "https://") {
        (8, false)
    } else if starts_with_ignoring_case(text, at, "http://") {
        (7, false)
    } else if starts_with_ignoring_case(text, at, "www.") {
        (3, true)
    } else {
        return None;
    };
    let domain_start = at + protocol_len;
    let domain_len = run_len(text, domain_start, |byte| {
        is_word(byte) || byte == b'-' || byte == b'.'
    });
    if domain_len == 0 {
        return None;
    }
    let path_start = domain_start + domain_len;
    let path_len = text.get(path_start..).map_or(0, |rest| {
        rest.find([' ', '\t', '\r', '\n']).unwrap_or(rest.len())
    });
    let end = path_start + path_len;
    if !may_follow(text, at) {
        return None;
    }
    let (protocol, domain) = if www {
        ("", text.get(at..path_start)?)
    } else {
        (
            text.get(at..domain_start)?,
            text.get(domain_start..path_start)?,
        )
    };
    if !is_correct_domain(domain) {
        return None;
    }
    let (link, trail) = split_trail(text.get(if www { at } else { domain_start }..end)?);
    if link.is_empty() {
        return None;
    }
    Some(Found {
        end,
        href: format!("{}{protocol}{link}", if www { "http://" } else { "" }),
        label: format!("{protocol}{link}"),
        trail,
    })
}

fn mail_address(text: &str, at: usize) -> Option<Found> {
    if !may_follow(text, at) || text.get(..at).is_some_and(|before| before.ends_with('/')) {
        return None;
    }
    let local_len = run_len(text, at, |byte| {
        is_word(byte) || matches!(byte, b'-' | b'.' | b'+')
    });
    if local_len == 0 || text.as_bytes().get(at + local_len) != Some(&b'@') {
        return None;
    }
    let domain_part = |byte: u8| is_word(byte) || byte == b'-';
    let label_start = at + local_len + 1;
    let mut end = label_start + run_len(text, label_start, domain_part);
    if end == label_start {
        return None;
    }
    let mut segments = 0;
    while text.as_bytes().get(end) == Some(&b'.') {
        let len = run_len(text, end + 1, domain_part);
        if len == 0 {
            break;
        }
        end += 1 + len;
        segments += 1;
    }
    if segments == 0 {
        return None;
    }
    let address = text.get(at..end)?;
    if address.ends_with(|c: char| c == '-' || c == '_' || c.is_ascii_digit()) {
        return None;
    }
    Some(Found {
        end,
        href: format!("mailto:{address}"),
        label: address.to_owned(),
        trail: String::new(),
    })
}

fn link(href: String, label: String) -> Node {
    Node::Element(Element::A {
        href,
        title: None,
        c: vec![Node::Text(label)],
    })
}

fn replace(text: &str, find: fn(&str, usize) -> Option<Found>, out: &mut Vec<Node>) -> bool {
    let mut changed = false;
    let mut kept = 0;
    let mut at = 0;
    while at < text.len() {
        let Some(found) = find(text, at) else {
            at = next_boundary(text, at);
            continue;
        };
        if at > kept {
            out.push(Node::Text(
                text.get(kept..at).unwrap_or_default().to_owned(),
            ));
        }
        out.push(link(found.href, found.label));
        if !found.trail.is_empty() {
            out.push(Node::Text(found.trail));
        }
        changed = true;
        kept = found.end;
        at = found.end;
    }
    if changed && kept < text.len() {
        out.push(Node::Text(text.get(kept..).unwrap_or_default().to_owned()));
    }
    changed
}

fn char_at(text: &str, at: usize) -> Option<char> {
    text.get(at..).and_then(|rest| rest.chars().next())
}

fn char_before(text: &str, at: usize) -> Option<char> {
    text.get(..at).and_then(|before| before.chars().next_back())
}

/// Unicode punctuation and symbols, as micromark counts them.
fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation() || (!c.is_ascii() && !c.is_alphanumeric() && !c.is_whitespace())
}

fn is_atext(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.' | '_')
}

/// Whether everything from `at` is punctuation that closes a sentence rather
/// than continues an address.
fn is_trail(text: &str, mut at: usize) -> bool {
    loop {
        match char_at(text, at) {
            None | Some('<') => return true,
            Some(c) if c.is_whitespace() => return true,
            Some('!' | '"' | '\'' | ')' | '*' | ',' | '.' | ':' | ';' | '?' | '_' | '~') => at += 1,
            Some('&') => {
                let name = run_len(text, at + 1, |byte| byte.is_ascii_alphabetic());
                if name == 0 || text.as_bytes().get(at + 1 + name) != Some(&b';') {
                    return false;
                }
                at += name + 2;
            }
            Some(']') => {
                at += 1;
                match char_at(text, at) {
                    None | Some('(' | '[') => return true,
                    Some(c) if c.is_whitespace() => return true,
                    _ => {}
                }
            }
            Some(_) => return false,
        }
    }
}

fn domain_end(text: &str, mut at: usize) -> Option<usize> {
    let mut underscore_in_last = false;
    let mut underscore_in_last_but_one = false;
    let mut seen = false;
    while let Some(c) = char_at(text, at) {
        if c == '.' || c == '_' {
            if is_trail(text, at) {
                break;
            }
            if c == '_' {
                underscore_in_last = true;
            } else {
                underscore_in_last_but_one = underscore_in_last;
                underscore_in_last = false;
            }
        } else if c.is_whitespace() || (c != '-' && is_punctuation(c)) {
            break;
        } else {
            seen = true;
        }
        at += c.len_utf8();
    }
    (seen && !underscore_in_last && !underscore_in_last_but_one).then_some(at)
}

fn path_end(text: &str, mut at: usize) -> usize {
    let mut opened = 0;
    let mut closed = 0;
    while let Some(c) = char_at(text, at) {
        match c {
            _ if c.is_whitespace() => break,
            '(' => opened += 1,
            ')' if closed < opened => closed += 1,
            '!' | '"' | '&' | '\'' | ')' | '*' | ',' | '.' | ':' | ';' | '<' | '?' | ']' | '_'
            | '~' => {
                if is_trail(text, at) {
                    break;
                }
                if c == ')' {
                    closed += 1;
                }
            }
            _ => {}
        }
        at += c.len_utf8();
    }
    at
}

fn found(text: &str, at: usize, end: usize, prefix: &str) -> Option<Found> {
    let label = text.get(at..end)?.to_owned();
    Some(Found {
        end,
        href: format!("{prefix}{label}"),
        label,
        trail: String::new(),
    })
}

fn read_email(text: &str, at: usize) -> Option<Found> {
    if char_before(text, at).is_some_and(|previous| previous == '/' || is_atext(previous)) {
        return None;
    }
    let local = run_len(text, at, |byte| is_atext(byte as char));
    if local == 0 || text.as_bytes().get(at + local) != Some(&b'@') {
        return None;
    }
    let mut end = at + local + 1;
    let (mut data, mut dot) = (false, false);
    loop {
        match text.as_bytes().get(end) {
            Some(b'.')
                if text
                    .as_bytes()
                    .get(end + 1)
                    .is_some_and(u8::is_ascii_alphanumeric) =>
            {
                dot = true
            }
            Some(&byte) if byte == b'-' || byte == b'_' || byte.is_ascii_alphanumeric() => {
                data = true
            }
            _ => break,
        }
        end += 1;
    }
    let ends_in_letter = text
        .as_bytes()
        .get(end - 1)
        .is_some_and(u8::is_ascii_alphabetic);
    (data && dot && ends_in_letter)
        .then(|| found(text, at, end, "mailto:"))
        .flatten()
}

fn read_www(text: &str, at: usize) -> Option<Found> {
    let follows = match char_before(text, at) {
        None => true,
        Some(previous) => matches!(
            previous,
            '(' | '*' | '_' | '[' | ']' | '~' | ' ' | '\t' | '\n' | '\r'
        ),
    };
    if !follows || !starts_with_ignoring_case(text, at, "www.") || text.len() <= at + 4 {
        return None;
    }
    let end = path_end(text, domain_end(text, at)?);
    found(text, at, end, "http://")
}

fn read_protocol(text: &str, at: usize) -> Option<Found> {
    if char_before(text, at).is_some_and(|previous| previous.is_ascii_alphabetic()) {
        return None;
    }
    let scheme = run_len(text, at, |byte| byte.is_ascii_alphabetic()).min(5);
    let named = text.get(at..at + scheme)?;
    if !(named.eq_ignore_ascii_case("http") || named.eq_ignore_ascii_case("https"))
        || text.get(at + scheme..at + scheme + 3) != Some("://")
    {
        return None;
    }
    let host = at + scheme + 3;
    let first = char_at(text, host)?;
    if first.is_control() || first.is_whitespace() || is_punctuation(first) {
        return None;
    }
    let end = path_end(text, domain_end(text, host)?);
    found(text, at, end, "")
}

fn read_address(text: &str, at: usize) -> Option<Found> {
    let c = char_at(text, at)?;
    let email = if is_atext(c) {
        read_email(text, at)
    } else {
        None
    };
    email.or_else(|| match c {
        'h' | 'H' => read_protocol(text, at),
        'w' | 'W' => read_www(text, at),
        _ => None,
    })
}

fn each_text(nodes: Vec<Node>, find: fn(&str, usize) -> Option<Found>, out: &mut Vec<Node>) {
    for node in nodes {
        match node {
            Node::Text(text) => {
                if !replace(&text, find, out) {
                    out.push(Node::Text(text));
                }
            }
            other => out.push(other),
        }
    }
}

/// Splits `text` around every address in it.
pub fn link_addresses(text: String, out: &mut Vec<Node>) {
    let mut read = Vec::new();
    each_text(vec![Node::Text(text)], read_address, &mut read);
    let mut webbed = Vec::new();
    each_text(read, web_address, &mut webbed);
    each_text(webbed, mail_address, out);
}
