//! The files going with a draft, each kept in step with the pill that stands for it in the text:
//! a picture's says `[Image #n]`, its place among the files, and any other file's says `+name`,
//! as the `+` menu writes it. That is how the core finds where in the words each file belongs.

use std::collections::HashSet;
use std::ops::{Deref, Range};
use std::path::{Path, PathBuf};

use gpui_kit::base::input::{InlineToken, InlineTokenSpan, InputContent};
use sikemux_wire::protocol::ChatContext;

use super::attach::{about_uri, is_image, picture_label};
use super::mention::{FILE, FOLDER};

pub(super) const IMAGE: &str = "image:";

/// The file the token `id` stands for, when it stands for one.
pub(super) fn path_in(id: &str) -> Option<&str> {
    [IMAGE, FILE, FOLDER]
        .iter()
        .find_map(|kind| id.strip_prefix(kind))
}

/// The files `spans` stand for, in the order of the text, each once.
fn pilled(spans: &[InlineTokenSpan]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    (spans.iter())
        .filter_map(|span| path_in(span.token().id()))
        .filter(|path| seen.insert(*path))
        .map(PathBuf::from)
        .collect()
}

/// Where `spans` stand for `path`, each with the space before it.
pub(super) fn places(path: &Path, spans: &[InlineTokenSpan], text: &str) -> Vec<Range<usize>> {
    (spans.iter())
        .filter(|span| path_in(span.token().id()).is_some_and(|held| Path::new(held) == path))
        .map(|span| {
            let range = span.range();
            let before = text.get(..range.start).unwrap_or_default();
            range.start - usize::from(before.ends_with(' '))..range.end
        })
        .collect()
}

/// A file's pill in a draft that holds `spans`; `number` is its place among the files going.
pub(super) fn pill_of(path: &Path, number: usize, spans: &[InlineTokenSpan]) -> InlineToken {
    if is_image(path) {
        return picture_pill(path, number);
    }
    let named = path.to_string_lossy();
    let kind = if path.is_dir() { FOLDER } else { FILE };
    let tag = file_tag(&named, spans.iter().map(InlineTokenSpan::token));
    InlineToken::new(format!("{kind}{named}"), tag)
}

pub(super) fn picture_pill(path: &Path, number: usize) -> InlineToken {
    InlineToken::new(
        format!("{IMAGE}{}", path.to_string_lossy()),
        image_tag(number),
    )
    .with_label(picture_label(number, path))
}

/// `token` as it stands for its file at `path`, the one numbered `number`, beside `tokens`.
pub(super) fn moved(
    token: &InlineToken,
    path: &Path,
    number: usize,
    tokens: &[InlineToken],
) -> InlineToken {
    if token.id().starts_with(IMAGE) {
        return picture_pill(path, number);
    }
    let named = path.to_string_lossy();
    let kind = match token.id().starts_with(FOLDER) {
        true => FOLDER,
        false => FILE,
    };
    let tag = file_tag(&named, tokens.iter());
    InlineToken::new(format!("{kind}{named}"), tag)
}

fn image_tag(number: usize) -> String {
    format!("[Image #{number}]")
}

/// `+name` for the file at `path`, with as many of its folders as tell it from the other files
/// `tokens` stand for.
pub(super) fn file_tag<'a>(
    path: &str,
    tokens: impl Iterator<Item = &'a InlineToken> + Clone,
) -> String {
    let parts = parts(path);
    let written = (tokens.clone())
        .find(|token| path_in(token.id()) == Some(path) && token.text().starts_with('+'));
    if let Some(token) = written {
        return token.text().to_string();
    }
    let others: Vec<Vec<&str>> = tokens
        .filter_map(|token| path_in(token.id()))
        .filter(|other| *other != path)
        .map(self::parts)
        .collect();
    let tails = (0..parts.len()).rev().filter_map(|from| parts.get(from..));
    let told_apart = tails
        .clone()
        .find(|tail| !others.iter().any(|other| other.ends_with(tail)));
    format!("+{}", told_apart.unwrap_or(&parts).join("/"))
}

fn parts(path: &str) -> Vec<&str> {
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect()
}

/// `content` with each picture's pill saying the picture's place among `files`. While the person
/// writes, a pill's own words may lag behind what it draws: changing them would be an edit to undo.
pub(super) fn numbered(content: InputContent, files: &[PathBuf]) -> InputContent {
    let named = |token: &InlineToken| {
        let path = Path::new(token.id().strip_prefix(IMAGE)?);
        let number = 1 + files.iter().position(|file| file == path)?;
        (token.text().as_ref() != image_tag(number)).then(|| picture_pill(path, number))
    };
    if !(content.tokens().iter()).any(|span| named(span.token()).is_some()) {
        return content;
    }
    rewritten(&content, |token| {
        Some(named(token).unwrap_or_else(|| token.clone()))
    })
}

