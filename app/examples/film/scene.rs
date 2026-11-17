//! A film's scene file: its words and steps, read from text.

use super::*;

#[derive(Clone, Debug)]
pub(super) struct Word {
    pub(super) text: String,
    pub(super) quoted: bool,
}

#[derive(Debug)]
pub(super) struct Step {
    pub(super) line: usize,
    pub(super) verb: String,
    pub(super) words: Vec<Word>,
    pub(super) background: bool,
}

pub(super) fn read_scene(text: &str) -> anyhow::Result<Vec<Step>> {
    let mut steps = Vec::new();
    for (at, line) in text.lines().enumerate() {
        let mut words = Vec::new();
        let mut chars = line.chars().peekable();
        while let Some(&next) = chars.peek() {
            if next.is_whitespace() {
                chars.next();
            } else if next == '#' {
                break;
            } else if next == '"' {
                chars.next();
                let mut text = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => text.extend(chars.next()),
                        _ => text.push(c),
                    }
                }
                words.push(Word { text, quoted: true });
            } else {
                let mut text = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() {
                        break;
                    }
                    text.push(c);
                    chars.next();
                }
                words.push(Word {
                    text,
                    quoted: false,
                });
            }
        }
        let background = words
            .last()
            .is_some_and(|word| !word.quoted && word.text == "&");
        if background {
            words.pop();
        }
        if words.is_empty() {
            continue;
        }
        let verb = words.remove(0).text;
        steps.push(Step {
            line: at + 1,
            verb,
            words,
            background,
        });
    }
    Ok(steps)
}

impl Step {
    pub(super) fn number(&self, at: usize) -> Option<f32> {
        self.words.get(at).and_then(|word| word.text.parse().ok())
    }

    pub(super) fn numbers(&self, at: usize) -> Option<Vec<f32>> {
        self.words.get(at).and_then(|word| {
            word.text
                .split(',')
                .map(|part| part.trim().parse().ok())
                .collect()
        })
    }

    pub(super) fn text(&self, at: usize) -> anyhow::Result<&str> {
        self.words
            .get(at)
            .map(|word| word.text.as_str())
            .ok_or_else(|| anyhow!("line {}: {} needs more", self.line, self.verb))
    }
}
