//! What every code host works out the same way from what it fetched.

use serde_json::Value;

use crate::account::as_account;
use crate::http::{Api, Calls};
use crate::store::{Account, Accounts};
use crate::{Service, ServiceError};

use super::asks::{LinePick, PatchQuery};
use super::{ChangedFile, JobLog, LogExcerpt, LogLine, Pull, PullStage};

const PATCH_LINES: usize = 400;
const MAX_PATCH_LINES: usize = 1000;
const PATCH_BYTES: usize = 32 * 1024;
const DEFAULT_TAIL: usize = 200;
const MAX_TAIL: usize = 2000;
const MAX_CONTEXT: usize = 50;
const EXCERPT_CHARS: usize = 30_000;
const LINE_CHARS: usize = 2_000;

fn cut_at_char(text: &str, most: usize) -> &str {
    text.get(..text.floor_char_boundary(most))
        .unwrap_or_default()
}

/// Up to `lines` lines of `patch` from `offset`, and at most `PATCH_BYTES` of
/// them, with a note whenever any of the patch is left out.
fn window(patch: &str, offset: usize, lines: Option<usize>) -> (String, Option<String>) {
    let all: Vec<&str> = patch.split('\n').collect();
    let total = all.len();
    if offset >= total {
        return (
            String::new(),
            Some(format!(
                "The patch has {total} lines; offset {offset} is past its end."
            )),
        );
    }
    let most = lines.unwrap_or(PATCH_LINES).clamp(1, MAX_PATCH_LINES);
    let mut shown = String::new();
    let mut end = offset;
    let mut line_cut = false;
    for line in all.iter().skip(offset).take(most) {
        let room = PATCH_BYTES.saturating_sub(shown.len() + 1);
        if line.len() > room {
            if end == offset {
                shown.push_str(cut_at_char(line, room));
                line_cut = true;
                end += 1;
            }
            break;
        }
        if end > offset {
            shown.push('\n');
        }
        shown.push_str(line);
        end += 1;
    }
    if offset == 0 && end == total && !line_cut {
        return (shown, None);
    }
    let mut note = format!("Lines {}-{end} of {total}.", offset + 1);
    if line_cut {
        note.push_str(&format!(
            " Line {end} was cut at {} KB.",
            PATCH_BYTES / 1024
        ));
    }
    if end < total {
        note.push_str(&format!(
            " {} more lines left out: pass offset {end} for the next part, or run git diff locally.",
            total - end
        ));
    }
    (shown, Some(note))
}

fn is_named(file: &ChangedFile, path: &str) -> bool {
    file.path == path || file.previous_path.as_deref() == Some(path)
}

/// The files to answer with, carrying the patches `query` asks for. Agents get
/// the file list alone unless they name `paths`, and then a window of each
/// patch; `fullPatches` asks for all of them. `change` is what the host calls
/// a pull request.
pub fn pick_patches<S: Service>(
    files: Vec<ChangedFile>,
    query: &PatchQuery,
    change: &str,
) -> Result<Vec<ChangedFile>, ServiceError<S>> {
    if query.full_patches {
        return Ok(files);
    }
    if let Some(unknown) = query
        .paths
        .iter()
        .find(|path| !files.iter().any(|file| is_named(file, path)))
    {
        return Err(ServiceError::BadArg(format!(
            "`{unknown}` is not among the files this {change} changes"
        )));
    }
    Ok(files
        .into_iter()
        .filter_map(|mut file| {
            if query.paths.is_empty() {
                file.patch = None;
                return Some(file);
            }
            if !query.paths.iter().any(|path| is_named(&file, path)) {
                return None;
            }
            match file.patch.take() {
                Some(patch) => {
                    let (shown, note) = window(&patch, query.offset, query.lines);
                    file.patch = Some(shown);
                    file.note = note;
                }
                None => {
                    file.note = Some(format!(
                        "{} shows no patch for this file: it is binary, too large, or only renamed. Run git diff locally.",
                        S::NAME
                    ));
                }
            }
            Some(file)
        })
        .collect())
}

