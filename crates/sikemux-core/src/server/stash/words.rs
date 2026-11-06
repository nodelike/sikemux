//! Tidying what is stashed from a page into words for a prompt.

/// Where a link was taken out of a text, while what it leaves around it is tidied.
pub(super) const CUT: char = '\u{1}';

/// `text` with the links in `cut` taken out, and what they leave dangling with them: an `and`,
/// `or` or comma before a link that ended the sentence, brackets around only links, doubled
/// spaces and a space before punctuation.
pub(super) fn words(text: &str, cut: &mut [String]) -> String {
    cut.sort_by_key(|url| std::cmp::Reverse(url.len()));
    let mut marked = text.to_owned();
    for url in cut.iter() {
        marked = marked.replace(url.as_str(), &CUT.to_string());
    }
    let lines: Vec<String> = marked.lines().map(tidied).collect();
    let mut words = String::new();
    let mut blank = false;
    for line in lines {
        if line.is_empty() {
            blank = !words.is_empty();
            continue;
        }
        if !words.is_empty() {
            words.push_str(if blank { "\n\n" } else { "\n" });
        }
        words.push_str(&line);
        blank = false;
    }
    words
}

pub(super) enum Token<'a> {
    Word(&'a str),
    /// A link taken out, with the punctuation written before and after it.
    Cut {
        open: &'a str,
        close: &'a str,
    },
}

pub(super) fn tidied(line: &str) -> String {
    let tokens: Vec<Token> = line
        .split_whitespace()
        .flat_map(|word| {
            let Some(start) = word.find(CUT) else {
                return vec![Token::Word(word)];
            };
            let end = word.rfind(CUT).map_or(start, |end| end + CUT.len_utf8());
            let (before, close) = (&word[..start], &word[end..]);
            match before.chars().any(char::is_alphanumeric) {
                true => vec![Token::Word(before), Token::Cut { open: "", close }],
                false => vec![Token::Cut {
                    open: before,
                    close,
                }],
            }
        })
        .collect();
    let joins = |word: &str| matches!(word, "and" | "or" | "&" | ",");
    let mut kept: Vec<String> = Vec::new();
    let mut at = 0;
    while at < tokens.len() {
        let Token::Cut { open, .. } = tokens[at] else {
            if let Token::Word(word) = tokens[at] {
                kept.push(word.to_owned());
            }
            at += 1;
            continue;
        };
        let mut last = at;
        let mut next = at + 1;
        while next < tokens.len() {
            match tokens[next] {
                Token::Cut { .. } => last = next,
                Token::Word(word)
                    if joins(word) && matches!(tokens.get(next + 1), Some(Token::Cut { .. })) => {}
                Token::Word(_) => break,
            }
            next += 1;
        }
        let close = match tokens[last] {
            Token::Cut { close, .. } => close,
            Token::Word(_) => "",
        };
        let ends = next == tokens.len() || close.contains(['.', '!', '?', ';', ':']);
        let closing = |char: char| !open.is_empty() && matches!(char, ')' | ']' | '}');
        let mut after: String = close.chars().filter(|char| !closing(*char)).collect();
        if ends {
            after.retain(|char| char != ',');
            if kept.last().is_some_and(|word| joins(word)) {
                kept.pop();
            }
            if let Some(word) = kept.last_mut() {
                if word.ends_with(',') {
                    word.pop();
                }
            }
        }
        match kept.last_mut() {
            Some(word) => word.push_str(&after),
            None if !after.chars().all(|char| char.is_ascii_punctuation()) => kept.push(after),
            None => {}
        }
        at = last + 1;
    }
    kept.join(" ")
}
