//! How an agent's own `git commit` or `git merge`, and a commit the core runs
//! for the git panel or Auto, gets the footer naming the agents whose lines it
//! holds. It runs with hooks of ours in front of the repository's: they run
//! its hooks as before, then ask the core for the footer once git has settled
//! what the commit holds, and add it to the message. What the commit holds is
//! git's business alone.
//!
//! The footer goes in at `prepare-commit-msg`, which runs even with
//! `--no-verify`, or at `commit-msg` when the message had nothing to put it
//! under before the editor ran. A commit holding lines of several writers
//! gets the note saying whose they are once it is made, at `post-commit`, or
//! at `post-rewrite` for an amend, after git copied the replaced commit's.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sikemux_process::vars;

use crate::trailers::{self, PendingNote};

/// The name this binary is linked under for the hooks to run it.
pub(super) const LINK: &str = "sikemux-commit-footer";
/// The folder of the hooks, which the core hands agents.
pub(super) const HOOKS: &str = "SIKEMUX_AGENT_HOOKS";
/// Where the repository's own hooks are, for ours to run them.
const REPO_HOOKS: &str = "SIKEMUX_REPO_HOOKS";
/// How many `GIT_CONFIG_COUNT` settings there were before ours, for our hooks
/// to put back so no git their hooks run takes our hooks too.
const CONFIG_COUNT: &str = "SIKEMUX_CONFIG_COUNT";
const AMEND: &str = "SIKEMUX_COMMIT_AMEND";
/// Where the note for the commit being made waits for it to be made.
const NOTE_FILE: &str = "SIKEMUX_NOTE_FILE";
const OURS: [&str; 5] = [
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "post-merge",
    "post-rewrite",
];
const ANSWER_WAIT: Duration = Duration::from_secs(3);

pub(super) fn folder(data_dir: &Path) -> PathBuf {
    data_dir.join("agent-hooks")
}

/// Has `command`, the real git about to run `git <options> commit <args>` or
/// `merge <args>`, run our hooks, linked in `folder`, in front of the
/// repository's.
pub(super) fn prepare(
    command: &mut Command,
    folder: &Path,
    git: &Path,
    options: &[String],
    args: &[String],
) {
    let Some((theirs, note_file)) = repo_paths(git, options) else {
        return;
    };
    let Ok(ours) = hooks_for(folder, &theirs) else {
        return;
    };
    let amend = args
        .iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--amend");
    let count = config_count();
    rewrite_notes(command);
    command
        .env("GIT_CONFIG_COUNT", (count + 2).to_string())
        .env(format!("GIT_CONFIG_KEY_{}", count + 1), "core.hooksPath")
        .env(format!("GIT_CONFIG_VALUE_{}", count + 1), ours)
        .env(CONFIG_COUNT, count.to_string())
        .env(REPO_HOOKS, theirs)
        .env(NOTE_FILE, note_file);
    if amend {
        command.env(AMEND, "1");
    } else {
        command.env_remove(AMEND);
    }
}

/// Has `command`, a git that may amend or rebase commits, carry their notes
/// saying whose lines they hold over to the commits that replace them.
pub(super) fn rewrite_notes(command: &mut Command) {
    let count = config_count();
    command
        .env("GIT_CONFIG_COUNT", (count + 1).to_string())
        .env(format!("GIT_CONFIG_KEY_{count}"), "notes.rewriteRef")
        .env(format!("GIT_CONFIG_VALUE_{count}"), trailers::NOTES_REF);
}

fn config_count() -> usize {
    std::env::var("GIT_CONFIG_COUNT")
        .ok()
        .and_then(|count| count.parse().ok())
        .unwrap_or(0)
}

