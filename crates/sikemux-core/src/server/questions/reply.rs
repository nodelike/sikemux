//! Reading an agent's reply for what it asks the person: CLIs do not say they asked, so the core
//! reads the language of it.

/// How a question opens when it offers more work, so a yes or a no answers it, or an
/// offer that leaves the person to say opens, whatever it ends in.
pub(super) const OFFERS: [&str; 10] = [
    "shall i ",
    "should i ",
    "want me to ",
    "do you want me to ",
    "would you like me to ",
    "can i ",
    "may i ",
    "shall we ",
    "if you want, i can ",
    "if you'd like, i can ",
];

/// What an agent's reply asks the person, in its own words, one ask for each paragraph or list
/// item that asks, in order, each with whether a yes or a no answers it.
/// A paragraph or list item counts when its last sentence ends in a question mark, so one
/// answered in the same paragraph does not, or offers more as "If you want, I can…" does.
/// A last sentence that means nothing alone, such as "Yes or no?", stands for the latest real
/// question before it, anywhere in the reply.
pub(super) fn asked_in(reply: &str) -> Vec<(String, bool)> {
    let mut found: Vec<(String, bool)> = Vec::new();
    let mut earlier: Vec<String> = Vec::new();
    for paragraph in paragraphs(reply) {
        let mut sentences = sentences_of(&paragraph);
        sentences.pop();
        earlier.extend(sentences.into_iter().filter(|sentence| {
            sentence.trim_end_matches([')', '"']).ends_with('?') && bare_ask(sentence).is_none()
        }));
        let Some((sentence, polar)) = last_ask(&paragraph) else {
            continue;
        };
        let Some(takes_yes_or_no) = bare_ask(&sentence) else {
            earlier.push(sentence.clone());
            found.push((sentence, polar));
            continue;
        };
        let meant =
            (earlier.iter()).rfind(|question| !takes_yes_or_no || answers_yes_or_no(question));
        match meant {
            Some(question) => {
                found.retain(|(asked, _)| asked != question);
                let polar = takes_yes_or_no || answers_yes_or_no(question);
                found.push((question.clone(), polar));
            }
            None => found.push((sentence, takes_yes_or_no)),
        }
    }
    found
}

/// The reply's paragraphs and list items, as one line each. Code, quotes, headings and table
/// rows say nothing to the person.
pub(super) fn paragraphs(reply: &str) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut block: Vec<&str> = Vec::new();
    let mut fenced = false;
    for line in reply.lines().chain(std::iter::once("")) {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            paragraphs.push(block.join(" "));
            block.clear();
            continue;
        }
        if fenced {
            continue;
        }
        let item = list_item(line);
        let apart = line.is_empty()
            || line.starts_with('#')
            || line.starts_with('>')
            || line.starts_with('|');
        if apart || item.is_some() {
            paragraphs.push(block.join(" "));
            block.clear();
        }
        if let Some(item) = item {
            block.push(item);
        } else if !apart {
            block.push(line);
        }
    }
    paragraphs.retain(|paragraph| !paragraph.trim().is_empty());
    paragraphs
}

/// A list item's text without its marker.
pub(super) fn list_item(line: &str) -> Option<&str> {
    let marked = ["- ", "* ", "+ "]
        .iter()
        .find_map(|marker| line.strip_prefix(marker));
    marked.or_else(|| {
        let digits = line
            .find(|c: char| !c.is_ascii_digit())
            .filter(|at| *at > 0)?;
        line[digits..]
            .strip_prefix(". ")
            .or_else(|| line[digits..].strip_prefix(") "))
    })
}

/// How a question opens that a yes or a no cannot answer.
pub(super) const OPEN_ASKS: [&str; 9] = [
    "what ", "which ", "who ", "whom ", "whose ", "where ", "when ", "why ", "how ",
];

/// The paragraph's last sentence, without its emphasis, when it asks the person something or
/// offers more, and whether a yes or a no answers it, read after a lead-in such as "Next
/// step:".
pub(super) fn last_ask(paragraph: &str) -> Option<(String, bool)> {
    let paragraph = paragraph.trim();
    let (ends, asks) = sentence_ends(paragraph);
    let start = ends.last().copied().unwrap_or(0);
    let sentence = plain(paragraph[start..].trim());
    let lower = sentence.to_lowercase();
    let asked = after_lead_in(&lower);
    let leaves = asked.starts_with("if you")
        && !asked.contains(" or ")
        && OFFERS.iter().any(|opening| asked.starts_with(opening));
    (asks || leaves).then(|| {
        let polar = answers_yes_or_no(&sentence);
        (sentence, polar)
    })
}

