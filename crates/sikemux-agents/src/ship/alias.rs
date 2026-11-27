use std::path::{Path, PathBuf};

use crate::git_guard::git_args;

pub(super) enum Expanded {
    Args(Vec<String>),
    Shell(String),
}

pub(super) fn gh_aliases_from_config(
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let Some(file) = gh_config_file(env) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    parse_gh_aliases(&text)
}

fn gh_config_file(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(dir) = env("GH_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir).join("config.yml"));
    }
    if let Some(dir) = env("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir).join("gh").join("config.yml"));
    }
    #[cfg(windows)]
    if let Some(dir) = env("AppData").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir).join("GitHub CLI").join("config.yml"));
    }
    let home = env("HOME")
        .or_else(|| env("USERPROFILE"))
        .filter(|dir| !dir.is_empty())?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("gh")
            .join("config.yml"),
    )
}

pub(super) fn parse_gh_aliases(text: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(start) = lines.iter().position(|line| {
        !line.starts_with([' ', '\t'])
            && line
                .strip_prefix("aliases:")
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t', '#']))
    }) else {
        return Vec::new();
    };
    let after_colon = lines
        .get(start)
        .and_then(|line| line.strip_prefix("aliases:"))
        .unwrap_or_default()
        .trim();
    if let Some(inline) = after_colon
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
    {
        return parse_inline_map(inline);
    }
    let mut aliases = Vec::new();
    let mut entry_indent: Option<usize> = None;
    let mut at = start + 1;
    while let Some(&line) = lines.get(at) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            at += 1;
            continue;
        }
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        if indent == 0 {
            break;
        }
        let base = *entry_indent.get_or_insert(indent);
        if indent != base {
            at += 1;
            continue;
        }
        let Some((raw_key, raw_value)) = split_yaml_pair(trimmed) else {
            at += 1;
            continue;
        };
        let key = unquote_yaml(raw_key);
        let raw_value = raw_value.trim();
        let value = if is_block_scalar(raw_value) {
            let folded = raw_value.starts_with('>');
            let mut block = Vec::new();
            let mut block_indent: Option<usize> = None;
            at += 1;
            while let Some(&next) = lines.get(at) {
                if next.trim().is_empty() {
                    block.push("");
                    at += 1;
                    continue;
                }
                let next_indent = next.len() - next.trim_start_matches([' ', '\t']).len();
                if next_indent <= base {
                    break;
                }
                let strip = *block_indent.get_or_insert(next_indent);
                block.push(next.get(strip.min(next.len())..).unwrap_or_default());
                at += 1;
            }
            while block.last() == Some(&"") {
                block.pop();
            }
            if folded {
                block.join(" ")
            } else {
                block.join("\n")
            }
        } else {
            at += 1;
            unquote_yaml(raw_value)
        };
        if !key.is_empty() && !value.is_empty() {
            upsert(&mut aliases, key, value);
        }
    }
    aliases
}

fn parse_inline_map(inline: &str) -> Vec<(String, String)> {
    let mut aliases = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut entries = Vec::new();
    for ch in inline.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if quote == Some('"') && ch == '\\' {
            current.push(ch);
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            current.push(ch);
            if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                current.push(ch);
            }
            ',' => {
                entries.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        entries.push(current);
    }
    for entry in entries {
        if let Some((k, v)) = split_yaml_pair(entry.trim()) {
            let key = unquote_yaml(k);
            let value = unquote_yaml(v.trim());
            if !key.is_empty() && !value.is_empty() {
                upsert(&mut aliases, key, value);
            }
        }
    }
    aliases
}

fn split_yaml_pair(line: &str) -> Option<(&str, &str)> {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (at, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quote == Some('"') && ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            ':' => {
                let after = line.get(at + 1..)?;
                if after.is_empty() || after.starts_with([' ', '\t']) {
                    return Some((line.get(..at)?.trim(), after));
                }
            }
            _ => {}
        }
    }
    None
}

fn is_block_scalar(value: &str) -> bool {
    matches!(value, "|" | "|-" | "|+" | ">" | ">-" | ">+")
}

fn unquote_yaml(value: &str) -> String {
    let value = value.trim();
    if let Some(inner) = value
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return inner.replace("''", "'");
    }
    if let Some(inner) = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                }
            } else {
                out.push(ch);
            }
        }
        return out;
    }
    match value.split_once(" #") {
        Some((before, _)) => before.trim().to_owned(),
        None => value.to_owned(),
    }
}

fn upsert(aliases: &mut Vec<(String, String)>, key: String, value: String) {
    if let Some((_, kept)) = aliases.iter_mut().find(|(k, _)| k == &key) {
        *kept = value;
    } else {
        aliases.push((key, value));
    }
}

