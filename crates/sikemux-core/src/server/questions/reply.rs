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

/// Words that open a yes-or-no question without saying what would be done.
const NOT_WORK: [&str; 24] = [
    "are", "was", "were", "does", "did", "has", "have", "had", "will", "would", "could", "can",
    "should", "shall", "may", "might", "must", "the", "this", "that", "you", "your", "also",
    "just",
];

/// The verb of the work a yes-or-no question offers, such as `commit` in "Want me to commit it?".
pub(super) fn offered_work(sentence: &str) -> Option<String> {
    if sentences_of(sentence)
        .last()
        .is_some_and(|last| bare_ask(last).is_some())
    {
        return None;
    }
    let lower = sentence.to_lowercase();
    let asked = after_lead_in(&lower);
    let work = (OFFERS.iter())
        .find_map(|opening| asked.strip_prefix(opening))
        .unwrap_or(asked);
    let verb = work.split_whitespace().next()?;
    let verb = verb.trim_matches(|c: char| !c.is_alphabetic());
    (verb.len() >= 3 && !NOT_WORK.contains(&verb)).then(|| verb.to_owned())
}

/// What one paragraph or list item of a reply asks the person.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Ask {
    pub text: String,
    /// A yes or a no answers it.
    pub polar: bool,
    /// It asks the person to choose.
    pub choice: Option<Choice>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Choice {
    /// The options the reply names, none when it names fewer than two.
    pub options: Vec<String>,
    /// The option the agent says it would take.
    pub pick: Option<String>,
}

/// What an agent's reply asks the person, in its own words, one ask for each paragraph or list
/// item that asks, in order.
/// A paragraph or list item counts when its last sentence ends in a question mark, so one
/// answered in the same paragraph does not, offers more as "If you want, I can…" does, or asks
/// the person to choose as "Pick one" does. Sentences after it that only say what the agent
/// prefers or that it waits for the pick, such as "I'm leaning A.", show with it.
/// A last sentence that means nothing alone, such as "Yes or no?", stands for the latest real
/// question before it, anywhere in the reply, or with no such question, comes with the rest of
/// its paragraph, or the paragraph before when it stands alone, which is what it asks about.
/// A yes or a no does not answer a paragraph that also asks something open, such as "Who owns it?".
pub(super) fn asked_in(reply: &str) -> Vec<Ask> {
    let named = options_named_in(reply);
    let mut found: Vec<Ask> = Vec::new();
    let mut earlier: Vec<String> = Vec::new();
    let mut said: Option<String> = None;
    for paragraph in paragraphs(reply) {
        let mut sentences = sentences_of(&paragraph);
        let whole = sentences.join(" ");
        sentences.pop();
        let before = if sentences.is_empty() {
            said.take()
        } else {
            Some(sentences.join(" "))
        };
        said = Some(whole);
        let known = earlier.len();
        earlier.extend(sentences.into_iter().filter(|sentence| {
            sentence.trim_end_matches([')', '"']).ends_with('?') && bare_ask(sentence).is_none()
        }));
        let asked_here = earlier.len() > known;
        let Some(ask) = last_ask(&paragraph, &named) else {
            continue;
        };
        let Some(takes_yes_or_no) = bare_ask(&ask.text) else {
            earlier.push(ask.text.clone());
            found.push(ask);
            continue;
        };
        let meant =
            (earlier.iter()).rfind(|question| !takes_yes_or_no || answers_yes_or_no(question));
        match meant {
            Some(question) => {
                found.retain(|asked| &asked.text != question);
                let choice = choice_of(question, &[], &named);
                let polar = choice.is_none() && (takes_yes_or_no || answers_yes_or_no(question));
                found.push(Ask {
                    text: question.clone(),
                    polar,
                    choice,
                });
            }
            None => {
                let shown = before.map_or_else(
                    || ask.text.clone(),
                    |before| format!("{before} {}", ask.text),
                );
                found.push(Ask {
                    text: shown,
                    polar: takes_yes_or_no && !asked_here,
                    choice: None,
                });
            }
        }
    }
    found
}

/// Words that point back at something a question does not name itself.
const POINTING: [&str; 5] = ["it", "them", "they", "these", "those"];