/// `content` with each token as `change` makes it, its words alone where that is none.
pub(super) fn rewritten(
    content: &InputContent,
    mut change: impl FnMut(&InlineToken) -> Option<InlineToken>,
) -> InputContent {
    let mut text = String::new();
    let mut tokens = Vec::new();
    let mut rest = 0;
    for span in content.tokens() {
        let range = span.range();
        text.push_str(content.text().get(rest..range.start).unwrap_or_default());
        rest = range.end;
        let start = text.len();
        match change(span.token()) {
            Some(token) => {
                text.push_str(token.text());
                tokens.push((start..text.len(), token));
            }
            None => text.push_str(span.token().text()),
        }
    }
    text.push_str(content.text().get(rest..).unwrap_or_default());
    with_tokens(text, tokens)
}

/// `text` with the tokens that fit it.
pub(super) fn with_tokens(
    text: impl Into<gpui_kit::SharedString>,
    tokens: impl IntoIterator<Item = (Range<usize>, InlineToken)>,
) -> InputContent {
    (tokens.into_iter()).fold(InputContent::new(text), |content, (range, token)| {
        content.clone().with_token(range, token).unwrap_or(content)
    })
}

/// The files going with a draft, in the order the message numbers them.
#[derive(Default)]
pub struct Files {
    going: Vec<PathBuf>,
    /// The files that had a pill when the draft last changed.
    pilled: HashSet<PathBuf>,
    /// The files taken off with their pill, and what the agent alone is told of each, so undoing
    /// that brings them back.
    let_go: Vec<(PathBuf, Vec<ChatContext>)>,
}

impl Deref for Files {
    type Target = [PathBuf];

    fn deref(&self) -> &[PathBuf] {
        &self.going
    }
}

impl Files {
    pub(super) fn of(going: Vec<PathBuf>, spans: &[InlineTokenSpan]) -> Self {
        Self {
            going,
            pilled: pilled(spans).into_iter().collect(),
            let_go: Vec::new(),
        }
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Where `path` stands among the files going, from one.
    pub(super) fn number(&self, path: &Path) -> Option<usize> {
        self.iter().position(|file| file == path).map(|at| at + 1)
    }

    /// Adds `path` unless it is already going; its place, unless the message holds no more.
    pub(super) fn add(&mut self, path: &Path) -> Option<usize> {
        if let Some(number) = self.number(path) {
            return Some(number);
        }
        if self.len() == sikemux_calls::chat::MAX_ATTACHMENTS {
            return None;
        }
        self.going.push(path.to_owned());
        Some(self.len())
    }

    /// Takes `path` off, with its notes in `context`.
    pub(super) fn let_go(&mut self, path: &Path, context: &mut Vec<ChatContext>) {
        let Some(at) = self.iter().position(|file| file == path) else {
            return;
        };
        let path = self.going.remove(at);
        let uri = about_uri(&path);
        let notes = context.extract_if(.., |item| item.uri == uri).collect();
        self.let_go.retain(|(held, _)| *held != path);
        self.let_go.push((path, notes));
    }

    /// Follows the draft, which now holds `spans`: a file whose pill is gone goes, one whose pill
    /// came back is back with its notes, and those with a pill stand in the order of the text.
    pub(super) fn settle(&mut self, spans: &[InlineTokenSpan], context: &mut Vec<ChatContext>) {
        let shown = pilled(spans);
        let gone: Vec<PathBuf> = (self.pilled.iter())
            .filter(|path| !shown.contains(path))
            .cloned()
            .collect();
        for path in gone {
            self.let_go(&path, context);
        }
        for path in &shown {
            let held = self.let_go.iter().position(|(held, _)| held == path);
            if let Some(held) = held.filter(|_| self.add(path).is_some()) {
                context.extend(self.let_go.remove(held).1);
            }
        }
        let (pictures, others): (Vec<PathBuf>, Vec<PathBuf>) =
            (shown.iter().cloned()).partition(|path| is_image(path));
        let mut ordered = (pictures.into_iter().chain(others))
            .filter(|path| self.going.contains(path))
            .collect::<Vec<_>>()
            .into_iter();
        for file in &mut self.going {
            if shown.contains(file) {
                if let Some(next) = ordered.next() {
                    *file = next;
                }
            }
        }
        self.pilled = shown.into_iter().collect();
    }
}