pub(super) fn record_gh_alias(words: &[String], aliases: &mut Vec<(String, String)>) {
    let Some((program, args)) = words.split_first() else {
        return;
    };
    if crate::git_guard::base_name(program) != "gh" {
        return;
    }
    let gh = super::gh_args(args);
    match gh.as_slice() {
        [first, sub, rest @ ..] if first == "alias" && sub == "set" => {
            let mut shell = false;
            let mut positional = Vec::new();
            for word in rest {
                if word == "-s" || word == "--shell" {
                    shell = true;
                } else if !word.starts_with('-') {
                    positional.push(word.as_str());
                }
            }
            if let [name, expansion, ..] = positional.as_slice() {
                let value = if shell && !expansion.starts_with('!') {
                    format!("!{expansion}")
                } else {
                    (*expansion).to_owned()
                };
                upsert(aliases, (*name).to_owned(), value);
            }
        }
        [first, sub, rest @ ..] if first == "alias" && sub == "delete" => {
            if let Some(name) = rest.iter().find(|word| !word.starts_with('-')) {
                aliases.retain(|(key, _)| key != name);
            }
        }
        _ => {}
    }
}

pub(super) fn record_git_alias(words: &[String], aliases: &mut Vec<(String, String)>) {
    let Some((program, args)) = words.split_first() else {
        return;
    };
    if crate::git_guard::base_name(program) != "git" {
        return;
    }
    let sub = git_args(args);
    if sub.first().map(String::as_str) != Some("config") {
        return;
    }
    let mut positional = Vec::new();
    let mut words = sub.iter().skip(1);
    while let Some(word) = words.next() {
        if word == "--" {
            positional.extend(words.map(String::as_str));
            break;
        }
        if matches!(word.as_str(), "-f" | "--file" | "--blob" | "--type") {
            words.next();
            continue;
        }
        if word.starts_with('-') {
            continue;
        }
        positional.push(word.as_str());
    }
    if let [key, value] = positional.as_slice() {
        if let Some((prefix, name)) = key.split_once('.') {
            if prefix.eq_ignore_ascii_case("alias") && !name.is_empty() {
                upsert(aliases, name.to_ascii_lowercase(), (*value).to_owned());
            }
        }
    }
}

pub(super) fn expand_gh(args: &[String], aliases: &[(String, String)]) -> Option<Expanded> {
    if aliases.is_empty() {
        return None;
    }
    let mut prefix_flags = Vec::new();
    let mut at = 0;
    while let Some(word) = args.get(at) {
        if !word.starts_with('-') {
            break;
        }
        prefix_flags.push(word.clone());
        let takes_value =
            !word.contains('=') && !matches!(word.as_str(), "-h" | "--help" | "--version");
        at += 1;
        if takes_value {
            if let Some(value) = args.get(at) {
                prefix_flags.push(value.clone());
                at += 1;
            }
        }
    }
    let rest = args.get(at..)?;
    let command_len = rest
        .iter()
        .take(3)
        .take_while(|word| !word.starts_with('-'))
        .count();
    let (matched_len, expansion) = (1..=command_len).rev().find_map(|len| {
        let candidate = rest.get(..len)?.join(" ");
        aliases
            .iter()
            .rev()
            .find(|(key, _)| key == &candidate)
            .map(|(_, value)| (len, value.as_str()))
    })?;
    let after = rest.get(matched_len..).unwrap_or_default();
    if let Some(shell) = expansion.strip_prefix('!') {
        return Some(Expanded::Shell(substitute_shell(shell, after, false)));
    }
    let words = split_words(expansion);
    let (mut expanded, used) = substitute_words(&words, after);
    expanded.extend(after.get(used..).unwrap_or_default().iter().cloned());
    expanded.extend(prefix_flags);
    Some(Expanded::Args(expanded))
}

pub(super) fn expand_git(
    args: &[String],
    line_aliases: &[(String, String)],
    lookup: &dyn Fn(&[String], &str) -> Option<String>,
) -> Option<Expanded> {
    if super::git_global_help(args) {
        return None;
    }
    let sub = git_args(args);
    let (command, sub_args) = sub.split_first()?;
    if command.starts_with('-') || super::GIT_COMMANDS.contains(&command.as_str()) {
        return None;
    }
    if super::has_program_flag(sub_args, &["--help"], super::GIT_PUSH_SWITCHES, false) {
        return None;
    }
    let global = args.get(..args.len() - sub.len()).unwrap_or_default();
    let stands_for = inline_git_alias(global, command)
        .or_else(|| {
            line_aliases
                .iter()
                .rev()
                .find(|(key, _)| key.eq_ignore_ascii_case(command))
                .map(|(_, value)| value.clone())
        })
        .or_else(|| lookup(global, command))?;
    if let Some(shell) = stands_for.strip_prefix('!') {
        return Some(Expanded::Shell(substitute_shell(shell, sub_args, true)));
    }
    let mut expanded = strip_inline_alias(global, command);
    expanded.extend(split_words(&stands_for));
    expanded.extend(sub_args.iter().cloned());
    Some(Expanded::Args(expanded))
}

