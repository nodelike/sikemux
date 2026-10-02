use std::path::Path;

use serde::Serialize;

use super::log::relative_time;
use super::{
    git_command, git_walk_permit, open_repo, run_blocking, run_command_with_timeout, run_git,
    GIT_COMMAND_TIMEOUT,
};

#[derive(Serialize, Clone)]
pub struct BlameCommit {
    /// Full commit oid; all-zeros for not-yet-committed lines.
    sha: String,
    /// Short id for display (`1d3fa0b2`); empty when uncommitted.
    short: String,
    author: String,
    author_email: String,
    /// Pre-formatted relative time (`3 days ago`); empty when uncommitted.
    time: String,
    /// Raw author timestamp (unix seconds) — kept so the UI can re-format.
    timestamp: i64,
    summary: String,
    /// True for lines that only exist in the working buffer (zero sha).
    uncommitted: bool,
}

/// Compact per-file blame: the unique commits touched, plus a parallel array
/// mapping each 0-based line to its commit's index in `commits`. The split
/// keeps the IPC payload small (commit metadata isn't repeated per line) and
/// lets the editor do O(1) cursor-line lookups with no further backend calls.
#[derive(Serialize, Default)]
pub struct GitBlame {
    commits: Vec<BlameCommit>,
    lines: Vec<u32>,
}

fn is_zero_sha(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b == b'0')
}

fn blame_commit(
    sha: String,
    author: String,
    author_email: String,
    timestamp: i64,
    summary: String,
) -> BlameCommit {
    let uncommitted = is_zero_sha(&sha);
    BlameCommit {
        short: if uncommitted {
            String::new()
        } else {
            sha[..8.min(sha.len())].to_string()
        },
        author: if uncommitted {
            "You".to_string()
        } else {
            author
        },
        author_email,
        time: if uncommitted {
            String::new()
        } else {
            relative_time(timestamp)
        },
        timestamp,
        summary: if uncommitted {
            "Uncommitted changes".to_string()
        } else {
            summary
        },
        uncommitted,
        sha,
    }
}

/// Parse `git blame --porcelain`. Commit metadata is emitted only the first
/// time each commit appears, so we accumulate it keyed by sha and remember
/// first-seen order for stable indices.
fn parse_blame_porcelain(out: &str) -> GitBlame {
    use std::collections::HashMap;

    struct Meta {
        author: String,
        author_email: String,
        timestamp: i64,
        summary: String,
    }

    let mut meta: HashMap<String, Meta> = HashMap::new();
    let mut index_of: HashMap<String, u32> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut line_sha: Vec<(usize, String)> = Vec::new();

    let mut cur = String::new();
    let mut cur_line = 0usize;
    let mut max_line = 0usize;

    for line in out.lines() {
        // Content line — closes the entry for the line we last saw a header for.
        if let Some(_content) = line.strip_prefix('\t') {
            if !cur.is_empty() && cur_line > 0 {
                line_sha.push((cur_line, cur.clone()));
                if cur_line > max_line {
                    max_line = cur_line;
                }
            }
            continue;
        }

        // Header: "<40-hex-sha> <orig-line> <final-line> [<group-count>]".
        let b = line.as_bytes();
        let is_header =
            b.len() > 40 && b[40] == b' ' && b[..40].iter().all(|c| c.is_ascii_hexdigit());
        if is_header {
            let mut parts = line.split(' ');
            let sha = parts.next().unwrap_or("").to_string();
            let _orig = parts.next();
            cur_line = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            cur = sha.clone();
            if !index_of.contains_key(&sha) {
                index_of.insert(sha.clone(), order.len() as u32);
                order.push(sha.clone());
                meta.insert(
                    sha,
                    Meta {
                        author: String::new(),
                        author_email: String::new(),
                        timestamp: 0,
                        summary: String::new(),
                    },
                );
            }
            continue;
        }

        // Metadata line for the current commit.
        if let Some(rest) = line.strip_prefix("author ") {
            if let Some(m) = meta.get_mut(&cur) {
                m.author = rest.to_string();
            }
        } else if let Some(rest) = line.strip_prefix("author-mail ") {
            if let Some(m) = meta.get_mut(&cur) {
                m.author_email = rest.trim_matches(|c| c == '<' || c == '>').to_string();
            }
        } else if let Some(rest) = line.strip_prefix("author-time ") {
            if let Some(m) = meta.get_mut(&cur) {
                m.timestamp = rest.trim().parse().unwrap_or(0);
            }
        } else if let Some(rest) = line.strip_prefix("summary ") {
            if let Some(m) = meta.get_mut(&cur) {
                m.summary = rest.to_string();
            }
        }
    }

    let commits: Vec<BlameCommit> = order
        .iter()
        .map(|sha| {
            let m = &meta[sha];
            blame_commit(
                sha.clone(),
                m.author.clone(),
                m.author_email.clone(),
                m.timestamp,
                m.summary.clone(),
            )
        })
        .collect();

    let mut lines = vec![0u32; max_line];
    for (ln, sha) in line_sha {
        if ln >= 1 && ln <= max_line {
            if let Some(&idx) = index_of.get(&sha) {
                lines[ln - 1] = idx;
            }
        }
    }

    GitBlame { commits, lines }
}