/// The lines of a job's log `pick` asks for, the end of it when it names none,
/// cut to what an agent can read at once.
pub fn log_excerpt(whole: JobLog, pick: LinePick) -> LogExcerpt {
    let in_range = |line: &LogLine| {
        pick.from.is_none_or(|from| line.number >= from)
            && pick.to.is_none_or(|to| line.number <= to)
    };
    let mut lines: Vec<LogLine> = whole.lines.into_iter().filter(in_range).collect();
    let needle = pick
        .grep
        .map(|grep| grep.to_lowercase())
        .filter(|grep| !grep.is_empty());
    if let Some(needle) = needle {
        let context = pick.context.unwrap_or(0).min(MAX_CONTEXT);
        let mut kept = vec![false; lines.len()];
        for (index, line) in lines.iter().enumerate() {
            if line.text.to_lowercase().contains(&needle) {
                let end = (index + context + 1).min(lines.len());
                kept[index.saturating_sub(context)..end].fill(true);
            }
        }
        let mut kept = kept.into_iter();
        lines.retain(|_| kept.next().unwrap_or(false));
    }
    let forward = pick.from.is_some();
    let most = pick.tail.unwrap_or(DEFAULT_TAIL).clamp(1, MAX_TAIL);
    let mut cut = lines.len() > most;
    if cut {
        if forward {
            lines.truncate(most);
        } else {
            lines.drain(..lines.len() - most);
        }
    }
    for line in &mut lines {
        if line.text.len() > LINE_CHARS {
            line.text = format!("{}…", cut_at_char(&line.text, LINE_CHARS));
        }
    }
    let mut chars = 0;
    let mut fits = |line: &LogLine| {
        chars += line.text.len() + 16;
        chars <= EXCERPT_CHARS
    };
    let kept = if forward {
        lines.iter().take_while(|line| fits(line)).count()
    } else {
        lines.iter().rev().take_while(|line| fits(line)).count()
    };
    let note = (kept < lines.len()).then(|| {
        let left = if forward {
            lines.drain(kept..).collect::<Vec<_>>()
        } else {
            lines.drain(..lines.len() - kept).collect()
        };
        let (first, last) = (left[0].number, left[left.len() - 1].number);
        format!(
            "Lines {first} to {last} were left out to keep this under {EXCERPT_CHARS} characters; ask for them with from and to."
        )
    });
    cut |= note.is_some();
    LogExcerpt {
        lines,
        truncated: cut || whole.truncated,
        note,
    }
}

/// A name that goes into an API path, with only the characters every host allows in one.
pub fn valid_name(name: &str, most: usize) -> bool {
    !name.is_empty()
        && name.len() <= most
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A commit named in a path, at least long enough to be one commit.
pub fn commit_hash<S>(sha: &str) -> Result<&str, ServiceError<S>> {
    if sha.len() < 7 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ServiceError::BadArg(format!(
            "`{sha}` is not a commit hash"
        )));
    }
    Ok(sha)
}

/// The pull requests that hold a commit, the merged one first, as `commitPulls` promises.
pub fn merged_first(mut pulls: Vec<Pull>) -> Vec<Pull> {
    pulls.sort_by_key(|pull| pull.state != PullStage::Merged);
    pulls
}

/// The server and path of a git remote, in any of the ways a remote can be
/// written: scp-like `git@host:owner/repo.git`, or an `https`, `ssh` or `git` URL.
pub fn split_remote(remote: &str) -> Option<(String, &str)> {
    let remote = remote.trim();
    let (host, path) = if let Some((_, rest)) = remote.split_once("://") {
        let rest = rest.split_once('@').map_or(rest, |(_, after)| after);
        let (authority, path) = rest.split_once('/')?;
        (authority.split(':').next()?, path)
    } else {
        let rest = remote.split_once('@').map_or(remote, |(_, after)| after);
        rest.split_once(':')?
    };
    (!host.is_empty()).then(|| {
        (
            host.to_ascii_lowercase(),
            path.trim_end_matches('/').trim_end_matches(".git"),
        )
    })
}