/// The folder a git invocation runs in, following any `-C` global options.
pub(super) fn git_run_dir(args: &[String], cwd: &Path) -> PathBuf {
    let sub = git_args(args);
    let global = args.get(..args.len() - sub.len()).unwrap_or_default();
    let mut dir = cwd.to_path_buf();
    let mut at = 0;
    while let Some(word) = global.get(at) {
        if word == "-C" {
            if let Some(moved) = global.get(at + 1) {
                dir = dir.join(moved);
            }
            at += 2;
        } else if let Some(moved) = word.strip_prefix("-C") {
            if !moved.is_empty() {
                dir = dir.join(moved);
            }
            at += 1;
        } else {
            at += 1;
        }
    }
    dir
}

fn inline_git_alias(global: &[String], name: &str) -> Option<String> {
    let mut found = None;
    let mut at = 0;
    while let Some(word) = global.get(at) {
        let pair = if word == "-c" {
            at += 1;
            global.get(at).map(String::as_str)
        } else {
            word.strip_prefix("-c").filter(|rest| !rest.is_empty())
        };
        at += 1;
        if let Some((key, value)) = pair.and_then(|pair| pair.split_once('=')) {
            if let Some((prefix, alias)) = key.split_once('.') {
                if prefix.eq_ignore_ascii_case("alias") && alias.eq_ignore_ascii_case(name) {
                    found = Some(value.to_owned());
                }
            }
        }
    }
    found
}

fn strip_inline_alias(global: &[String], name: &str) -> Vec<String> {
    let matches_alias = |pair: &str| {
        pair.split_once('=').is_some_and(|(key, _)| {
            key.split_once('.').is_some_and(|(prefix, alias)| {
                prefix.eq_ignore_ascii_case("alias") && alias.eq_ignore_ascii_case(name)
            })
        })
    };
    let mut out = Vec::with_capacity(global.len());
    let mut at = 0;
    while let Some(word) = global.get(at) {
        if word == "-c" {
            if let Some(pair) = global.get(at + 1) {
                if matches_alias(pair) {
                    at += 2;
                    continue;
                }
            }
        } else if let Some(pair) = word.strip_prefix("-c").filter(|rest| !rest.is_empty()) {
            if matches_alias(pair) {
                at += 1;
                continue;
            }
        }
        out.push(word.clone());
        at += 1;
    }
    out
}

fn substitute_words(words: &[String], args: &[String]) -> (Vec<String>, usize) {
    let mut used = 0;
    let expanded = words
        .iter()
        .map(|word| replace_placeholders(word, args, &mut used, false))
        .collect();
    (expanded, used)
}

fn substitute_shell(script: &str, args: &[String], append_unused: bool) -> String {
    let mut used = 0;
    let mut out = replace_placeholders(script, args, &mut used, true);
    if append_unused && used == 0 && !args.is_empty() {
        for arg in args {
            out.push(' ');
            out.push_str(&shell_quote(arg));
        }
    }
    out
}

fn replace_placeholders(text: &str, args: &[String], used: &mut usize, quote: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            out.push(ch);
            continue;
        }
        let braced = chars.peek() == Some(&'{');
        if braced {
            chars.next();
        }
        match chars.peek().copied() {
            Some(d) if d.is_ascii_digit() && d != '0' => {
                chars.next();
                if braced && chars.peek() != Some(&'}') {
                    out.push('$');
                    out.push('{');
                    out.push(d);
                    continue;
                }
                if braced {
                    chars.next();
                }
                let idx = (d as usize) - ('1' as usize);
                *used = (*used).max(idx + 1);
                if let Some(arg) = args.get(idx) {
                    if quote {
                        out.push_str(&shell_quote(arg));
                    } else {
                        out.push_str(arg);
                    }
                }
            }
            Some('*' | '@') => {
                let sym = chars.next().unwrap_or('@');
                if braced && chars.peek() != Some(&'}') {
                    out.push('$');
                    out.push('{');
                    out.push(sym);
                    continue;
                }
                if braced {
                    chars.next();
                }
                *used = args.len();
                let joined: Vec<String> = args
                    .iter()
                    .map(|arg| if quote { shell_quote(arg) } else { arg.clone() })
                    .collect();
                out.push_str(&joined.join(" "));
            }
            _ => {
                out.push('$');
                if braced {
                    out.push('{');
                }
            }
        }
    }
    out
}

fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '='))
    {
        return word.to_owned();
    }
    format!("'{}'", word.replace('\'', "'\\''"))
}

fn split_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current: Option<String> = None;
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                let buf = current.get_or_insert_with(String::new);
                for next in chars.by_ref() {
                    if next == '\'' {
                        break;
                    }
                    buf.push(next);
                }
            }
            '"' => {
                let buf = current.get_or_insert_with(String::new);
                while let Some(next) = chars.next() {
                    match next {
                        '"' => break,
                        '\\' => {
                            if let Some(escaped) = chars.next() {
                                buf.push(escaped);
                            }
                        }
                        other => buf.push(other),
                    }
                }
            }
            '\\' => {
                if let Some(escaped) = chars.next() {
                    current.get_or_insert_with(String::new).push(escaped);
                }
            }
            c if c.is_whitespace() => {
                if let Some(word) = current.take() {
                    words.push(word);
                }
            }
            other => current.get_or_insert_with(String::new).push(other),
        }
    }
    if let Some(word) = current {
        words.push(word);
    }
    words
}
