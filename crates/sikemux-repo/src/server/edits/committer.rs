//! Who made a commit in a repository whose commits name no agent, and whose
//! lines it holds, so an agent can still push its own and the commit still
//! tells who wrote it: a signed git note under [`NOTES_REF`] that stays on this
//! computer, since pushing a branch does not push notes.

use crate::server::remote_state::RemoteParts;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::owners::{git, run};
use crate::server::Core;
use crate::trailers::{self, Agent, Commit, COMMITTERS_REF as NOTES_REF};

const NOTE_ATTEMPTS: usize = 3;

#[derive(Deserialize, Serialize)]
pub struct Committer {
    pub name: String,
    pub session: Option<String>,
    pub chat: String,
    #[serde(default)]
    pub cli: String,
    #[serde(default)]
    pub model: Option<String>,
    /// The other agents whose lines the commit holds, as a footer would have named them.
    #[serde(default)]
    pub with: Vec<Committer>,
}

impl Committer {
    /// The note naming `agents`, the first of them as who made the commit; none for no agent.
    pub fn of_agents(agents: &[Agent]) -> Option<Self> {
        let (made_by, with) = agents.split_first()?;
        let one = |agent: &Agent| Committer {
            name: agent.name.clone(),
            session: agent.session.clone(),
            chat: agent.chat.clone(),
            cli: agent.cli.clone(),
            model: agent.model.clone(),
            with: Vec::new(),
        };
        Some(Committer {
            with: with.iter().map(one).collect(),
            ..one(made_by)
        })
    }

    /// Every agent the note names, who made the commit first.
    pub fn agents(mut self) -> Vec<Agent> {
        let with = std::mem::take(&mut self.with);
        let one = |committer: Committer| Agent {
            name: committer.name,
            cli: committer.cli,
            cli_version: None,
            model: committer.model,
            sikemux: None,
            session: committer.session,
            chat: committer.chat,
        };
        std::iter::once(self).chain(with).map(one).collect()
    }

    /// The note as signed for the commit `made_of`.
    pub fn signed(&self, made_of: &Commit, key: &iroh::SecretKey) -> Option<String> {
        let text = serde_json::to_string(self).ok()?;
        Some(trailers::signed_note(&text, made_of, key))
    }
}

pub fn record(core: &Core, dir: &Path, commit: &str, agent: &Agent) {
    if let Some(key) = core.remote().host_key() {
        record_with(dir, commit, agent, &key);
    }
}

/// Keeps `agent` as who made `commit`, signed with the host's `key`.
pub fn record_with(dir: &Path, commit: &str, agent: &Agent, key: &iroh::SecretKey) {
    if let Some(committer) = Committer::of_agents(std::slice::from_ref(agent)) {
        put(dir, commit, &committer, key);
    }
}

pub fn of(core: &Core, dir: &Path, commit: &str) -> Option<Committer> {
    read(dir, commit, &core.remote().host_keys())
}

/// The commits that have a note of who made them, whoever signed it.
pub fn noted(dir: &Path) -> std::collections::HashSet<String> {
    let notes = format!("--ref={NOTES_REF}");
    let listed = git(dir, &["notes", &notes, "list"]).unwrap_or_default();
    (listed.lines())
        .filter_map(|line| line.split_whitespace().nth(1))
        .map(str::to_owned)
        .collect()
}

/// A commit a note says an agent made.
pub struct Made {
    pub sha: String,
    /// Seconds since 1970.
    pub at: u64,
    pub subject: String,
    pub by: Committer,
}

/// The commits on any branch made since `since`, in seconds, that a note one of `hosts`
/// signed says an agent made, newest first.
pub fn made_since(dir: &Path, since: u64, hosts: &[iroh::PublicKey]) -> Vec<Made> {
    let has_note = noted(dir);
    if has_note.is_empty() {
        return Vec::new();
    }
    let since = format!("--since={since}");
    let args = [
        "log",
        "--all",
        since.as_str(),
        "--max-count=400",
        "--format=%H%x1f%ct%x1f%s",
    ];
    let log = git(dir, &args).unwrap_or_default();
    (log.lines())
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\x1f');
            let (sha, at, subject) = (parts.next()?, parts.next()?, parts.next()?);
            if !has_note.contains(sha) {
                return None;
            }
            Some(Made {
                sha: sha.to_owned(),
                at: at.parse().unwrap_or(0),
                subject: subject.to_owned(),
                by: read(dir, sha, hosts)?,
            })
        })
        .collect()
}

