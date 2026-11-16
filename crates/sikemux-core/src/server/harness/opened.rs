//! What an agent opened for the person with `ui_open`, kept as it stood then for the calls it
//! raises next.

use std::io::{BufRead, BufReader};
use std::path::Path;

use sikemux_git::status::{git_status, GitChange};
use sikemux_view::calls::{CallChange, CallChangedFile, CallOpened};

use crate::server::connection::blocking;
use crate::server::Core;

const EXCERPT_LINES: usize = 12;
const LINES_ABOVE: u32 = 3;
const MOST_CHARS: usize = 200;
const MOST_FILES: usize = 40;

pub(super) async fn keep(
    core: &Core,
    agent: &str,
    kind: &str,
    path: Option<String>,
    line: Option<u32>,
    checkout: &str,
) {
    let opened = match kind {
        "file" => {
            let Some(path) = path else { return };
            match blocking(move || Ok(excerpt(&path, line))).await {
                Ok(Some(opened)) => opened,
                _ => return,
            }
        }
        "diff" => match git_status(checkout.to_owned()).await {
            Ok(status) => CallOpened::Diff {
                checkout: checkout.to_owned(),
                files: (status.files.into_iter().take(MOST_FILES))
                    .map(|file| CallChangedFile {
                        change: change_of(match file.worktree {
                            GitChange::Unchanged => file.index,
                            change => change,
                        }),
                        path: file.path,
                    })
                    .collect(),
            },
            Err(_) => return,
        },
        _ => return,
    };
    crate::server::chief::opened(core, agent, opened);
}

fn excerpt(path: &str, line: Option<u32>) -> Option<CallOpened> {
    let file = std::fs::File::open(Path::new(path)).ok()?;
    let first = line.map_or(1, |line| line.saturating_sub(LINES_ABOVE).max(1));
    let lines: Vec<String> = BufReader::new(file)
        .lines()
        .skip(first as usize - 1)
        .take(EXCERPT_LINES)
        .collect::<Result<_, _>>()
        .ok()?;
    let excerpt = (lines.iter())
        .map(|line| line.chars().take(MOST_CHARS).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    Some(CallOpened::File {
        path: path.to_owned(),
        line,
        first,
        excerpt,
    })
}

fn change_of(change: GitChange) -> CallChange {
    match change {
        GitChange::Added | GitChange::Untracked | GitChange::Copied => CallChange::Added,
        GitChange::Deleted => CallChange::Deleted,
        GitChange::Renamed => CallChange::Renamed,
        _ => CallChange::Modified,
    }
}
