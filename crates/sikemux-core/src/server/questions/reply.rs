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
        let Some(ask) = last_ask(&paragraph) else {
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
                let polar =
                    !is_choice(question) && (takes_yes_or_no || answers_yes_or_no(question));
                found.push(Ask {
                    text: question.clone(),
                    polar,
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
                });
            }
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
pub(super) fn last_ask(paragraph: &str) -> Option<Ask> {
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
    let polar = !is_choice(&sentence) && answers_yes_or_no(&sentence);
    let text = std::iter::once(sentence).chain(tail).collect::<Vec<_>>();
    Some(Ask {
        text: text.join(" "),
        polar,
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

/// Whether a sentence asks the person to choose: it tells them to, or asks which, or asks one
/// thing or another.
fn is_choice(sentence: &str) -> bool {
    let lower = sentence.to_lowercase();
    let asked = after_lead_in(&lower);
    let question = sentence.trim_end_matches([')', '"']).ends_with('?');
    let either = asked.contains(" or ") && !asked.starts_with("if you");
    chooses(sentence) || (question && (asked.starts_with("which ") || either))
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
