use std::collections::HashMap;
use std::ops::Range;

use irosashi::{ScopeListId, Session, StateStack, TokenizeOptions};

use crate::{Style, Styles};

/// A styled run of a line, in bytes from the line's start. Runs in the text colour
/// are left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub style: Style,
}

/// Lines still to tokenize: from `next`, and at least through `through`, an edit's
/// last line.
#[derive(Clone, Copy)]
struct Pending {
    next: usize,
    through: usize,
}

/// A text's lines with the tokenizer state each one starts in, so an edit retokenizes
/// from the edited line until a line ends in the state it ended in before. Lines are
/// tokenized only as far as asked, so a long text never read to its end costs only
/// what was read.
pub struct Document {
    session: Session,
    options: TokenizeOptions,
    states: Vec<StateStack>,
    spans: Vec<Vec<Span>>,
    /// Lines before this one have been tokenized at least once, so the state each
    /// one ends in is known.
    frontier: usize,
    pending: Option<Pending>,
    styles: HashMap<ScopeListId, Style>,
}

impl Document {
    /// Lines longer than `max_line_length` bytes stay in the text colour and keep the
    /// state they start in.
    pub fn new(session: Session, max_line_length: Option<usize>) -> Self {
        let initial = session.initial_state();
        Self {
            session,
            options: TokenizeOptions { max_line_length },
            states: vec![initial],
            spans: Vec::new(),
            frontier: 0,
            pending: None,
            styles: HashMap::new(),
        }
    }

    pub fn line_count(&self) -> usize {
        self.spans.len()
    }

    /// Replaces every line, as when the whole text changes.
    pub fn reset(&mut self, lines: usize) {
        let initial = self.session.initial_state();
        self.states = vec![initial; lines + 1];
        self.spans = vec![Vec::new(); lines];
        self.frontier = 0;
        self.pending = (lines > 0).then_some(Pending {
            next: 0,
            through: 0,
        });
    }

    /// Lines `start..=old_end` became lines `start..=new_end`.
    pub fn edit(&mut self, start: usize, old_end: usize, new_end: usize) {
        let old_end = old_end.min(self.spans.len().saturating_sub(1)).max(start);
        if start >= self.spans.len() {
            let added = new_end + 1 - self.spans.len();
            let last = self.states[self.spans.len()].clone();
            self.spans.extend(std::iter::repeat_n(Vec::new(), added));
            self.states.extend(std::iter::repeat_n(last, added));
        } else {
            let placeholder = self.states[start].clone();
            self.spans.splice(
                start..=old_end,
                std::iter::repeat_n(Vec::new(), new_end - start + 1),
            );
            self.states.splice(
                start + 1..=old_end,
                std::iter::repeat_n(placeholder, new_end - start),
            );
        }
        let shift = |line: usize| {
            if line <= start {
                line
            } else if line > old_end {
                line + new_end - old_end
            } else {
                new_end
            }
        };
        self.frontier = if self.frontier > old_end {
            shift(self.frontier)
        } else {
            self.frontier.min(start)
        };
        let pending = match self.pending {
            Some(pending) => Pending {
                next: shift(pending.next).min(start),
                through: shift(pending.through).max(new_end),
            },
            None => Pending {
                next: start,
                through: new_end,
            },
        };
        self.pending = Some(Pending {
            next: pending.next,
            through: pending.through.min(self.spans.len() - 1),
        });
    }

    /// The first line still to tokenize, if any.
    pub fn pending_line(&self) -> Option<usize> {
        self.pending.map(|pending| pending.next)
    }

    /// Tokenizes the pending lines before line `until`, reading each through `line`,
    /// and returns the lines it tokenized. It stops early where an edit settles, so
    /// call it again while lines before `until` are pending.
    pub fn tokenize<L: AsRef<str>>(
        &mut self,
        styles: &Styles,
        mut line: impl FnMut(usize) -> L,
        until: usize,
    ) -> Range<usize> {
        let Some(mut pending) = self.pending else {
            return 0..0;
        };
        let first = pending.next;
        while pending.next < until.min(self.spans.len()) {
            let index = pending.next;
            let text = line(index);
            let text = text.as_ref();
            let result =
                self.session
                    .tokenize_line(text, &self.states[index], index == 0, self.options);
            let mut spans: Vec<Span> = Vec::new();
            for token in &result.tokens {
                let style = *self.styles.entry(token.scopes).or_insert_with(|| {
                    styles.style(self.session.scope_names(token.scopes).into_iter())
                });
                if style == Style::default() || token.start == token.end {
                    continue;
                }
                match spans.last_mut() {
                    Some(last) if last.end == token.start as u32 && last.style == style => {
                        last.end = token.end as u32
                    }
                    _ => spans.push(Span {
                        start: token.start as u32,
                        end: token.end as u32,
                        style,
                    }),
                }
            }
            self.spans[index] = spans;
            let known = index + 1 < self.frontier;
            let settled =
                known && index >= pending.through && self.states[index + 1] == result.state;
            self.states[index + 1] = result.state;
            pending.next += 1;
            self.frontier = self.frontier.max(pending.next);
            if settled {
                pending.next = self.frontier;
            }
            if pending.next == self.spans.len() {
                self.pending = None;
                return first..index + 1;
            }
            if settled {
                self.pending = Some(pending);
                return first..index + 1;
            }
        }
        self.pending = Some(pending);
        first..pending.next
    }

    /// The styled runs of `line`, empty for a line not tokenized yet.
    pub fn spans(&self, line: usize) -> &[Span] {
        self.spans.get(line).map_or(&[], Vec::as_slice)
    }

    /// About how much memory the lines' states and runs take.
    pub fn bytes(&self) -> usize {
        let runs: usize = self
            .spans
            .iter()
            .map(|line| {
                std::mem::size_of::<Vec<Span>>() + line.capacity() * std::mem::size_of::<Span>()
            })
            .sum();
        runs + self.states.capacity() * std::mem::size_of::<StateStack>()
    }
}