/// The repository's hooks folder, and where a commit's note waits for it.
fn repo_paths(git: &Path, options: &[String]) -> Option<(PathBuf, PathBuf)> {
    let output = sikemux_process::user_environment::command(git)
        .args(options)
        .args(["rev-parse", "--path-format=absolute", "--git-path", "hooks"])
        .args(["--git-path", "sikemux-agents-note"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let paths = String::from_utf8(output.stdout).ok()?;
    let mut paths = paths.lines().map(str::trim).filter(|path| !path.is_empty());
    let (hooks, note) = (paths.next()?, paths.next()?);
    output
        .status
        .success()
        .then(|| (PathBuf::from(hooks), PathBuf::from(note)))
}

/// A folder of hooks that stand in for each of the repository's in `theirs`,
/// plus ours. Git runs every hook a folder has, so each set of hooks gets a
/// folder of its own, named for the set and for what the hooks say.
fn hooks_for(folder: &Path, theirs: &Path) -> std::io::Result<PathBuf> {
    let mut names: Vec<String> = std::fs::read_dir(theirs)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            std::fs::metadata(entry.path()).is_ok_and(|meta| {
                meta.is_file() && sikemux_process::os::is_executable(&entry.path())
            })
        })
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.contains('.'))
        .chain(OURS.map(str::to_owned))
        .collect();
    names.sort();
    names.dedup();
    let script = stand_in(&folder.join(LINK));
    let set = Sha256::new()
        .chain_update(names.join("\n"))
        .chain_update(&script)
        .finalize();
    let set = hex::encode(&set[..6]);
    let hooks = folder.join(set);
    if hooks.is_dir() {
        return Ok(hooks);
    }
    let fresh = folder.join(format!(".{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&fresh)?;
    for name in &names {
        let path = fresh.join(name);
        std::fs::write(&path, &script)?;
        sikemux_process::os::make_executable(&path)?;
    }
    if std::fs::rename(&fresh, &hooks).is_err() {
        let _ = std::fs::remove_dir_all(&fresh);
    }
    Ok(hooks)
}

fn stand_in(footer: &Path) -> String {
    let footer = footer.to_string_lossy().replace('\'', "'\\''");
    format!(
        r#"#!/bin/sh
name=${{0##*/}}
hook="$SIKEMUX_REPO_HOOKS/$name"
note=$SIKEMUX_NOTE_FILE
if [ "${{SIKEMUX_CONFIG_COUNT:-0}}" = 0 ]; then unset GIT_CONFIG_COUNT; else export GIT_CONFIG_COUNT="$SIKEMUX_CONFIG_COUNT"; fi
unset SIKEMUX_REPO_HOOKS SIKEMUX_CONFIG_COUNT SIKEMUX_NOTE_FILE
case $name in
prepare-commit-msg|commit-msg)
	if [ -f "$hook" ] && [ -x "$hook" ]; then "$hook" "$@" || exit; fi
	if [ "$name" = commit-msg ] && grep -q '^Agent-Signature: ' "$1"; then exit 0; fi
	SIKEMUX_NOTE_FILE=$note exec '{footer}' "$name" "$@" ;;
post-commit|post-merge|post-rewrite)
	if [ -n "$note" ] && [ -f "$note" ]; then SIKEMUX_NOTE_FILE=$note '{footer}' "$name" "$@" </dev/null; fi ;;
esac
[ -x "$hook" ] || exit 0
exec "$hook" "$@"
"#
    )
}

/// Run by our hooks with the hook's name and its arguments: puts the footer in
/// the message and the note on the commit made.
pub(super) fn run(args: Vec<OsString>) -> i32 {
    let note_file = std::env::var_os(NOTE_FILE)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    match args.as_slice() {
        [hook, file, ..] if hook == "prepare-commit-msg" || hook == "commit-msg" => {
            sign(hook, file, note_file.as_deref())
        }
        [hook, ..] if hook == "post-commit" || hook == "post-merge" => {
            if std::env::var_os(AMEND).is_none() {
                note_file.as_deref().map(put_note);
            }
        }
        [hook, kind, ..] if hook == "post-rewrite" && kind == "amend" => {
            note_file.as_deref().map(put_note);
        }
        _ => {}
    }
    0
}

/// Puts the footer in the message `file`, and keeps the note for the commit
/// in `note_file`. A commit goes on without them when the core cannot be asked
/// or the committer is no agent of its, and the core is asked once per commit
/// when it gives none.
fn sign(hook: &OsString, file: &OsString, note_file: Option<&Path>) {
    let mut unanswered = file.clone();
    unanswered.push(".sikemux-unanswered");
    let git = sikemux_process::os::parent_id().to_string();
    if hook == "commit-msg" {
        let asked = std::fs::read_to_string(&unanswered).is_ok_and(|pid| pid == git);
        let _ = std::fs::remove_file(&unanswered);
        if asked {
            return;
        }
    } else if let Some(note_file) = note_file {
        let _ = std::fs::remove_file(note_file);
    }
    let Ok(text) = std::fs::read_to_string(file) else {
        return;
    };
    let empty = trailers::into_message_file(&text, &[]).is_none();
    if empty || (hook == "commit-msg" && trailers::is_signed(&text)) {
        return;
    }
    let Some(Answer { footer, note }) = ask() else {
        if hook == "prepare-commit-msg" {
            let _ = std::fs::write(&unanswered, git);
        }
        return;
    };
    if let Some(signed) = trailers::into_message_file(&text, &footer) {
        let _ = std::fs::write(file, signed);
    }
    if let (Some(note_file), Some(note)) = (note_file, note) {
        if let Ok(note) = serde_json::to_vec(&note) {
            let _ = std::fs::write(note_file, note);
        }
    }
}

/// Puts the note waiting in `note_file` on `HEAD`, or takes `HEAD`'s away
/// when it waits with none, if `HEAD` is the commit it was worked out for.
fn put_note(note_file: &Path) {
    let pending = std::fs::read(note_file);
    let _ = std::fs::remove_file(note_file);
    let Ok(pending) = serde_json::from_slice::<PendingNote>(&pending.unwrap_or_default()) else {
        return;
    };
    let git = || {
        let mut git = sikemux_process::user_environment::command(
            std::env::var_os(vars::REAL_GIT).unwrap_or_else(|| OsString::from("git")),
        );
        git.args(["-c", "core.fsmonitor=false"]);
        git
    };
    let Ok(head) = git().args(["cat-file", "commit", "HEAD"]).output() else {
        return;
    };
    let head = String::from_utf8_lossy(&head.stdout);
    let header = head.split("\n\n").next().unwrap_or_default();
    let tree = header.lines().find_map(|line| line.strip_prefix("tree "));
    let parents: Vec<&str> = header
        .lines()
        .filter_map(|line| line.strip_prefix("parent "))
        .collect();
    if tree != Some(pending.tree.as_str()) || parents != pending.parents {
        return;
    }
    let notes = format!("--ref={}", trailers::NOTES_REF);
    let put = || match &pending.note {
        Some(note) => git()
            .args(["notes", &notes, "add", "-f", "-F", "-", "HEAD"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| {
                if let Some(mut stdin) = child.stdin.take() {
                    stdin.write_all(note.as_bytes())?;
                }
                child.wait()
            }),
        None => git()
            .args(["notes", &notes, "remove", "--ignore-missing", "HEAD"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status(),
    };
    // Another commit's hooks may be putting a note on at the same moment,
    // holding the lock on the notes.
    for _ in 0..NOTE_ATTEMPTS {
        if put().is_ok_and(|status| status.success()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

const NOTE_ATTEMPTS: usize = 3;

#[derive(Deserialize)]
struct Answer {
    footer: Vec<String>,
    note: Option<PendingNote>,
}

/// Where agents reach the core that started them.
pub(super) fn endpoint() -> Option<PathBuf> {
    sikemux_process::paths::handed_cli_endpoint()
        .or_else(crate::cli::endpoint::default_endpoint_path)
}

fn ask() -> Option<Answer> {
    let endpoint = endpoint()?;
    let top = std::env::current_dir().ok()?;
    let index = std::env::var_os("GIT_INDEX_FILE").map(|index| top.join(index));
    let params = json!({
        "index": index,
        "amend": std::env::var_os(AMEND).is_some(),
    });
    let answer = crate::cli::client::attempt(
        &endpoint,
        &top.to_string_lossy(),
        None,
        crate::cli::protocol::COMMIT_FOOTER_METHOD,
        &params,
        ANSWER_WAIT,
    )
    .ok()?;
    serde_json::from_value::<Answer>(answer)
        .ok()
        .filter(|answer| !answer.footer.is_empty())
}