/// Filter drivers are commands git runs on file contents. Ones from system or
/// global config are the user's own; any other scope came with the repo.
fn repo_defines_filters(repo: &str) -> bool {
    match run_git(
        repo,
        &[
            "config",
            "--show-scope",
            "--includes",
            "--get-regexp",
            r"^filter\..*\.(clean|smudge|process)$",
        ],
    ) {
        Ok((true, out, _)) => out
            .lines()
            .any(|line| !matches!(line.split('\t').next(), Some("system" | "global"))),
        Ok((false, out, err)) => !out.trim().is_empty() || !err.trim().is_empty(),
        Err(_) => true,
    }
}

fn blame_with_libgit2(
    repo: &str,
    path: &str,
    contents: Option<String>,
) -> Result<GitBlame, String> {
    use std::collections::HashMap;

    let r = open_repo(repo)?;
    let buffer = match contents {
        Some(text) => text.into_bytes(),
        None => std::fs::read(Path::new(repo).join(path)).map_err(|e| e.to_string())?,
    };
    let committed = r
        .blame_file(Path::new(path), None)
        .map_err(|e| e.message().to_string())?;
    let blame = committed
        .blame_buffer(&buffer)
        .map_err(|e| e.message().to_string())?;

    let mut commits = Vec::new();
    let mut index_of: HashMap<git2::Oid, u32> = HashMap::new();
    let mut lines: Vec<u32> = Vec::new();
    for hunk in blame.iter() {
        let oid = hunk.final_commit_id();
        let index = *index_of.entry(oid).or_insert_with(|| {
            let commit = r.find_commit(oid).ok();
            let author = commit.as_ref().map(|c| c.author());
            commits.push(blame_commit(
                oid.to_string(),
                author
                    .as_ref()
                    .map(|a| String::from_utf8_lossy(a.name_bytes()).into_owned())
                    .unwrap_or_default(),
                author
                    .as_ref()
                    .map(|a| String::from_utf8_lossy(a.email_bytes()).into_owned())
                    .unwrap_or_default(),
                author.as_ref().map(|a| a.when().seconds()).unwrap_or(0),
                commit
                    .as_ref()
                    .and_then(|c| c.summary_bytes())
                    .map(|b| String::from_utf8_lossy(b).into_owned())
                    .unwrap_or_default(),
            ));
            (commits.len() - 1) as u32
        });
        let start = hunk.final_start_line();
        let end = start + hunk.lines_in_hunk();
        if start == 0 {
            continue;
        }
        if lines.len() < end - 1 {
            lines.resize(end - 1, 0);
        }
        for line in start..end {
            lines[line - 1] = index;
        }
    }
    Ok(GitBlame { commits, lines })
}