/// The paragraph's sentences, without their emphasis.
pub(super) fn sentences_of(paragraph: &str) -> Vec<String> {
    let paragraph = paragraph.trim();
    let (ends, _) = sentence_ends(paragraph);
    let starts = std::iter::once(0).chain(ends.iter().copied());
    let stops = ends.iter().copied().chain(std::iter::once(paragraph.len()));
    starts
        .zip(stops)
        .map(|(start, stop)| plain(paragraph[start..stop].trim()))
        .collect()
}

/// Where the paragraph's sentences end, and whether its last ends in a question mark. No
/// sentence ends inside backticks or emphasis, and inside backticks nothing asks.
pub(super) fn sentence_ends(paragraph: &str) -> (Vec<usize>, bool) {
    let (mut coded, mut strong, mut stressed) = (false, false, false);
    let mut ends: Vec<usize> = Vec::new();
    let mut ending = false;
    let mut asks = false;
    let chars: Vec<(usize, char)> = paragraph.char_indices().collect();
    let mut at = 0;
    while at < chars.len() {
        let (index, c) = chars[at];
        let before = at.checked_sub(1).map(|before| chars[before].1);
        let next = chars.get(at + 1).map(|(_, next)| *next);
        at += 1;
        if c == '`' {
            coded = !coded;
            ending = false;
            continue;
        }
        if coded {
            continue;
        }
        match c {
            '*' | '_' if next == Some(c) => {
                strong = !strong;
                at += 1;
                continue;
            }
            '*' => {
                stressed = !stressed;
                continue;
            }
            '_' if emphasis_underscore(before, next) => {
                stressed = !stressed;
                continue;
            }
            _ => {}
        }
        if c.is_whitespace() && ending && !strong && !stressed {
            ends.push(index);
        }
        ending = matches!(c, '.' | '?' | '!') || (ending && matches!(c, ')' | '"'));
        if c == '?'
            && chars[at..]
                .iter()
                .all(|(_, rest)| matches!(rest, '*' | '_' | ')'))
        {
            asks = true;
        }
    }
    (ends, asks)
}

pub(super) fn after_lead_in(lower: &str) -> &str {
    lower.rsplit(": ").next().unwrap_or(lower)
}

pub(super) fn answers_yes_or_no(sentence: &str) -> bool {
    let lower = sentence.to_lowercase();
    let asked = after_lead_in(&lower);
    !asked.contains(" or ") && !OPEN_ASKS.iter().any(|opening| asked.starts_with(opening))
}

/// For a question that means nothing without what came before it, whether it takes a yes or a
/// no; none for any other sentence.
pub(super) fn bare_ask(sentence: &str) -> Option<bool> {
    let lower = sentence.to_lowercase();
    match after_lead_in(&lower).trim_end_matches(['?', '!', '.']) {
        "yes or no" | "yes/no" | "y/n" | "ok" | "okay" | "right" | "agreed" | "sound good"
        | "sounds good" | "good to go" | "go ahead" => Some(true),
        "thoughts" | "your thoughts" | "your call" | "what do you think" | "what do you say" => {
            Some(false)
        }
        _ => None,
    }
}

/// Whether an underscore marks emphasis rather than joining a word, as in `snake_case`.
pub(super) fn emphasis_underscore(before: Option<char>, next: Option<char>) -> bool {
    !before.is_some_and(char::is_alphanumeric) || !next.is_some_and(char::is_alphanumeric)
}

/// Markdown text as the person reads it: without the backticks, asterisks and emphasis
/// underscores that mark it up.
pub(super) fn plain(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let kept = chars.iter().enumerate().filter(|&(at, &c)| match c {
        '`' | '*' => false,
        '_' => !emphasis_underscore(
            at.checked_sub(1).map(|before| chars[before]),
            chars.get(at + 1).copied(),
        ),
        _ => true,
    });
    let text: String = kept.map(|(_, c)| *c).collect();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