/// Whether a question leans on what came before it, so it says nothing without the chat: it
/// points back with a word such as "them", or ends on "this" or "that", and names nothing
/// itself, such as a file, a command or a name.
pub(super) fn leans(sentence: &str) -> bool {
    let words: Vec<&str> = (sentence.split_whitespace())
        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .collect();
    let lower: Vec<String> = words.iter().map(|word| word.to_lowercase()).collect();
    let points = lower.iter().any(|word| POINTING.contains(&word.as_str()))
        || lower
            .last()
            .is_some_and(|last| last == "this" || last == "that");
    let names = words.iter().enumerate().any(|(at, word)| {
        word.contains(['.', '/', '_', '#', '@', '-'])
            || word.chars().any(|c| c.is_ascii_digit())
            || (at > 0 && *word != "I" && word.starts_with(char::is_uppercase))
    });
    points && !names
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

/// The paragraph's last sentence, without its emphasis, when it asks the person something, offers
/// more or asks them to choose, read after a lead-in such as "Next step:". Only remarks on the
/// choice may follow it, which show with it.
pub(super) fn last_ask(paragraph: &str, named: &[String]) -> Option<Ask> {
    let paragraph = paragraph.trim();
    let mut sentences = sentences_of(paragraph);
    let remarks = (sentences.iter().rev())
        .take_while(|sentence| remarks_on_choosing(sentence))
        .count();
    let tail = sentences.split_off(sentences.len() - remarks);
    let (sentence, asks) = if tail.is_empty() {
        let (ends, asks) = sentence_ends(paragraph);
        let start = ends.last().copied().unwrap_or(0);
        (plain(paragraph[start..].trim()), asks)
    } else {
        let sentence = sentences.pop()?;
        let asks = sentence.trim_end_matches([')', '"']).ends_with('?');
        (sentence, asks)
    };
    let lower = sentence.to_lowercase();
    let asked = after_lead_in(&lower);
    let leaves = asked.starts_with("if you")
        && !asked.contains(" or ")
        && OFFERS.iter().any(|opening| asked.starts_with(opening));
    if !asks && !leaves && !chooses(&sentence) {
        return None;
    }
    let choice = choice_of(&sentence, &tail, named);
    let polar = choice.is_none() && answers_yes_or_no(&sentence);
    let text = std::iter::once(sentence).chain(tail).collect::<Vec<_>>();
    Some(Ask {
        text: text.join(" "),
        polar,
        choice,
    })
}

/// How the agent says which option it would take, such as "I'm leaning A".
const PREFERRING: [&str; 16] = [
    "i'm leaning toward ",
    "i'm leaning towards ",
    "i'm leaning ",
    "i am leaning ",
    "leaning ",
    "i lean ",
    "i'd lean ",
    "i'd go with ",
    "i would go with ",
    "i'd pick ",
    "i would pick ",
    "my pick is ",
    "my vote is ",
    "i'd choose ",
    "i'd recommend ",
    "i recommend ",
];

/// Words of a remark about the person's pick, such as "I won't start until you pick".
const ON_CHOOSING: [&str; 9] = [
    "until you ",
    "your pick",
    "my pick",
    "your call",
    "open call",
    "your choice",
    "you pick",
    "you choose",
    "you decide",
];

/// Whether a sentence only remarks on a choice: what the agent prefers, or that it waits for the
/// person's pick.
fn remarks_on_choosing(sentence: &str) -> bool {
    let lower = sentence.to_lowercase();
    !lower.trim_end_matches([')', '"']).ends_with('?')
        && !chooses(sentence)
        && (preference_of(&lower).is_some()
            || ON_CHOOSING.iter().any(|words| lower.contains(words)))
}

/// What a remark says the agent prefers, in lower case.
fn preference_of(lower: &str) -> Option<&str> {
    let asked = after_lead_in(lower);
    let preferred = PREFERRING
        .iter()
        .find_map(|opening| asked.strip_prefix(opening))?;
    let end = preferred
        .find([',', '.', ';', '!', '(', '—'])
        .unwrap_or(preferred.len());
    let preferred = preferred[..end].trim();
    let preferred = [" because", " since", " for ", " so "]
        .iter()
        .filter_map(|word| preferred.find(word))
        .min()
        .map_or(preferred, |at| &preferred[..at]);
    Some(preferred.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace()))
        .filter(|preferred| !preferred.is_empty())
}

