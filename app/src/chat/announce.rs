//! What a screen reader hears of the chat in front while its turn runs: how the turn stands,
//! each message the agent finishes and then keeps working after, and the step it is on, as
//! the person's `ScreenReader` preference allows. The turn's closing reply is never read; the
//! person hears "Done" and reads it themselves.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui::accesskit::Live;
use gpui::Role;
use gpui_kit::{
    div, px, Context, InteractiveElement, IntoElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window,
};
use sikemux_view::chat::{ChatPart, ChatRole};
use sikemux_view::prefs::ScreenReader;
use sikemux_view::turn::{TurnPhase, TurnState};

use super::ChatRow;

/// A message longer than this many characters is read up to here.
const LONGEST_READ: usize = 2_000;
/// Steps closer together than this are passed over for the next one.
const STEP_GAP: Duration = Duration::from_millis(1_500);

#[derive(Default)]
pub struct Announcer {
    level: ScreenReader,
    /// The agent's `@name`, said before its messages.
    name: Option<SharedString>,
    /// How the turn stood when last heard; none until the chat's first head, which is taken
    /// silently, as is everything already in it.
    phase: Option<Option<TurnPhase>>,
    /// The running turn's text parts already read or passed over.
    heard: HashSet<String>,
    step: Option<SharedString>,
    step_at: Option<Instant>,
    said: SharedString,
}

impl Announcer {
    pub fn wear(&mut self, level: ScreenReader, name: Option<&str>) {
        self.level = level;
        if self.name.as_deref() != name {
            self.name = name.map(|name| format!("@{}", name.trim_start_matches('@')).into());
        }
    }

    /// Hears the chat as it now stands, and whether it has something new to say that the
    /// person asked to hear.
    pub fn follow(
        &mut self,
        turn: Option<&TurnState>,
        rows: &[ChatRow],
        step: Option<SharedString>,
    ) -> bool {
        let phase = turn.map(|turn| turn.state);
        let Some(was) = self.phase.replace(phase) else {
            self.heard = finished_texts(rows).map(|(id, _)| id.to_owned()).collect();
            self.step = step;
            return false;
        };
        let mut words = Vec::new();
        let working = phase == Some(TurnPhase::Working);
        if working && !matches!(was, Some(TurnPhase::Working | TurnPhase::NeedsYou)) {
            self.heard = finished_texts(rows).map(|(id, _)| id.to_owned()).collect();
        }
        if self.level != ScreenReader::Status {
            for (id, text) in finished_texts(rows) {
                if self.heard.insert(id.to_owned()) && working {
                    words.push(self.message(text));
                }
            }
        }
        if phase != was {
            words.extend(turn.and_then(|turn| state_words(turn, was)));
        }
        let step = step.filter(|_| working);
        if self.level == ScreenReader::Everything && step.is_some() && step != self.step {
            let spaced = self.step_at.is_none_or(|at| at.elapsed() >= STEP_GAP);
            if spaced || !words.is_empty() {
                words.extend(step.clone().map(String::from));
                self.step_at = Some(Instant::now());
            }
        }
        self.step = step;
        if words.is_empty() {
            return false;
        }
        let said = words.join(". ");
        self.said = if *self.said == said {
            format!("{said}\u{00a0}").into()
        } else {
            said.into()
        };
        true
    }

    fn message(&self, markdown: &str) -> String {
        let mut text = spoken(markdown);
        if text.chars().count() > LONGEST_READ {
            text = text.chars().take(LONGEST_READ).collect::<String>() + "… More in the chat";
        }
        match &self.name {
            Some(name) => format!("{name}: {text}"),
            None => text,
        }
    }
}

impl Render for Announcer {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let region = div()
            .id("chat-announcer")
            .role(Role::Status)
            .absolute()
            .top_0()
            .left_0()
            .size(px(1.))
            .a11y_synthetic_children(|tree| tree.parent_node().set_live(Live::Polite));
        if self.said.is_empty() {
            region
        } else {
            region.aria_value(self.said.clone())
        }
    }
}

/// The words for a turn come to stand as it does, from how it stood.
fn state_words(turn: &TurnState, was: Option<TurnPhase>) -> Option<String> {
    Some(match turn.state {
        TurnPhase::Working if was == Some(TurnPhase::NeedsYou) => return None,
        TurnPhase::Working => "Working".into(),
        TurnPhase::NeedsYou => "Needs you".into(),
        TurnPhase::Waiting => match &turn.wait {
            Some(wait) if !wait.what.is_empty() => format!("Waiting for {}", wait.what),
            _ => "Waiting".into(),
        },
        TurnPhase::Done => "Done".into(),
        TurnPhase::Stopped => "Stopped".into(),
        TurnPhase::Failed => "Failed".into(),
        TurnPhase::Unknown => return None,
    })
}

/// The agent's text parts in the latest turn that it went on working after, by id, oldest
/// first. The turn begins after the person's last prompt, or with the message that opened it.
fn finished_texts(rows: &[ChatRow]) -> impl Iterator<Item = (&str, &str)> {
    let start = rows
        .iter()
        .rposition(|row| row.message.role == ChatRole::User || row.message.opens_turn.is_some())
        .map_or(0, |at| at + 1);
    let parts: Vec<&ChatPart> = rows[start..]
        .iter()
        .filter(|row| row.message.role != ChatRole::User)
        .flat_map(|row| row.message.parts.iter())
        .collect();
    let last = parts.len().saturating_sub(1);
    parts
        .into_iter()
        .enumerate()
        .filter(move |(at, _)| *at < last)
        .filter_map(|(_, part)| match part {
            ChatPart::Text { id, text, .. } if !text.trim().is_empty() => {
                Some((id.as_str(), text.as_str()))
            }
            _ => None,
        })
}

/// Markdown as it reads aloud: without the marks that only shape it, and a link as its words.
fn spoken(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    for line in markdown.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            continue;
        }
        let line = line.trim_start_matches(['#', '>', ' ']);
        let line = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .unwrap_or(line);
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '*' | '`' => {}
                ']' if chars.peek() == Some(&'(') => {
                    for c in chars.by_ref() {
                        if c == ')' {
                            break;
                        }
                    }
                }
                '[' => {}
                c => out.push(c),
            }
        }
        if !line.is_empty() {
            out.push(' ');
        }
    }
    out.trim_end().to_owned()
}