fn put(dir: &Path, commit: &str, committer: &Committer, key: &iroh::SecretKey) -> Option<()> {
    let (tree, parents) = made_of(dir, commit)?;
    let made_of = Commit {
        tree: &tree,
        parents: &parents,
    };
    let signed = committer.signed(&made_of, key)?;
    let notes = format!("--ref={NOTES_REF}");
    // Another commit may be putting a note on at the same moment, holding the
    // lock on the notes.
    for _ in 0..NOTE_ATTEMPTS {
        let args = ["notes", notes.as_str(), "add", "-f", "-F", "-", commit];
        if run(dir, &args, None, Some(signed.as_bytes())).is_ok() {
            return Some(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

fn read(dir: &Path, commit: &str, hosts: &[iroh::PublicKey]) -> Option<Committer> {
    let notes = format!("--ref={NOTES_REF}");
    let blob = git(dir, &["notes", &notes, "show", commit]).ok()?;
    let (tree, parents) = made_of(dir, commit)?;
    let made_of = Commit {
        tree: &tree,
        parents: &parents,
    };
    let text = trailers::verify_note(&blob, &made_of, hosts)?;
    serde_json::from_str(text).ok()
}

fn made_of(dir: &Path, commit: &str) -> Option<(String, Vec<String>)> {
    let listed = git(dir, &["log", "-1", "--format=%T %P", commit]).ok()?;
    let mut ids = listed.split_whitespace().map(str::to_owned);
    Some((ids.next()?, ids.collect()))
}

#[cfg(test)]
mod tests {
    use super::super::push::{replay, Signing, Unpushed};
    use super::*;
    use crate::server::edits::tests_git::git_in;

    fn commit(dir: &Path, file: &str) -> String {
        std::fs::write(dir.join(file), file).unwrap();
        git_in(dir, &["add", file]);
        git_in(dir, &["commit", "-qm", file]);
        git_in(dir, &["rev-parse", "HEAD"])
    }

    /// With attribution off on 2026-11-23, goal summaries found a goal's commits only by their
    /// footers, so commits naming no agent never showed.
    #[test]
    fn a_commit_naming_no_agent_is_found_by_the_note_of_who_made_it() {
        let root = tempfile::tempdir().unwrap();
        let dir = sikemux_process::files::real_path(root.path()).unwrap();
        git_in(&dir, &["init", "-q", "-b", "main"]);
        git_in(&dir, &["config", "user.name", "t"]);
        git_in(&dir, &["config", "user.email", "t@t"]);
        git_in(&dir, &["config", "sikemux.attribution", "false"]);
        commit(&dir, "base.txt");
        let mine = commit(&dir, "mine.txt");
        let key = iroh::SecretKey::generate();
        let agent = |name: &str| Agent {
            name: name.to_owned(),
            cli: "claude".to_owned(),
            cli_version: None,
            model: None,
            sikemux: None,
            session: Some(format!("{name}-session")),
            chat: format!("{name}-chat"),
        };
        let made_by = Committer::of_agents(&[agent("Otter"), agent("Heron")]).unwrap();
        put(&dir, &mine, &made_by, &key).unwrap();

        let found = made_since(&dir, 1_000_000_000, &[key.public()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].sha, mine);
        assert_eq!(found[0].subject, "mine.txt");
        let names: Vec<String> = (found.into_iter().next().unwrap().by.agents().into_iter())
            .map(|agent| agent.chat)
            .collect();
        assert_eq!(names, ["Otter-chat", "Heron-chat"]);
        let stranger = [iroh::SecretKey::generate().public()];
        assert!(made_since(&dir, 1_000_000_000, &stranger).is_empty());
    }

    /// Repositories with `sikemux.attribution false` could never push through
    /// `git_commit`: without a footer, every commit read as no agent's.
    #[test]
    fn a_commit_without_a_footer_still_tells_who_made_it_after_being_made_again() {
        let root = tempfile::tempdir().unwrap();
        let dir = sikemux_process::files::real_path(root.path()).unwrap();
        git_in(&dir, &["init", "-q", "-b", "main"]);
        git_in(&dir, &["config", "user.name", "t"]);
        git_in(&dir, &["config", "user.email", "t@t"]);
        let base = commit(&dir, "base.txt");
        let theirs = commit(&dir, "theirs.txt");
        let mine = commit(&dir, "mine.txt");
        let key = iroh::SecretKey::generate();
        let hosts = [key.public()];
        let committer = Committer {
            name: "Otter".to_owned(),
            session: Some("session".to_owned()),
            chat: "chat".to_owned(),
            cli: "claude".to_owned(),
            model: None,
            with: Vec::new(),
        };
        put(&dir, &mine, &committer, &key).unwrap();

        let found = read(&dir, &mine, &hosts).unwrap();
        assert_eq!(found.chat, "chat");
        assert_eq!(found.session.as_deref(), Some("session"));
        assert!(read(&dir, &theirs, &hosts).is_none());
        let stranger = [iroh::SecretKey::generate().public()];
        assert!(read(&dir, &mine, &stranger).is_none());

        let unpushed = Unpushed {
            id: mine,
            mine: true,
            by: String::new(),
            subject: String::new(),
            waits: false,
        };
        let signing = Signing {
            key: Some(key),
            hosts: hosts.to_vec(),
        };
        let made = replay(&signing, &dir, &base, &[&unpushed], "here").unwrap();
        assert_eq!(read(&dir, &made, &hosts).unwrap().chat, "chat");
    }
}