/// How an imperative opens that leaves the person to choose, such as "Tell me which".
const CHOOSING: [&str; 8] = [
    "tell me which",
    "tell me whether",
    "let me know which",
    "let me know whether",
    "choose ",
    "decide between ",
    "decide which",
    "decide whether",
];

/// Whether a sentence tells the person to choose, as "Pick one" or "Say A or B" does, unlike an
/// instruction such as "Run make dev to get your build back".
fn chooses(sentence: &str) -> bool {
    let lower = sentence.to_lowercase();
    let asked = after_lead_in(&lower);
    if ["your call: ", "your pick: ", "your choice: "]
        .iter()
        .any(|lead_in| lower.starts_with(lead_in))
    {
        return asked.contains(" or ");
    }
    let picks = ["pick one", "pick which", "pick between", "pick either"]
        .iter()
        .any(|opening| asked.starts_with(opening));
    let names_two = ["pick ", "say ", "go with "]
        .iter()
        .any(|opening| asked.starts_with(opening))
        && asked.contains(" or ");
    CHOOSING.iter().any(|opening| asked.starts_with(opening)) || picks || names_two
}

/// What a sentence asks the person to choose between, with the remarks after it, when it asks
/// them to choose: it tells them to, or asks which, or asks one thing or another.
fn choice_of(sentence: &str, remarks: &[String], named: &[String]) -> Option<Choice> {
    let lower = sentence.to_lowercase();
    let asked = after_lead_in(&lower);
    let question = sentence.trim_end_matches([')', '"']).ends_with('?');
    let either = asked.contains(" or ") && !asked.starts_with("if you");
    if !chooses(sentence) && !(question && (asked.starts_with("which ") || either)) {
        return None;
    }
    let (mut options, mut pick) = quoted(sentence);
    if options.len() < 2 {
        (options, pick) = either_or(sentence);
    }
    if options.len() < 2 {
        options = named.to_vec();
    }
    if options.len() < 2 {
        options.clear();
    }
    options.truncate(super::MAX_CHOICES);
    let preferred = (remarks.iter()).find_map(|remark| {
        let lower = remark.to_lowercase();
        let preferred = preference_of(&lower)?;
        (options.iter())
            .find(|option| refers_to(preferred, option))
            .cloned()
    });
    pick = pick.filter(|pick| options.contains(pick)).or(preferred);
    Some(Choice { options, pick })
}

/// Whether what the agent prefers, in lower case, names the option, such as "a" for "Option A".
fn refers_to(preferred: &str, option: &str) -> bool {
    let option = option.to_lowercase();
    option == preferred
        || option.ends_with(&format!(" {preferred}"))
        || preferred.ends_with(&format!(" {option}"))
}

/// How the agent marks the option it would take, right after it.
const PICK_MARKS: [&str; 4] = ["(my pick)", "(recommended)", "(my vote)", "(preferred)"];

/// The options a sentence puts in quotes, and the one it marks as its pick.
fn quoted(sentence: &str) -> (Vec<String>, Option<String>) {
    let mut options = Vec::new();
    let mut pick = None;
    let mut rest = sentence;
    while let Some(open) = rest.find(['"', '“']) {
        let after = &rest[open..];
        let quote = after.chars().next().map_or(1, char::len_utf8);
        let after = &after[quote..];
        let Some(close) = after.find(['"', '”']) else {
            break;
        };
        let option = after[..close].trim();
        rest = &after[close..];
        rest = &rest[rest.chars().next().map_or(1, char::len_utf8)..];
        if option.is_empty() {
            continue;
        }
        if marks_pick(rest) {
            pick = Some(option.to_owned());
        }
        options.push(option.to_owned());
    }
    (options, pick)
}

fn marks_pick(after: &str) -> bool {
    let after = after.trim_start().to_lowercase();
    PICK_MARKS.iter().any(|mark| after.starts_with(mark))
}

