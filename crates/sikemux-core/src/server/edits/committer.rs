//! Who made a commit in a repository whose commits name no agent, so an agent
//! can still push its own: a signed git note under [`NOTES_REF`] that stays on
//! this computer, since pushing a branch does not push notes.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::owners::{git, run};
use crate::server::Core;
use crate::trailers::{self, Agent, Commit, COMMITTERS_REF as NOTES_REF};

const NOTE_ATTEMPTS: usize = 3;

#[derive(Deserialize, Serialize)]
pub(super) struct Committer {
    pub name: String,
    pub session: Option<String>,
    pub chat: String,
}

pub(super) fn record(core: &Core, dir: &Path, commit: &str, agent: &Agent) {
    let Some(key) = core.remote.host_key() else {
        return;
    };
    let committer = Committer {
        name: agent.name.clone(),
        session: agent.session.clone(),
        chat: agent.chat.clone(),
    };
    put(dir, commit, &committer, &key);
}

pub(super) fn of(core: &Core, dir: &Path, commit: &str) -> Option<Committer> {
    read(dir, commit, &core.remote.host_keys())
}

fn put(dir: &Path, commit: &str, committer: &Committer, key: &iroh::SecretKey) -> Option<()> {
    let text = serde_json::to_string(committer).ok()?;
    let (tree, parents) = made_of(dir, commit)?;
    let made_of = Commit {
        tree: &tree,
        parents: &parents,
    };
    let signed = trailers::signed_note(&text, &made_of, key);
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

    /// Repositories with `sikemux.attribution false` could never push through
    /// `git_commit`: without a footer, every commit read as no agent's.
    #[test]
    fn a_commit_without_a_footer_still_tells_who_made_it_after_being_made_again() {
        let root = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(root.path()).unwrap();
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
        };
        let signing = Signing {
            key: Some(key),
            hosts: hosts.to_vec(),
        };
        let made = replay(&signing, &dir, &base, &[&unpushed], "here").unwrap();
        assert_eq!(read(&dir, &made, &hosts).unwrap().chat, "chat");
    }
}
