//! Git commands an agent may not run in a checkout other agents share, since
//! each would take in or throw away their uncommitted work along with its own.
//! In a clone or linked worktree of its own an agent is alone, and may run them.

mod footer;
mod heavy;
pub mod homes;
pub mod shim;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The other agents whose work a git command run in a folder would touch:
/// their uncommitted lines in the paths given there, or with none given,
/// anywhere in the checkout, and the agents working in it. None when the core
/// cannot say.
pub(crate) type Others<'a> = dyn Fn(&Path, &[String]) -> Option<Vec<String>> + 'a;

const COMMIT_INSTEAD: &str = "Commit with the git_commit tool: it takes the lines you wrote, and with `paths`, only those files or folders, with every change there no other agent wrote, such as files your own scripts made.";

/// What a command does to a whole checkout, and what to run instead.
struct Sweep {
    does: &'static str,
    instead: &'static str,
}

/// Why `command`, a shell command line run in `cwd`, may not run, if it may not.
pub fn refusal(command: &str, cwd: &Path, others: &Others) -> Option<String> {
    segments(command).iter().find_map(|words| {
        let words = command_words(words)?;
        if let Some(script) = shell_script(words) {
            return refusal(script, cwd, others);
        }
        let (first, args) = words.split_first()?;
        if base_name(first) != "git" {
            return None;
        }
        let git = || crate::git::git_command(cwd);
        refused(args, false, &git, others)
    })
}

/// The script of `sh -c script`, or of a shell given other options first.
pub(crate) fn shell_script(words: &[String]) -> Option<&str> {
    let (shell, rest) = words.split_first()?;
    if !is_shell(shell) {
        return None;
    }
    let mut words = rest.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "-o" | "+o" => {
                words.next();
            }
            flag if flag.starts_with('-') && !flag.starts_with("--") && flag.contains('c') => {
                return words.next().map(String::as_str);
            }
            flag if flag.starts_with(['-', '+']) => {}
            _ => return None,
        }
    }
    None
}

/// Why `git` with `args` may not run, if it may not. `git` starts the real
/// git where the command runs, to find the repository with.
/// `private_index` says the command stages into an index of its own rather
/// than the checkout's.
fn refused(
    args: &[String],
    private_index: bool,
    git: &dyn Fn() -> Command,
    others: &Others,
) -> Option<String> {
    let subcommand = git_args(args);
    let touched = touches(subcommand, private_index)?;
    if matches!(&touched, Touch::Paths { paths, .. } | Touch::Stages(paths) if paths.is_empty()) {
        return None;
    }
    let options = args
        .get(..args.len() - subcommand.len())
        .unwrap_or_default();
    let checkout = checkout(git(), options);
    if checkout.as_ref().is_some_and(|checkout| checkout.linked) {
        return None;
    }
    let shown = format!("git {}", subcommand.join(" "));
    match touched {
        Touch::Stages(paths) => {
            let checkout = checkout?;
            let names = others(&checkout.dir, &paths).filter(|names| !names.is_empty())?;
            let names = listed(&names);
            Some(format!(
                "Sikemux refused `{shown}`: it takes in uncommitted lines {names} wrote there along with yours. Commit with the git_commit tool and `paths`, which takes your lines there and changes no agent wrote, or ask {names} with agent_send."
            ))
        }
        Touch::Everything(sweep) => {
            // `git status` does not list the ignored files `git clean -x` deletes.
            let is_clean = subcommand.first().is_some_and(|sub| sub == "clean");
            if !is_clean && nothing_uncommitted(git(), options) {
                return None;
            }
            let who = match checkout.and_then(|checkout| others(&checkout.dir, &[])) {
                Some(names) if names.is_empty() => return None,
                Some(names) => {
                    let verb = if names.len() == 1 { "works" } else { "work" };
                    format!("{} also {verb} in this checkout", listed(&names))
                }
                None => "Sikemux could not confirm in time that no other agent works in this checkout, as happens when the machine is very busy. Run it again in a moment".to_owned(),
            };
            let Sweep { does, instead } = sweep;
            Some(format!(
                "Sikemux refused `{shown}`: it {does}, and {who}. {instead} In a clone or git worktree of your own this command is allowed."
            ))
        }
        Touch::Paths { mut paths, named } => {
            let checkout = checkout?;
            if !named {
                if is_commit(git(), options, &paths[0]) {
                    paths.remove(0);
                }
                paths.retain(|path| checkout.dir.join(path).symlink_metadata().is_ok());
                if paths.is_empty() {
                    return None;
                }
            }
            let names = others(&checkout.dir, &paths).filter(|names| !names.is_empty())?;
            let names = listed(&names);
            Some(format!(
                "Sikemux refused `{shown}`: it throws away uncommitted lines {names} wrote there. Ask {names} with agent_send first, or undo only your own lines by editing them."
            ))
        }
    }
}

