//! The person's guardrails for agents in a project, enforced where Sikemux runs
//! or commits for an agent: commands it starts for one, and `git_commit`.

use std::path::Path;

use super::Core;

/// Whether `path`, relative to the project, matches `glob`: `**` crosses folders, `*` and `?` do not.
pub(crate) fn path_matches(glob: &str, path: &str) -> bool {
    let glob = glob.trim().trim_start_matches("./");
    let glob = glob
        .strip_suffix('/')
        .map_or_else(|| glob.to_owned(), |folder| format!("{folder}/**"));
    matches(glob.as_bytes(), path.as_bytes(), true)
}

/// Whether `command` starts with what `pattern` describes, `*` standing for any text.
pub(crate) fn command_matches(pattern: &str, command: &str) -> bool {
    let pattern = format!("{}*", pattern.trim());
    let command = command.split_whitespace().collect::<Vec<_>>().join(" ");
    matches(pattern.as_bytes(), command.as_bytes(), false)
}

fn matches(pattern: &[u8], text: &[u8], paths: bool) -> bool {
    match pattern {
        [] => text.is_empty(),
        [b'*', b'*', rest @ ..] if paths => {
            if rest.is_empty() {
                return true;
            }
            let rest = rest.strip_prefix(b"/").unwrap_or(rest);
            (0..=text.len())
                .any(|at| (at == 0 || text[at - 1] == b'/') && matches(rest, &text[at..], paths))
        }
        [b'*', rest @ ..] => (0..=text.len())
            .take_while(|at| !paths || !text[..*at].contains(&b'/'))
            .any(|at| matches(rest, &text[at..], paths)),
        [b'?', rest @ ..] => {
            text.first().is_some_and(|first| !(paths && *first == b'/'))
                && matches(rest, &text[1..], paths)
        }
        [first, rest @ ..] => text.first() == Some(first) && matches(rest, &text[1..], paths),
    }
}

/// Refuses a command an agent asked Sikemux to run in `project`, when the person said not to.
pub(crate) fn check_command(core: &Core, project: &Path, command: &str) -> Result<(), String> {
    let rules = core.workspaces.rules_for(project);
    if let Some(pattern) = rules
        .never_run
        .iter()
        .find(|pattern| command_matches(pattern, command))
    {
        return Err(format!(
            "The person set this project so agents never run `{pattern}`. Do not run it another way; tell them what you needed it for."
        ));
    }
    if let Some(pattern) = rules
        .ask_before
        .iter()
        .find(|pattern| command_matches(pattern, command))
    {
        return Err(format!(
            "The person asks to be asked before agents run `{pattern}`. Ask them with ask, saying why; if they say yes, they run it or tell you how."
        ));
    }
    Ok(())
}

/// Refuses a commit by an agent that changes a path the person protected.
pub(crate) fn check_paths(core: &Core, checkout: &Path, paths: &[String]) -> Result<(), String> {
    let rules = core.workspaces.rules_for(checkout);
    let touched: Vec<&str> = paths
        .iter()
        .filter(|path| rules.never_edit.iter().any(|glob| path_matches(glob, path)))
        .map(String::as_str)
        .collect();
    if touched.is_empty() {
        return Ok(());
    }
    Err(format!(
        "Nothing was committed: the person set this project so agents ask before changing {}. Ask them with ask, and put the change back if they say no.",
        touched.join(", ")
    ))
}
