//! Review comments on a card's pull request: what is new since Auto last
//! looked, the message that hands them to the card's agent, and the replies
//! made of how its turn ended.

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use super::codehost::Feedback;

/// How many times Auto hands a pull request's new comments to its agent
/// before the card goes back to the person.
pub(super) const MAX_ROUNDS: u32 = 3;
const MAX_HANDLED: usize = 500;
const MAX_QUOTE_CHARS: usize = 2_000;
/// Ends every reply Auto posts, so reading the threads again skips them.
const SIGNED: &str = "through Sikemux Auto";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Review {
    /// What the pull request looked like when its comments were last read.
    mark: String,
    /// The newest comment's time on the host, which the next read starts from.
    since: Option<String>,
    handled: VecDeque<String>,
    /// Handed to the agent, waiting for its turn to end to be answered.
    pub asked: Vec<Feedback>,
    pub rounds: u32,
}

impl Review {
    /// The pull request moved since its comments were last read.
    pub(super) fn moved(&self, mark: &str) -> bool {
        self.mark != mark
    }

    pub(super) fn since(&self) -> Option<&str> {
        self.since.as_deref()
    }

    /// Takes in what was read at `mark`, giving back what is new: not from a
    /// bot, not a reply of Auto's and not handed to the agent before.
    pub(super) fn take(&mut self, mark: String, found: Vec<Feedback>) -> Vec<Feedback> {
        self.mark = mark;
        if let Some(newest) = found.iter().map(|item| &item.at).max() {
            if self.since.as_ref().is_none_or(|since| newest > since) {
                self.since = Some(newest.clone());
            }
        }
        found
            .into_iter()
            .filter(|item| {
                !item.bot && !item.body.contains(SIGNED) && !self.handled.contains(&item.id)
            })
            .collect()
    }

    pub(super) fn hand(&mut self, asked: Vec<Feedback>) {
        self.handled
            .extend(asked.iter().map(|item| item.id.clone()));
        while self.handled.len() > MAX_HANDLED {
            self.handled.pop_front();
        }
        self.asked = asked;
        self.rounds += 1;
    }
}

fn place(item: &Feedback) -> String {
    let who = item.author.as_deref().unwrap_or("A reviewer");
    match (&item.path, item.line) {
        (Some(path), Some(line)) => format!("{who} on {path}:{line}"),
        (Some(path), None) => format!("{who} on {path}"),
        _ if item.state.as_deref() == Some("CHANGES_REQUESTED") => {
            format!("{who}, requesting changes")
        }
        _ => who.to_owned(),
    }
}

/// What the agent is asked to do about the comments in `asked`.
pub(super) fn task(asked: &[Feedback], url: &str, branch: &str, round: u32) -> String {
    let mut text = format!("Reviewers left comments on your pull request {url}:");
    for (index, item) in asked.iter().enumerate() {
        let body: String = item.body.trim().chars().take(MAX_QUOTE_CHARS).collect();
        let body = if body.is_empty() {
            "> (no words; see their other comments)".to_owned()
        } else {
            body.lines()
                .map(|line| format!("> {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        text.push_str(&format!("\n\n#{} {}:\n{body}", index + 1, place(item)));
    }
    text.push_str(&format!(
        "\n\nAddress each on {branch}, run the checks that cover it and commit. Do not reply on the pull request yourself: end your turn with one line per comment, which Sikemux posts as your reply in its thread once it has pushed:\n#1 done: what you changed\n#2 skip: why you left it as it is\n#3 stuck: what you need from the person\nA stuck comment, or one you leave out, sends the card back to the person. This is round {round} of {MAX_ROUNDS}."
    ));
    text
}

#[derive(Clone, Copy)]
enum Verdict {
    Done,
    Skip,
    Stuck,
}

fn verdict_line(line: &str) -> Option<(usize, Verdict, String)> {
    let line = line.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '*' | '`'));
    let rest = line.strip_prefix('#')?;
    let digits = rest.find(|c: char| !c.is_ascii_digit())?;
    let number = rest[..digits].parse().ok()?;
    let (word, said) = rest[digits..].split_once(':')?;
    let verdict = match word
        .trim_matches(|c: char| c.is_whitespace() || c == '*')
        .to_lowercase()
        .as_str()
    {
        "done" => Verdict::Done,
        "skip" => Verdict::Skip,
        "stuck" => Verdict::Stuck,
        _ => return None,
    };
    Some((
        number,
        verdict,
        said.trim_matches(|c: char| c.is_whitespace() || c == '*')
            .to_owned(),
    ))
}

/// The agent's line for each comment, by its number, with the lines under it
/// up to a blank one.
fn verdicts(answer: &str) -> BTreeMap<usize, (Verdict, String)> {
    let mut found = BTreeMap::new();
    let mut open = None;
    for line in answer.lines() {
        if let Some((number, verdict, said)) = verdict_line(line) {
            found.insert(number, (verdict, said));
            open = Some(number);
        } else if line.trim().is_empty() {
            open = None;
        } else if let Some((_, said)) = open.and_then(|number| found.get_mut(&number)) {
            said.push('\n');
            said.push_str(line.trim_end());
        }
    }
    found
}

/// The replies to post, as `(comment id, reply, what it said)`, the
/// reviewers to ask to look again, and the comments the agent did not settle.
#[derive(Default)]
pub(super) struct Answers {
    pub replies: Vec<(String, String, String)>,
    pub reviewers: Vec<String>,
    pub stuck: Vec<String>,
}

pub(super) fn answers(asked: &[Feedback], answer: Option<&str>, name: &str, head: &str) -> Answers {
    let verdicts = answer.map(verdicts).unwrap_or_default();
    let short = &head[..head.len().min(7)];
    let mut answers = Answers::default();
    for (index, item) in asked.iter().enumerate() {
        let number = index + 1;
        let reply = match verdicts.get(&number) {
            Some((Verdict::Done, said)) => {
                format!("{said}\n\nChanged in {short}. _{name} {SIGNED}_")
            }
            Some((Verdict::Skip, said)) => format!("{said}\n\n_{name} {SIGNED}_"),
            Some((Verdict::Stuck, said)) => {
                answers
                    .stuck
                    .push(format!("#{number} {}: {said}", place(item)));
                continue;
            }
            None => {
                answers.stuck.push(format!(
                    "#{number} {}: {name} did not say how it handled it",
                    place(item)
                ));
                continue;
            }
        };
        answers
            .replies
            .push((item.id.clone(), reply.trim().to_owned(), item.body.clone()));
        if let Some(author) = &item.author {
            if !answers.reviewers.contains(author) {
                answers.reviewers.push(author.clone());
            }
        }
    }
    answers
}
