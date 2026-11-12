//! Finds the sentences of an answer that rest on nothing: in a paragraph or list item holding a
//! link to a source, each sentence that carries none. It fails towards keeping text, so a
//! sentence is split off only where its end is certain, and only plain words are ever cut.

use std::ops::Range;

use pulldown_cmark::{Event, Options as ParserOptions, Parser, Tag, TagEnd};

/// Words that end in a full stop without ending a sentence.
const SHORTENED: [&str; 16] = [
    "e.g", "i.e", "vs", "etc", "no", "nos", "mr", "mrs", "ms", "dr", "st", "cf", "al", "approx",
    "fig", "vol",
];

#[derive(Clone, Copy, PartialEq)]
enum Piece {
    /// Words as written, nothing in them read as markup.
    Words,
    Source {
        found: bool,
    },
    /// Emphasis, code, another link, or words whose markup changed them.
    Marked,
}

/// What to leave out of `text`, each a byte range and how many sentences it holds: in each
/// paragraph or list item that holds a source `sources` found (it answers `None` for a link that is
/// no source, and whether what a source names was found), every sentence of plain words with no
/// such source, along with the space that parts it from its neighbour. A sentence that asks
/// anything, leads in with `:`, or may run on from or into the text around `text`, is kept.
/// Quotes, tables, headings and footnotes are left as written.
pub fn unsourced(text: &str, sources: impl Fn(&str) -> Option<bool>) -> Vec<(Range<usize>, usize)> {
    let flags = ParserOptions::ENABLE_TABLES
        | ParserOptions::ENABLE_STRIKETHROUGH
        | ParserOptions::ENABLE_TASKLISTS
        | ParserOptions::ENABLE_FOOTNOTES;
    let mut cuts = Vec::new();
    let mut aside = 0usize;
    let mut marked = 0usize;
    let mut run: Vec<(Range<usize>, Piece)> = Vec::new();
    let mut link: Option<(usize, Piece)> = None;
    for (event, range) in Parser::new_ext(text, flags).into_offset_iter() {
        let inline = match &event {
            Event::Start(Tag::BlockQuote(_) | Tag::Heading { .. } | Tag::Table(_))
            | Event::Start(Tag::FootnoteDefinition(_)) => {
                cut_run(text, &mut run, &mut cuts);
                aside += 1;
                false
            }
            Event::End(TagEnd::BlockQuote(_) | TagEnd::Heading(_) | TagEnd::Table)
            | Event::End(TagEnd::FootnoteDefinition) => {
                run.clear();
                aside = aside.saturating_sub(1);
                false
            }
            Event::Start(Tag::Link { dest_url, .. }) if marked == 0 => {
                let piece = match sources(dest_url) {
                    Some(found) => Piece::Source { found },
                    None => Piece::Marked,
                };
                link = Some((range.start, piece));
                marked += 1;
                true
            }
            Event::End(TagEnd::Link) if marked == 1 => {
                marked = 0;
                if let Some((start, piece)) = link.take() {
                    run.push((start..range.end, piece));
                }
                true
            }
            Event::Start(
                Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Link { .. }
                | Tag::Image { .. }
                | Tag::Superscript
                | Tag::Subscript,
            ) => {
                if marked == 0 {
                    run.push((range.clone(), Piece::Marked));
                }
                marked += 1;
                true
            }
            Event::End(
                TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Link
                | TagEnd::Image
                | TagEnd::Superscript
                | TagEnd::Subscript,
            ) => {
                marked = marked.saturating_sub(1);
                true
            }
            _ if marked > 0 => true,
            Event::Text(words) => {
                let piece = match text.get(range.clone()) == Some(words.as_ref()) {
                    true => Piece::Words,
                    false => Piece::Marked,
                };
                run.push((range.clone(), piece));
                true
            }
            Event::SoftBreak => {
                run.push((range.clone(), Piece::Words));
                true
            }
            Event::Code(_)
            | Event::InlineHtml(_)
            | Event::InlineMath(_)
            | Event::HardBreak
            | Event::FootnoteReference(_)
            | Event::TaskListMarker(_) => {
                run.push((range.clone(), Piece::Marked));
                true
            }
            _ => false,
        };
        if !inline {
            if aside == 0 {
                cut_run(text, &mut run, &mut cuts);
            } else {
                run.clear();
            }
        }
    }
    if aside == 0 {
        cut_run(text, &mut run, &mut cuts);
    }
    cuts
}

/// One piece of a run as the scan reads it: a character of plain words, or a whole piece that is
/// not.
#[derive(Clone, Copy)]
enum Token {
    Char {
        at: usize,
        c: char,
    },
    Whole {
        start: usize,
        end: usize,
        piece: Piece,
    },
}

impl Token {
    fn start(self) -> usize {
        match self {
            Token::Char { at, .. } => at,
            Token::Whole { start, .. } => start,
        }
    }

    fn end(self) -> usize {
        match self {
            Token::Char { at, c } => at + c.len_utf8(),
            Token::Whole { end, .. } => end,
        }
    }

    fn space(self) -> bool {
        matches!(self, Token::Char { c, .. } if c.is_whitespace())
    }

    fn source(self) -> Option<bool> {
        match self {
            Token::Whole {
                piece: Piece::Source { found },
                ..
            } => Some(found),
            _ => None,
        }
    }
}

/// One sentence of a run: the bytes from its first word to its end, sources placed after it
/// included, and what it holds.
struct Sentence {
    range: Range<usize>,
    found: bool,
    marked: bool,
    asks: bool,
    /// It ends in `.`, `!` or `?`, rather than where the run does.
    ended: bool,
}

impl Sentence {
    fn new(range: Range<usize>) -> Self {
        Self {
            range,
            found: false,
            marked: false,
            asks: false,
            ended: false,
        }
    }
}