/// The options of a sentence that names them one or the other, as in "Want me to cache it, or
/// drop it?", and the one it marks as its pick. None when what it asks opens with a question
/// word or points elsewhere, as "Pick one" does.
fn either_or(sentence: &str) -> (Vec<String>, Option<String>) {
    let body = sentence.rsplit(": ").next().unwrap_or(sentence);
    let openings = OFFERS.iter().copied().chain([
        "choose between ",
        "decide between ",
        "choose ",
        "pick ",
        "say ",
        "go with ",
    ]);
    let body = (openings.into_iter())
        .find_map(|opening| strip_prefix_ignoring_case(body, opening))
        .unwrap_or(body);
    let lower = body.to_ascii_lowercase();
    let first = (lower.split_whitespace().next())
        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric()));
    let points_elsewhere = ["one", "which", "either", "between"]
        .iter()
        .any(|word| first == Some(word));
    if points_elsewhere || OPEN_ASKS.iter().any(|opening| lower.starts_with(opening)) {
        return (Vec::new(), None);
    }
    let end = [
        ", and i'll",
        " and i'll",
        " and i will",
        ", so i ",
        " so i ",
        ", then i'll",
    ]
    .iter()
    .filter_map(|words| lower.find(words))
    .min()
    .unwrap_or(body.len());
    let body = body[..end].trim_end_matches(['?', '.', '!', '…', ' ']);
    let mut pick = None;
    let options: Vec<String> = (body.split(" or "))
        .map(|option| {
            let option = option.trim().trim_end_matches(',').trim();
            let marked = PICK_MARKS
                .iter()
                .find_map(|mark| strip_suffix_ignoring_case(option, mark));
            let option = capitalised(marked.unwrap_or(option).trim());
            if marked.is_some() {
                pick = Some(option.clone());
            }
            option
        })
        .collect();
    if options.iter().any(String::is_empty) {
        return (Vec::new(), None);
    }
    (options, pick)
}

/// Words that name an option on a line of its own, such as "Option A" or "Mockup B".
const OPTION_WORDS: [&str; 6] = [
    "option ", "mockup ", "layout ", "plan ", "variant ", "design ",
];

/// The options a reply lays out as lines of their own, such as "**A** — one board" and
/// "**B** — two columns", in order; none when it lays out fewer than two.
fn options_named_in(reply: &str) -> Vec<String> {
    let mut named: Vec<String> = Vec::new();
    let mut fenced = false;
    for line in reply.lines() {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
        }
        if fenced {
            continue;
        }
        let line = line.trim_start_matches('#').trim_start();
        let line = plain(list_item(line).unwrap_or(line));
        if let Some(label) = option_label(&line).filter(|label| !named.contains(label)) {
            named.push(label);
        }
    }
    if named.len() < 2 {
        named.clear();
    }
    named
}

/// The label a line opens with when it lays out an option: a capital from A to H, or a word such
/// as "Option" with a letter or a number, then a colon, a dash, a stop or a bracket.
fn option_label(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let worded =
        (OPTION_WORDS.iter()).find_map(|word| lower.strip_prefix(word).map(|_| word.len()));
    let start = worded.unwrap_or(0);
    let mark = line.get(start..)?;
    let length = mark
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(mark.len());
    let token = &mark[..length];
    let letter = token.len() == 1 && ('A'..='H').contains(&token.chars().next()?);
    let numbered =
        worded.is_some() && !token.is_empty() && token.chars().all(|c| c.is_ascii_digit());
    if !letter && !numbered {
        return None;
    }
    let after = &mark[length..];
    let separated = [":", " —", " –", " - ", ")", ". "]
        .iter()
        .any(|separator| after.starts_with(separator))
        || after.is_empty() && worded.is_some();
    separated.then(|| line[..start + length].to_owned())
}

fn strip_prefix_ignoring_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

fn strip_suffix_ignoring_case<'a>(text: &'a str, suffix: &str) -> Option<&'a str> {
    let at = text.len().checked_sub(suffix.len())?;
    let tail = text.get(at..)?;
    tail.eq_ignore_ascii_case(suffix).then(|| &text[..at])
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
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
        "yes or no"
        | "yes/no"
        | "y/n"
        | "ok"
        | "okay"
        | "right"
        | "agreed"
        | "sound good"
        | "sounds good"
        | "good to go"
        | "go ahead"
        | "is that ok"
        | "is that okay"
        | "is this ok"
        | "is this okay"
        | "that ok"
        | "that okay"
        | "ok with that"
        | "okay with that"
        | "are you ok with that"
        | "are you okay with that"
        | "does that work"
        | "does this work"
        | "does that work for you"
        | "work for you"
        | "works for you"
        | "does that sound good"
        | "does that sound right"
        | "does that make sense"
        | "good with that"
        | "happy with that" => Some(true),
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
