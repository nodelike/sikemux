//! The footer of a commit agents made: one line per agent whose changes it
//! holds, then the host's signature over them.
//!
//! ```text
//! Agent: Otter (claude 2.1.257, opus 4.7, sikemux 0.5.3, session 5f0c…, chat 0199…, host 7f3a9c21d0e4b5a6)
//! Agent-Signature: <ed25519 signature in base64>
//! ```
//!
//! The host signs every field of every line with the commit's tree and
//! parents, so a footer written or edited by hand, or copied onto another
//! commit, does not check out. A field the host did not know is left out.
//!
//! An agent is known by the session its CLI keeps, which resuming it needs and
//! which outlives the chat. The chat is Sikemux's agent, a new one each time
//! the session is opened again, and stands in for the session until the CLI
//! has said which it runs.

use base64::engine::general_purpose::STANDARD_NO_PAD as BASE64;
use base64::Engine;
use iroh::{PublicKey, SecretKey, Signature};
use serde::{Deserialize, Serialize};

const AGENT: &str = "Agent: ";
const SIGNATURE: &str = "Agent-Signature: ";
const SIKEMUX: &str = "sikemux ";
const SESSION: &str = "session ";
const CHAT: &str = "chat ";
/// How much of the host's key an agent line shows, enough to pick it out of
/// the hosts a reader knows.
const HOST_CHARS: usize = 16;
const SCISSORS: &str = " ------------------------ >8 ------------------------";

/// Where the notes saying which lines of a commit each of its agents wrote
/// are kept, for commits no one writer has all the lines of.
pub const NOTES_REF: &str = "refs/notes/agents";

/// What a commit's hooks put on it once git has made it, if it is the commit
/// of `tree` and `parents`: `note`, as [`signed_note`] makes it, or, with
/// none, no note at all, taking away one an amend copied from the commit it
/// replaced.
#[derive(Serialize, Deserialize)]
pub struct PendingNote {
    pub tree: String,
    pub parents: Vec<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub name: String,
    pub cli: String,
    /// The version of the agent's CLI.
    pub cli_version: Option<String>,
    pub model: Option<String>,
    /// The version of Sikemux the agent ran in.
    pub sikemux: Option<String>,
    /// The CLI's own session.
    pub session: Option<String>,
    pub chat: String,
}

impl Agent {
    /// What the agent is known by: its session, else its chat.
    pub fn key(&self) -> &str {
        self.session.as_deref().unwrap_or(&self.chat)
    }

    /// `self` with every field as its line can show it, so the line reads
    /// back as the agent it was written from.
    fn shown(&self) -> Self {
        let optional = |text: Option<&String>, read: fn(&str) -> String| {
            text.map(|text| read(text)).filter(|text| !text.is_empty())
        };
        let model = optional(self.model.as_ref(), words).filter(|model| {
            ![SIKEMUX, SESSION, CHAT]
                .iter()
                .any(|label| model.starts_with(label))
        });
        Self {
            name: words(&self.name),
            cli: one_word(&self.cli),
            cli_version: optional(self.cli_version.as_ref(), one_word),
            model,
            sikemux: optional(self.sikemux.as_ref(), one_word),
            session: optional(self.session.as_ref(), one_word),
            chat: one_word(&self.chat),
        }
    }
}