/// Whether git, given the global `options`, finds nothing uncommitted anywhere
/// in the checkout.
fn nothing_uncommitted(mut git: Command, options: &[String]) -> bool {
    git.args(options)
        .args(["status", "--porcelain"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|output| output.status.success() && output.stdout.is_empty())
}

/// Whether git, given the global `options`, reads `word` as a commit, as
/// `git checkout` does with its first word before taking it for a path.
fn is_commit(mut git: Command, options: &[String], word: &str) -> bool {
    git.args(options)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
        .arg(format!("{word}^{{commit}}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The checkout git runs in, given the global `options`: the folder it runs
/// in, and whether it is a linked worktree, with its own index and files,
/// rather than a repository's main checkout. None when git cannot say.
struct Checkout {
    dir: PathBuf,
    linked: bool,
}

fn checkout(mut git: Command, options: &[String]) -> Option<Checkout> {
    let output = git
        .args(options)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
            "--show-toplevel",
            "--show-prefix",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    let (own, common, top) = (lines.next()?, lines.next()?, lines.next()?);
    Some(Checkout {
        dir: Path::new(top).join(lines.next().unwrap_or_default()),
        linked: own != common,
    })
}

enum Touch {
    /// The whole checkout, and what the command does to it.
    Everything(Sweep),
    /// Files whose uncommitted changes the command throws away. Unless
    /// `named`, they are `git checkout`'s words, the first of which may be a
    /// commit rather than a path.
    Paths { paths: Vec<String>, named: bool },
    /// Files whose whole uncommitted changes the command stages or commits.
    Stages(Vec<String>),
}

fn touches(args: &[String], private_index: bool) -> Option<Touch> {
    if let Some(sweep) = sweeping(args, private_index) {
        return Some(Touch::Everything(sweep));
    }
    let (sub, rest) = args.split_first()?;
    let valued: &[&str] = match sub.as_str() {
        "add" if !private_index => return Some(Touch::Stages(operands(rest, "", &[]))),
        "commit" if !private_index => {
            return Some(Touch::Stages(operands(rest, "mFCct", COMMIT_VALUED)));
        }
        "apply" if reverses(rest) => {
            return Some(Touch::Paths {
                paths: patched(&operands(rest, "pC", APPLY_VALUED)),
                named: true,
            });
        }
        "checkout" => &["-b", "-B", "--orphan"],
        "restore" => {
            let has = |short: char, long: &str| {
                flags_before_paths(rest).any(|flag| {
                    flag == long
                        || flag
                            .strip_prefix('-')
                            .filter(|letters| !letters.starts_with('-'))
                            .is_some_and(|letters| letters.contains(short))
                })
            };
            if has('S', "--staged") && !has('W', "--worktree") {
                return None;
            }
            &["-s", "--source"]
        }
        _ => return None,
    };
    let (before, after) = match rest.iter().position(|arg| arg == "--") {
        Some(at) => (&rest[..at], Some(&rest[at + 1..])),
        None => (rest, None),
    };
    let named = after.is_some() || sub == "restore";
    let mut paths: Vec<String> = after.into_iter().flatten().cloned().collect();
    if after.is_none() || sub == "restore" {
        let mut words = before.iter();
        while let Some(word) = words.next() {
            if valued.contains(&word.as_str()) {
                words.next();
            } else if !word.starts_with('-') {
                paths.push(word.clone());
            }
        }
    }
    Some(Touch::Paths { paths, named })
}

const COMMIT_VALUED: &[&str] = &[
    "--message",
    "--file",
    "--reuse-message",
    "--reedit-message",
    "--template",
    "--author",
    "--date",
    "--fixup",
    "--squash",
    "--cleanup",
    "--trailer",
];

const APPLY_VALUED: &[&str] = &["--exclude", "--include", "--directory"];

/// The words of a command's `args` that are not options or their values: its
/// paths. `letters` are the short options that take a value, and `valued`
/// the long ones given it as the next word.
fn operands(args: &[String], letters: &str, valued: &[&str]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if word == "--" {
            paths.extend(words.by_ref().cloned());
            break;
        }
        if valued.contains(&word.as_str()) {
            words.next();
        } else if let Some(short) = word.strip_prefix('-').filter(|rest| !rest.starts_with('-')) {
            let takes = short
                .char_indices()
                .find(|(_, letter)| letters.contains(*letter))
                .is_some_and(|(at, _)| at + 1 == short.len());
            if takes {
                words.next();
            }
        } else if !word.starts_with('-') {
            paths.push(word.clone());
        }
    }
    paths
}

/// Whether `git apply` with `rest` takes a patch back out of the files on disk.
fn reverses(rest: &[String]) -> bool {
    let flags: Vec<&str> = flags_before_paths(rest).collect();
    let reverse = flags
        .iter()
        .any(|flag| *flag == "--reverse" || (flag.starts_with("-R") && !flag.starts_with("--")));
    reverse
        && !flags
            .iter()
            .any(|flag| matches!(*flag, "--cached" | "--check" | "--stat"))
}

/// The files the patches at `patches` change, read from them. A patch read
/// from standard input is not looked at.
fn patched(patches: &[String]) -> Vec<String> {
    let mut paths = Vec::new();
    for patch in patches.iter().filter(|patch| *patch != "-") {
        let Ok(text) = std::fs::read_to_string(patch) else {
            continue;
        };
        for line in text.lines() {
            let named = line
                .strip_prefix("+++ b/")
                .or_else(|| line.strip_prefix("--- a/"));
            if let Some(path) = named.filter(|path| !paths.iter().any(|seen| seen == path)) {
                paths.push(path.to_owned());
            }
        }
    }
    paths
}

/// `names` as one would say them: "Otter", "Otter and Briar", "Otter, Briar and Moss".
fn listed(names: &[String]) -> String {
    match names.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

fn sweeping(args: &[String], private_index: bool) -> Option<Sweep> {
    const STAGES: Sweep = Sweep {
        does: "stages every change in the checkout",
        instead: COMMIT_INSTEAD,
    };
    const UNDOES: &str = "throws away uncommitted changes";
    let (sub, rest) = args.split_first()?;
    let everything = |paths: &[String]| {
        paths
            .iter()
            .any(|path| matches!(path.as_str(), "." | "./" | ":/" | ":" | "*" | ":(top)"))
    };
    let short = |letter: char, valued: &str| {
        flags_before_paths(rest).any(|flag| {
            flag.strip_prefix('-')
                .filter(|letters| !letters.starts_with('-'))
                .is_some_and(|letters| {
                    letters
                        .chars()
                        .take_while(|c| !valued.contains(*c))
                        .any(|c| c == letter)
                })
        })
    };
    let long = |name: &str| flags_before_paths(rest).any(|flag| flag == name);
    match sub.as_str() {
        "add" if private_index => None,
        "add" if short('A', "") || short('u', "") || long("--all") || long("--update") => {
            Some(STAGES)
        }
        "add" if everything(rest) => Some(STAGES),
        "commit" if short('a', "mFCctS") || long("--all") => Some(Sweep {
            does: "commits every change in the checkout",
            instead: COMMIT_INSTEAD,
        }),
        "stash" if !matches!(rest.first().map(String::as_str), Some("list" | "show")) => {
            Some(Sweep {
                does: "takes every uncommitted change out of the checkout",
                instead: COMMIT_INSTEAD,
            })
        }
        "reset" if long("--hard") || long("--merge") => Some(Sweep {
            does: UNDOES,
            instead: "To move the branch, `git reset --keep` leaves uncommitted changes alone and stops if one is in the way.",
        }),
        "checkout" | "restore" if everything(rest) => Some(Sweep {
            does: UNDOES,
            instead: "Undo only your own lines by editing them.",
        }),
        "clean" if short('f', "eE") || long("--force") => Some(Sweep {
            does: "deletes files nobody committed",
            instead: "Delete the files you made by name.",
        }),
        _ => None,
    }
}

fn flags_before_paths(args: &[String]) -> impl Iterator<Item = &str> {
    args.iter()
        .map(String::as_str)
        .take_while(|arg| *arg != "--")
        .filter(|arg| arg.starts_with('-'))
}

/// `git`'s arguments from its subcommand on, past options such as `-C dir`.
pub(crate) fn git_args(args: &[String]) -> &[String] {
    let mut at = 0;
    while let Some(arg) = args.get(at) {
        match arg.as_str() {
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" => at += 2,
            arg if arg.starts_with('-') => at += 1,
            _ => break,
        }
    }
    args.get(at..).unwrap_or_default()
}

/// A command's words without what runs it, such as `env X=1` or `sudo`.
pub(crate) fn command_words(words: &[String]) -> Option<&[String]> {
    let runner = |word: &str| {
        matches!(
            word,
            "env" | "command" | "exec" | "sudo" | "time" | "nice" | "nohup" | "builtin"
        ) || word.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    };
    let at = words.iter().position(|word| !runner(word))?;
    Some(&words[at..])
}

pub(crate) fn base_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn is_shell(word: &str) -> bool {
    matches!(base_name(word), "sh" | "bash" | "zsh" | "dash")
}

/// The words of each simple command in `line`, split at `;`, `&`, `|`,
/// brackets, backticks and line breaks outside quotes.
pub fn segments(line: &str) -> Vec<Vec<String>> {
    let mut segments = vec![Vec::new()];
    let mut word: Option<String> = None;
    let mut chars = line.chars();
    let end_word = |word: &mut Option<String>, segments: &mut Vec<Vec<String>>| {
        if let Some(done) = word.take() {
            if let Some(last) = segments.last_mut() {
                last.push(done);
            }
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                let quoted: String = chars.by_ref().take_while(|c| *c != '\'').collect();
                word.get_or_insert_with(String::new).push_str(&quoted);
            }
            '"' => {
                let text = word.get_or_insert_with(String::new);
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => text.extend(chars.next()),
                        c => text.push(c),
                    }
                }
            }
            '\\' => word.get_or_insert_with(String::new).extend(chars.next()),
            ';' | '&' | '|' | '(' | ')' | '`' | '\n' | '{' | '}' => {
                end_word(&mut word, &mut segments);
                segments.push(Vec::new());
            }
            c if c.is_whitespace() => end_word(&mut word, &mut segments),
            c => word.get_or_insert_with(String::new).push(c),
        }
    }
    end_word(&mut word, &mut segments);
    segments.retain(|words| !words.is_empty());
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shared_checkout_with_nothing_uncommitted_can_be_moved() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path();
        let git = |args: &[&str]| {
            let mut git = sikemux_process::user_environment::command("git");
            git.args(["-c", "user.name=t", "-c", "user.email=t@t"]);
            assert!(git.args(args).current_dir(top).status().unwrap().success());
        };
        git(&["init", "-q"]);
        std::fs::write(top.join("store.ts"), "x\n").unwrap();
        git(&["add", "store.ts"]);
        git(&["commit", "-qm", "store"]);
        let others = |_: &Path, _: &[String]| Some(vec!["Cowslip".to_owned()]);
        let refused = |command: &str| refusal(command, top, &others).is_some();
        assert!(!refused("git reset --hard HEAD"));
        assert!(refused("git clean -fdx"));
        std::fs::write(top.join("store.ts"), "y\n").unwrap();
        assert!(refused("git reset --hard HEAD"));
        assert!(!refused("git reset --keep HEAD"));
    }

    #[test]
    fn staging_or_undoing_a_file_with_another_agents_lines_is_refused() {
        let top = tempfile::tempdir().unwrap();
        let top = top.path();
        let git = |args: &[&str]| {
            let mut git = sikemux_process::user_environment::command("git");
            assert!(git.args(args).current_dir(top).status().unwrap().success());
        };
        git(&["init", "-q"]);
        std::fs::write(top.join("store.ts"), "x\n").unwrap();
        let patch = top.join("undo.patch");
        std::fs::write(
            &patch,
            "--- a/store.ts\n+++ b/store.ts\n@@ -1 +1 @@\n-x\n+y\n",
        )
        .unwrap();
        let others = |_: &Path, paths: &[String]| {
            Some(match paths.iter().any(|path| path == "store.ts") {
                true => vec!["Cowslip".to_owned()],
                false => Vec::new(),
            })
        };
        let refused = |command: &str| refusal(command, top, &others).is_some();
        assert!(refused("git add store.ts"));
        assert!(refused("git commit -qm 'fix: store' -- store.ts"));
        assert!(refused(&format!("git apply -R {}", patch.display())));
        assert!(!refused("git commit -m store.ts"));
        assert!(!refused("git add other.ts"));
        assert!(!refused(&format!(
            "git apply -R --cached {}",
            patch.display()
        )));
    }
}