/// Adds the cuts of one paragraph or list item's `run` to `cuts`, and empties it.
fn cut_run(
    text: &str,
    run: &mut Vec<(Range<usize>, Piece)>,
    cuts: &mut Vec<(Range<usize>, usize)>,
) {
    let pieces = std::mem::take(run);
    let sourced = (pieces.iter()).any(|(_, piece)| *piece == Piece::Source { found: true });
    if !sourced {
        return;
    }
    let sentences = sentences_of(&tokens(text, &pieces));
    let words = |sentence: &Sentence| text.get(sentence.range.clone()).unwrap_or_default();
    let first = sentences.first().map(|sentence| sentence.range.start);
    let last = sentences.last().map(|sentence| sentence.range.end);
    let kept = |sentence: &Sentence| {
        let begins_text = first == Some(sentence.range.start)
            && text
                .get(..sentence.range.start)
                .unwrap_or_default()
                .trim()
                .is_empty();
        let ends_text = last == Some(sentence.range.end)
            && text
                .get(sentence.range.end..)
                .unwrap_or_default()
                .trim()
                .is_empty();
        sentence.found
            || sentence.marked
            || sentence.asks
            || words(sentence).trim_end().ends_with(':')
            || (begins_text && words(sentence).starts_with(char::is_lowercase))
            || (ends_text && !sentence.ended)
    };
    let mut at = 0;
    while let Some(sentence) = sentences.get(at) {
        at += 1;
        if kept(sentence) {
            continue;
        }
        let before = (at.checked_sub(2)).and_then(|before| sentences.get(before));
        let mut end = sentence.range.end;
        let mut count = 1;
        while let Some(next) = sentences.get(at).filter(|next| !kept(next)) {
            end = next.range.end;
            count += 1;
            at += 1;
        }
        let cut = match (sentences.get(at), before) {
            (Some(next), _) => sentence.range.start..next.range.start,
            (None, Some(before)) => before.range.end..end,
            (None, None) => continue,
        };
        cuts.push((cut, count));
    }
}

fn tokens(text: &str, pieces: &[(Range<usize>, Piece)]) -> Vec<Token> {
    let mut tokens = Vec::new();
    for (range, piece) in pieces {
        match piece {
            Piece::Words => tokens.extend(
                (text.get(range.clone()).unwrap_or_default().char_indices()).map(|(offset, c)| {
                    Token::Char {
                        at: range.start + offset,
                        c,
                    }
                }),
            ),
            piece => tokens.push(Token::Whole {
                start: range.start,
                end: range.end,
                piece: *piece,
            }),
        }
    }
    tokens
}

/// The sentences of a run, split only where one surely ends: `.`, `!` or `?` outside brackets,
/// after a word that is no shortening, then space, and then a capital, a digit or the run's end.
/// Sources placed right after the end belong to the sentence before them.
fn sentences_of(tokens: &[Token]) -> Vec<Sentence> {
    let mut sentences = Vec::new();
    let mut open: Option<Sentence> = None;
    let mut depth = 0i32;
    let mut at = 0;
    while let Some(&token) = tokens.get(at) {
        at += 1;
        if open.is_none() && token.space() {
            continue;
        }
        let sentence = open.get_or_insert(Sentence::new(token.start()..token.end()));
        sentence.range.end = token.end();
        let c = match token {
            Token::Whole {
                piece: Piece::Source { found },
                ..
            } => {
                sentence.found |= found;
                continue;
            }
            Token::Whole { .. } => {
                sentence.marked = true;
                continue;
            }
            Token::Char { c, .. } => c,
        };
        sentence.asks |= c == '?';
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = (depth - 1).max(0),
            '.' | '!' | '?' if depth == 0 && ends(tokens, at - 1) => {
                sentence.ended = true;
                while let Some(next) =
                    (tokens.get(at..).unwrap_or_default().iter()).position(|token| !token.space())
                {
                    let Some(token) = tokens.get(at + next) else {
                        break;
                    };
                    let Some(found) = token.source() else {
                        break;
                    };
                    sentence.found |= found;
                    sentence.range.end = token.end();
                    at += next + 1;
                }
                sentences.extend(open.take());
            }
            _ => {}
        }
    }
    sentences.extend(open);
    sentences
}

/// Whether the `.`, `!` or `?` at `tokens[at]` surely ends a sentence.
fn ends(tokens: &[Token], at: usize) -> bool {
    let Some(Token::Char { c, .. }) = tokens.get(at).copied() else {
        return false;
    };
    if c == '.' {
        let word: String = (tokens.get(..at).unwrap_or_default().iter().rev())
            .map_while(|token| match token {
                Token::Char { c, .. } if c.is_alphanumeric() || *c == '.' || *c == '\'' => Some(*c),
                _ => None,
            })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let before = at.checked_sub(1).and_then(|before| tokens.get(before));
        let after_source = word.is_empty() && before.is_some_and(|token| token.source().is_some());
        let letters = word.chars().filter(|c| c.is_alphanumeric()).count();
        let shortened =
            word.contains('.') || letters <= 1 || SHORTENED.contains(&word.to_lowercase().as_str());
        if shortened && !after_source {
            return false;
        }
    }
    let rest = tokens.get(at + 1..).unwrap_or_default();
    match rest.first() {
        None => return true,
        Some(token) if token.space() || token.source().is_some() => {}
        Some(_) => return false,
    }
    let next = rest
        .iter()
        .find(|token| !token.space() && token.source().is_none());
    match next {
        None => true,
        Some(Token::Char { c, .. }) => c.is_uppercase() || c.is_ascii_digit(),
        Some(Token::Whole { .. }) => false,
    }
}