/// `text` on one line without the marks that split an agent line.
fn words(text: &str) -> String {
    let plain: String = text
        .chars()
        .map(|c| match c {
            ',' | '(' | ')' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn one_word(text: &str) -> String {
    words(text).replace(' ', "-")
}

/// What a commit is made of besides its message, which the signature covers.
pub struct Commit<'a> {
    pub tree: &'a str,
    pub parents: &'a [String],
}

/// The footer naming `agents` in `commit`, signed with the host's `key`.
pub fn footer(agents: &[Agent], commit: &Commit, key: Option<&SecretKey>) -> Vec<String> {
    let agents: Vec<Agent> = agents
        .iter()
        .map(Agent::shown)
        .filter(|agent| !agent.name.is_empty() && !agent.cli.is_empty() && !agent.chat.is_empty())
        .collect();
    let host = key.map(SecretKey::public);
    let mut lines: Vec<String> = agents
        .iter()
        .map(|agent| agent_line(agent, host.as_ref()))
        .collect();
    if let (Some(key), Some(host), false) = (key, host, agents.is_empty()) {
        let signature = key.sign(signed(commit, &host, &agents).as_bytes());
        lines.push(format!(
            "{SIGNATURE}{}",
            BASE64.encode(signature.to_bytes())
        ));
    }
    lines
}

fn agent_line(agent: &Agent, host: Option<&PublicKey>) -> String {
    let cli = match &agent.cli_version {
        Some(version) => format!("{} {version}", agent.cli),
        None => agent.cli.clone(),
    };
    let sikemux = agent
        .sikemux
        .as_ref()
        .map(|version| format!("{SIKEMUX}{version}"));
    let session = agent
        .session
        .as_ref()
        .map(|session| format!("{SESSION}{session}"));
    let chat = format!("{CHAT}{}", agent.chat);
    let host = host.map(|host| format!("host {}", short(host)));
    let parts: Vec<&str> = [
        Some(cli.as_str()),
        agent.model.as_deref(),
        sikemux.as_deref(),
        session.as_deref(),
        Some(chat.as_str()),
        host.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();
    format!("{AGENT}{} ({})", agent.name, parts.join(", "))
}

fn short(host: &PublicKey) -> String {
    host.to_string()[..HOST_CHARS].to_owned()
}

/// Each field by its length, so no field can run into the next.
fn field(text: &str) -> String {
    format!("{}:{text}", text.len())
}

/// What the host signs of `commit` for a `kind` of statement about it.
fn signed_commit(kind: &str, commit: &Commit, host: &PublicKey) -> String {
    let mut text = format!("sikemux agent {kind}\ntree {}\n", field(commit.tree));
    for parent in commit.parents {
        text.push_str(&format!("parent {}\n", field(parent)));
    }
    text.push_str(&format!("host {}\n", field(&host.to_string())));
    text
}

/// What the host signs of a footer.
fn signed(commit: &Commit, host: &PublicKey, agents: &[Agent]) -> String {
    let optional = |text: &Option<String>| field(text.as_deref().unwrap_or_default());
    let mut text = signed_commit("footer 3", commit, host);
    for agent in agents {
        text.push_str(&format!(
            "agent {} {} {} {} {} {} {}\n",
            field(&agent.name),
            field(&agent.cli),
            optional(&agent.cli_version),
            optional(&agent.model),
            optional(&agent.sikemux),
            optional(&agent.session),
            field(&agent.chat),
        ));
    }
    text
}

/// The note `note` on `commit`, as its blob holds it: the note, then the
/// host's signature over it with the commit's tree and parents, so a note
/// written or changed by hand, or copied onto another commit, does not check
/// out.
pub fn signed_note(note: &str, commit: &Commit, key: &SecretKey) -> String {
    let text = note_signed(note, commit, &key.public());
    let signature = key.sign(text.as_bytes());
    format!(
        "{note}\n{SIGNATURE}{}\n",
        BASE64.encode(signature.to_bytes())
    )
}

/// The note a note's `blob` holds, when one of `hosts` signed it for `commit`.
pub fn verify_note<'a>(blob: &'a str, commit: &Commit, hosts: &[PublicKey]) -> Option<&'a str> {
    let (note, signature) = blob.trim_end().rsplit_once('\n')?;
    let signature = BASE64.decode(signature.strip_prefix(SIGNATURE)?).ok()?;
    let signature = Signature::from_bytes(&signature.try_into().ok()?);
    hosts
        .iter()
        .any(|host| {
            host.verify(note_signed(note, commit, host).as_bytes(), &signature)
                .is_ok()
        })
        .then_some(note)
}

fn note_signed(note: &str, commit: &Commit, host: &PublicKey) -> String {
    let mut text = signed_commit("note 2", commit, host);
    text.push_str(&format!("note {}\n", field(note)));
    text
}

/// The agents a commit message's footer names, whether or not it checks out.
pub fn agents(message: &str) -> Vec<Agent> {
    read(message)
        .0
        .into_iter()
        .map(|(agent, _)| agent)
        .collect()
}

/// The agents `message`'s footer names, and the host of `hosts` that signed
/// it, when its signature checks out for `commit`.
pub fn verify(
    message: &str,
    commit: &Commit,
    hosts: &[PublicKey],
) -> Option<(PublicKey, Vec<Agent>)> {
    let (named, signature) = read(message);
    let signature = Signature::from_bytes(&BASE64.decode(signature?).ok()?.try_into().ok()?);
    let short_host = named.first()?.1.clone()?;
    if named
        .iter()
        .any(|(_, host)| host.as_ref() != Some(&short_host))
    {
        return None;
    }
    let agents: Vec<Agent> = named.into_iter().map(|(agent, _)| agent).collect();
    let host = hosts.iter().find(|host| {
        short(host) == short_host
            && host
                .verify(signed(commit, host, &agents).as_bytes(), &signature)
                .is_ok()
    })?;
    Some((*host, agents))
}

/// Each agent line of `message`'s footer with the host it names, and its
/// signature.
fn read(message: &str) -> (Vec<(Agent, Option<String>)>, Option<&str>) {
    let lines = last_paragraph(without_closing_comments(message)).lines();
    let mut agents = Vec::new();
    let mut signature = None;
    for line in lines {
        if let Some(value) = line.strip_prefix(SIGNATURE) {
            signature = Some(value.trim());
        } else if let Some(agent) = line.strip_prefix(AGENT).and_then(parse_agent) {
            agents.push(agent);
        }
    }
    (agents, signature)
}

fn parse_agent(value: &str) -> Option<(Agent, Option<String>)> {
    let (name, rest) = value.split_once(" (")?;
    let mut parts: Vec<&str> = rest.strip_suffix(')')?.split(", ").collect();
    let host = match parts.last()?.strip_prefix("host ") {
        Some(host) => {
            parts.pop();
            Some(host.to_owned())
        }
        None => None,
    };
    let chat = parts.pop()?.strip_prefix(CHAT)?.to_owned();
    let session = match parts.last()?.strip_prefix(SESSION) {
        Some(session) if parts.len() > 1 => {
            parts.pop();
            Some(session.to_owned())
        }
        _ => None,
    };
    let sikemux = match parts.last()?.strip_prefix(SIKEMUX) {
        Some(version) if parts.len() > 1 => {
            parts.pop();
            Some(version.to_owned())
        }
        _ => None,
    };
    let (cli, model) = match parts.as_slice() {
        [cli] => (*cli, None),
        [cli, model] => (*cli, Some((*model).to_owned())),
        _ => return None,
    };
    let (cli, cli_version) = match cli.split_once(' ') {
        Some((cli, version)) => (cli, Some(version.to_owned())),
        None => (cli, None),
    };
    let name = name.trim();
    (!name.is_empty() && !chat.is_empty()).then(|| {
        let agent = Agent {
            name: name.to_owned(),
            cli: cli.to_owned(),
            cli_version,
            model,
            sikemux,
            session,
            chat,
        };
        (agent, host)
    })
}

/// `message` with `footer` at its end, in its own trailer block if it has
/// one, in place of any footer it had.
pub fn append(message: &str, footer: &[String]) -> String {
    let message = without_footer(message.trim_end());
    if footer.is_empty() {
        return message;
    }
    let joined = if message.contains("\n\n") && last_paragraph(&message).lines().all(is_trailer) {
        "\n"
    } else {
        "\n\n"
    };
    format!("{message}{joined}{}\n", footer.join("\n"))
}

/// The commit message file git is about to commit, with `footer` in place of
/// any it had, ahead of the comments and the cut line git takes out. None when
/// the message says nothing, which git refuses to commit.
pub fn into_message_file(text: &str, footer: &[String]) -> Option<String> {
    let (content, comments, cut) = message_file(text)?;
    if without_footer(content.trim_end()).trim().is_empty() {
        return None;
    }
    let comments = comments.trim_start_matches('\n');
    let gap = if comments.is_empty() { "" } else { "\n" };
    Some(format!("{}{gap}{comments}{cut}", append(content, footer)))
}

/// Whether the commit message file already ends in a signed footer.
pub fn is_signed(text: &str) -> bool {
    message_file(text).is_some_and(|(content, _, _)| read(content).1.is_some())
}

/// A commit message file's message, the comments after it, and the cut line
/// with what follows it.
fn message_file(text: &str) -> Option<(&str, &str, &str)> {
    let cut_at = line_starts(text)
        .find(|at| first_line(&text[*at..]).get(1..) == Some(SCISSORS))
        .unwrap_or(text.len());
    let (body, cut) = text.split_at(cut_at);
    let said = |line: &str| !line.trim().is_empty() && !line.starts_with('#');
    let end = line_starts(body)
        .filter(|at| said(first_line(&body[*at..])))
        .last()
        .map(|at| at + first_line(&body[at..]).len())?;
    let (content, comments) = body.split_at(end);
    Some((content, comments, cut))
}

fn first_line(text: &str) -> &str {
    text.split('\n').next().unwrap_or_default()
}

fn line_starts(text: &str) -> impl Iterator<Item = usize> + '_ {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .filter(move |at| *at < text.len())
}

/// `message` without the footer lines of its trailer block.
fn without_footer(message: &str) -> String {
    let Some((before, last)) = message.rsplit_once("\n\n") else {
        return message.to_owned();
    };
    if !last.lines().all(is_trailer) {
        return message.to_owned();
    }
    let kept: Vec<&str> = last
        .lines()
        .filter(|line| !line.starts_with(AGENT) && !line.starts_with(SIGNATURE))
        .collect();
    match kept.as_slice() {
        [] => before.trim_end().to_owned(),
        kept => format!("{before}\n\n{}", kept.join("\n")),
    }
}

/// `message` without the comment lines git keeps at its end when it does not
/// clean the message up, such as the conflicts of a merge made with `--no-edit`.
fn without_closing_comments(message: &str) -> &str {
    let mut message = message.trim_end();
    while let Some((before, last)) = message.rsplit_once('\n') {
        if !last.starts_with('#') {
            break;
        }
        message = before.trim_end();
    }
    message
}

fn last_paragraph(message: &str) -> &str {
    message.rsplit("\n\n").next().unwrap_or_default()
}

fn is_trailer(line: &str) -> bool {
    line.split_once(": ").is_some_and(|(key, _)| {
        !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}