/// The first of `accounts` that can read `path`, default first, so a project
/// opens as the account that can see it by itself.
pub async fn first_that_sees<A: Api>(
    calls: &Calls<A>,
    data_dir: &std::path::Path,
    accounts: Vec<String>,
    path: &str,
) -> Result<Option<String>, ServiceError<A>> {
    for id in accounts {
        let seen: Result<Value, _> =
            as_account(Some(id.clone()), calls.get(data_dir, path, &[])).await;
        match seen {
            Ok(_) => return Ok(Some(id)),
            Err(ServiceError::Unconfigured) => return Ok(None),
            Err(_) => continue,
        }
    }
    Ok(None)
}

/// Every account, default first, by id.
pub fn ids_in_order<C: Accounts>(config: &C) -> Vec<String> {
    config
        .in_order()
        .iter()
        .map(|account| account.id().to_owned())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::codehost::asks::FilesQuery;
    use serde_json::json;

    struct Host;

    impl Service for Host {
        const ID: &'static str = "host";
        const NAME: &'static str = "Host";
    }

    fn picked(
        files: Vec<ChangedFile>,
        query: &PatchQuery,
    ) -> Result<Vec<ChangedFile>, ServiceError<Host>> {
        pick_patches(files, query, "pull request")
    }

    fn changed(path: &str, patch: Option<&str>) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            status: "modified".into(),
            additions: 1,
            deletions: 1,
            previous_path: None,
            patch: patch.map(Into::into),
            note: None,
        }
    }

    fn asking(paths: &[&str]) -> PatchQuery {
        PatchQuery {
            paths: paths.iter().map(|path| path.to_string()).collect(),
            ..PatchQuery::default()
        }
    }

    #[test]
    fn lists_the_files_without_their_patches_by_default() {
        let mut renamed = changed("new.rs", Some("@@ -1 +1 @@\n-a\n+b"));
        renamed.status = "renamed".into();
        renamed.previous_path = Some("old.rs".into());
        let listed = picked(
            vec![renamed, changed("b.rs", Some("@@"))],
            &PatchQuery::default(),
        )
        .unwrap();
        assert!(listed
            .iter()
            .all(|file| file.patch.is_none() && file.note.is_none()));
        assert_eq!(
            serde_json::to_value(&listed[0]).unwrap(),
            json!({ "path": "new.rs", "status": "renamed", "additions": 1, "deletions": 1, "previousPath": "old.rs" })
        );
        assert_eq!(
            serde_json::to_value(&listed[1]).unwrap(),
            json!({ "path": "b.rs", "status": "modified", "additions": 1, "deletions": 1 })
        );
    }

    #[test]
    fn named_paths_come_back_alone_with_their_patches() {
        let files = vec![
            changed("a.rs", Some("@@ -1 +1 @@\n-a\n+b")),
            changed("b.rs", Some("@@")),
            changed("logo.png", None),
        ];
        let some = picked(files.clone(), &asking(&["a.rs", "logo.png"])).unwrap();
        assert_eq!(some.len(), 2);
        assert_eq!(some[0].patch.as_deref(), Some("@@ -1 +1 @@\n-a\n+b"));
        assert_eq!(some[0].note, None);
        assert!(some[1].patch.is_none());
        assert!(some[1].note.as_deref().unwrap().contains("git diff"));

        assert!(matches!(
            picked(files.clone(), &asking(&["missing.rs"])),
            Err(ServiceError::BadArg(_))
        ));
        let full = PatchQuery {
            full_patches: true,
            ..PatchQuery::default()
        };
        assert_eq!(picked(files.clone(), &full).unwrap(), files);
    }

    #[test]
    fn reads_the_patch_arguments_beside_the_pull_request() {
        let query: FilesQuery = serde_json::from_value(json!({
            "owner": "a", "name": "b", "number": 3, "paths": ["x.rs"], "offset": 400, "lines": 50,
        }))
        .unwrap();
        assert_eq!(query.pull.number, 3);
        assert_eq!(query.patches.paths, ["x.rs"]);
        assert_eq!((query.patches.offset, query.patches.lines), (400, Some(50)));
        assert!(!query.patches.full_patches);
        let view: FilesQuery = serde_json::from_value(
            json!({ "owner": "a", "name": "b", "number": 3, "fullPatches": true }),
        )
        .unwrap();
        assert!(view.patches.full_patches);
    }

    #[test]
    fn a_rename_is_found_by_its_old_path_too() {
        let mut renamed = changed("new.rs", Some("@@"));
        renamed.previous_path = Some("old.rs".into());
        let found = picked(vec![renamed, changed("b.rs", None)], &asking(&["old.rs"])).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "new.rs");
    }

    #[test]
    fn a_long_patch_is_cut_and_says_how_to_read_the_rest() {
        let patch: Vec<String> = (1..=1000).map(|line| format!("+line {line}")).collect();
        let patch = patch.join("\n");
        let (shown, note) = window(&patch, 0, None);
        assert_eq!(shown.lines().count(), PATCH_LINES);
        assert_eq!(shown.lines().last(), Some("+line 400"));
        assert_eq!(
            note.as_deref(),
            Some("Lines 1-400 of 1000. 600 more lines left out: pass offset 400 for the next part, or run git diff locally.")
        );

        let (shown, note) = window(&patch, 990, None);
        assert_eq!(shown.lines().next(), Some("+line 991"));
        assert_eq!(note.as_deref(), Some("Lines 991-1000 of 1000."));

        let (shown, _) = window(&patch, 0, Some(5));
        assert_eq!(shown.lines().count(), 5);

        let (shown, note) = window(&patch, 5000, None);
        assert!(shown.is_empty());
        assert!(note.unwrap().contains("past its end"));
    }

    #[test]
    fn a_patch_stops_at_its_byte_budget() {
        let wide = format!("+{}", "é".repeat(PATCH_BYTES));
        let patch = format!("@@ -1 +1 @@\n{wide}\n+after");
        let (shown, note) = window(&patch, 0, None);
        assert_eq!(shown, "@@ -1 +1 @@");
        assert!(note.unwrap().contains("pass offset 1"));

        let (shown, note) = window(&patch, 1, None);
        assert!(shown.len() <= PATCH_BYTES);
        assert!(shown.starts_with("+é"));
        assert!(note.unwrap().contains("Line 2 was cut at 32 KB"));
    }

    #[test]
    fn an_excerpt_keeps_the_end_and_the_lines_that_mention_something() {
        let whole = JobLog {
            lines: (1..=5)
                .map(|number| LogLine {
                    number,
                    timestamp: None,
                    text: if number == 3 { "Error" } else { "fine" }.into(),
                })
                .collect(),
            expired: false,
            truncated: false,
        };
        let found = log_excerpt(
            whole,
            LinePick {
                grep: Some("error".into()),
                tail: Some(10),
                ..LinePick::default()
            },
        );
        assert_eq!(found.lines.len(), 1);
        assert_eq!(found.lines[0].number, 3);
        assert!(!found.truncated);
    }

    #[test]
    fn a_remote_is_read_in_every_way_it_can_be_written() {
        for remote in [
            "git@GitLab.com:group/app.git",
            "https://gitlab.com/group/app.git/",
            "ssh://git@gitlab.com:22/group/app",
        ] {
            assert_eq!(
                split_remote(remote),
                Some(("gitlab.com".to_string(), "group/app"))
            );
        }
        assert_eq!(split_remote("not a remote"), None);
    }
}