/// Blame a single file. When `contents` is provided we blame that buffer via
/// `--contents -` so unsaved editor edits line up correctly (those lines come
/// back as the zero-sha "uncommitted" commit). Untracked / no-HEAD / binary
/// files have nothing to blame and yield an empty result rather than an error
/// so the editor just shows no inline blame.
#[tauri::command]
pub async fn git_blame(
    repo: String,
    path: String,
    contents: Option<String>,
) -> Result<GitBlame, String> {
    let _permit = git_walk_permit().await?;
    run_blocking(move || -> Result<GitBlame, String> {
        if repo_defines_filters(&repo) {
            return Ok(blame_with_libgit2(&repo, &path, contents).unwrap_or_default());
        }
        let out = match contents {
            Some(text) => {
                let mut command = git_command(&repo);
                command.args([
                    "blame",
                    "--no-textconv",
                    "--porcelain",
                    "--contents",
                    "-",
                    "--",
                    &path,
                ]);
                let o = run_command_with_timeout(
                    &mut command,
                    Some(text.as_bytes()),
                    GIT_COMMAND_TIMEOUT,
                )?;
                if !o.status.success() {
                    return Ok(GitBlame::default());
                }
                String::from_utf8_lossy(&o.stdout).into_owned()
            }
            None => {
                let (ok, so, _se) = run_git(
                    &repo,
                    &["blame", "--no-textconv", "--porcelain", "--", &path],
                )?;
                if !ok {
                    return Ok(GitBlame::default());
                }
                so
            }
        };
        Ok(parse_blame_porcelain(&out))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::changes::git_diff;
    use crate::git::revisions::git_show;
    use crate::git::status::git_status;
    use crate::git::tests::{commit_base, git, init_repo, repo_arg};
    use std::fs;

    use tempfile::tempdir;

    #[tokio::test]
    async fn blame_maps_committed_and_uncommitted_lines() {
        let td = init_repo();
        fs::write(td.path().join("f.txt"), "one\ntwo\nthree\n").expect("write");
        git(td.path(), &["add", "f.txt"]);
        git(td.path(), &["commit", "-m", "seed"]);

        // Disk blame: every line attributed to the single seed commit.
        let on_disk = git_blame(repo_arg(td.path()), "f.txt".into(), None)
            .await
            .expect("blame disk");
        assert_eq!(on_disk.lines.len(), 3);
        assert!(on_disk.lines.iter().all(|&i| i == on_disk.lines[0]));
        let c = &on_disk.commits[on_disk.lines[0] as usize];
        assert_eq!(c.author, "sikemux");
        assert!(!c.uncommitted);
        assert_eq!(c.summary, "seed");
        assert_eq!(c.short.len(), 8);

        // Buffer blame: an appended line shows as not-yet-committed.
        let buffer = "one\ntwo\nthree\nfour\n".to_string();
        let blame = git_blame(repo_arg(td.path()), "f.txt".into(), Some(buffer))
            .await
            .expect("blame buffer");
        assert_eq!(blame.lines.len(), 4);
        let last = &blame.commits[blame.lines[3] as usize];
        assert!(last.uncommitted, "appended line should be uncommitted");
        assert_eq!(last.summary, "Uncommitted changes");
        assert!(!blame.commits[blame.lines[0] as usize].uncommitted);

        let library = blame_with_libgit2(
            &repo_arg(td.path()),
            "f.txt",
            Some("one\ntwo\nthree\nfour\n".into()),
        )
        .expect("library blame");
        let shas = |b: &GitBlame| -> Vec<String> {
            b.lines
                .iter()
                .map(|&i| b.commits[i as usize].sha.clone())
                .collect()
        };
        assert_eq!(shas(&library), shas(&blame));
    }

    fn arm_hostile_config(repo: &Path, markers: &Path) {
        for (name, body) in [
            ("fsmonitor", "exit 1"),
            ("clean", "cat"),
            ("textconv", "cat \"$1\""),
        ] {
            let script = markers.join(format!("{name}.sh"));
            let marker = markers.join(format!("{name}.ran"));
            fs::write(
                &script,
                format!("#!/bin/sh\ntouch '{}'\n{body}\n", marker.display()),
            )
            .expect("write script");
            sikemux_process::user_environment::command("chmod")
                .arg("+x")
                .arg(&script)
                .status()
                .expect("chmod");
        }
        let script = |name: &str| markers.join(format!("{name}.sh")).display().to_string();
        git(repo, &["config", "core.fsmonitor", &script("fsmonitor")]);
        git(repo, &["config", "filter.evil.clean", &script("clean")]);
        git(repo, &["config", "filter.evil.smudge", &script("clean")]);
        git(repo, &["config", "diff.evil.textconv", &script("textconv")]);
    }

    fn markers_left(markers: &Path) -> Vec<String> {
        let mut ran: Vec<String> = fs::read_dir(markers)
            .expect("read markers")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".ran"))
            .collect();
        ran.sort();
        ran
    }

    #[tokio::test]
    async fn reading_files_in_a_hostile_repo_runs_none_of_its_commands() {
        let td = init_repo();
        let markers = tempdir().expect("markers");
        fs::write(
            td.path().join(".gitattributes"),
            "*.txt filter=evil diff=evil\n",
        )
        .expect("write attributes");
        fs::write(td.path().join("f.txt"), "one\ntwo\nthree\n").expect("write");
        git(td.path(), &["add", "."]);
        git(td.path(), &["commit", "-m", "seed"]);
        arm_hostile_config(td.path(), markers.path());
        fs::write(td.path().join("f.txt"), "one\ntwo\nthree\nfour\n").expect("edit");
        fs::write(td.path().join("new.txt"), "hi\n").expect("write untracked");
        let repo = repo_arg(td.path());

        assert!(repo_defines_filters(&repo));
        let on_disk = git_blame(repo.clone(), "f.txt".into(), None)
            .await
            .expect("blame disk");
        assert_eq!(on_disk.lines.len(), 4);
        let seed = &on_disk.commits[on_disk.lines[0] as usize];
        assert_eq!(seed.author, "sikemux");
        assert_eq!(seed.author_email, "sikemux@example.test");
        assert_eq!(seed.summary, "seed");
        assert_eq!(seed.short.len(), 8);
        assert!(!seed.uncommitted);
        let last = &on_disk.commits[on_disk.lines[3] as usize];
        assert!(last.uncommitted);
        assert_eq!(last.summary, "Uncommitted changes");

        let buffer = git_blame(repo.clone(), "f.txt".into(), Some("zero\none\n".into()))
            .await
            .expect("blame buffer");
        assert_eq!(buffer.lines.len(), 2);
        assert!(buffer.commits[buffer.lines[0] as usize].uncommitted);
        assert!(!buffer.commits[buffer.lines[1] as usize].uncommitted);

        let diff = git_diff(repo.clone(), "new.txt".into(), false)
            .await
            .expect("untracked diff");
        assert!(diff.contains("+hi"), "{diff}");
        git_show(repo.clone(), "HEAD".into()).await.expect("show");
        git_status(repo.clone()).await.expect("status");

        assert_eq!(markers_left(markers.path()), Vec::<String>::new());
    }

    #[test]
    fn filter_drivers_the_repo_defines_are_untrusted() {
        let td = init_repo();
        let repo = repo_arg(td.path());
        assert!(!repo_defines_filters(&repo));
        git(
            td.path(),
            &["config", "filter.lfs.clean", "git-lfs clean -- %f"],
        );
        assert!(repo_defines_filters(&repo));
    }

    #[tokio::test]
    async fn blame_untracked_file_is_empty() {
        let td = init_repo();
        commit_base(td.path());
        fs::write(td.path().join("new.txt"), "hi\n").expect("write");
        let blame = git_blame(repo_arg(td.path()), "new.txt".into(), None)
            .await
            .expect("blame untracked");
        assert!(blame.commits.is_empty());
        assert!(blame.lines.is_empty());
    }
}
