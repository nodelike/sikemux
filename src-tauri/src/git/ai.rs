use std::time::Duration;

use super::ai_provider::{run_ai_commit_model, GitAiProvider};
use super::commit::commit_with_message;
use super::{git_has_head, git_ok, run_blocking};

const PRIMARY_DIFF_BUDGET: usize = 18_000;
const STAT_BUDGET: usize = 4_000;
pub(super) const AI_COMMAND_TIMEOUT: Duration = Duration::from_secs(5 * 60);
pub(super) const AI_RESPONSE_LIMIT: usize = 1024 * 1024;
const OMITTED_HUNK_MARKER: &str = "\n[... hunk body compacted ...]\n";
const OMITTED_GENERATED_MARKER: &str =
    "[generated, vendored, or lockfile patch omitted; file and hunk retained]\n";

fn clean_commit_message(raw: &str) -> Result<String, String> {
    let mut t = raw.trim().to_string();
    if t.starts_with("```") {
        if let Some(nl) = t.find('\n') {
            t = t[nl + 1..].to_string();
        }
    }
    let te = t.trim_end();
    if let Some(stripped) = te.strip_suffix("```") {
        t = stripped.to_string();
    }
    const CONV: [&str; 11] = [
        "feat", "fix", "refactor", "chore", "docs", "test", "perf", "build", "ci", "style",
        "revert",
    ];
    let mut lines: Vec<&str> = t.lines().collect();
    while let Some(first) = lines.first() {
        let f = first.trim();
        let is_conv = CONV.iter().any(|p| {
            f.strip_prefix(p)
                .map(|r| r.starts_with(':') || r.starts_with('(') || r.starts_with('!'))
                .unwrap_or(false)
        });
        if is_conv {
            break;
        }
        lines.remove(0);
    }
    while lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        lines.pop();
    }
    if lines.is_empty() {
        return Err("AI returned no usable commit message".into());
    }
    Ok(lines.join("\n"))
}

/// Build the AI prompt from a stat + diff. Shared by the generate-only
/// (`git_ai_message`) and stage-and-commit (`git_ai_commit`) paths so the
/// message style stays identical.
fn commit_message_prompt(repo: &str, stat: &str, diff: &str, compacted: bool) -> String {
    let branch = git_ok(repo, &["branch", "--show-current"])
        .unwrap_or_default()
        .trim()
        .to_string();
    let repo_name = std::path::Path::new(repo)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let diff_note = if compacted {
        "Unchanged context was removed. Every changed file and hunk header is represented, but noisy files and oversized hunk bodies may be compacted. Infer the commit from the retained changed lines plus the stat."
    } else {
        "Unchanged context was removed; all changed lines are included."
    };
    format!(
        "You are generating a Git commit message from the staged diff below.\n\
         Return ONLY the commit message. No markdown, no explanation, no quotes, no code fences.\n\n\
         Rules:\n\
         - First line: conventional commit format: type(scope): subject\n\
         - Imperative mood, no trailing period, <=72 chars if possible\n\
         - For trivial changes: subject line only\n\
         - For non-trivial changes: blank line after subject, then 2-6 bullets starting with \"- \"\n\
         - Common types: feat, fix, refactor, chore, docs, test, perf, build, ci, style\n\
         - Scope should be short and inferred from files/package/service when obvious; omit scope if unclear\n\n\
         Repo: {repo_name}\n\
         Branch: {branch}\n\n\
         Staged stat:\n{stat}\n\n\
         Diff note: {diff_note}\n\n\
         Staged changed-line diff:\n{diff}\n"
    )
}

#[derive(Debug)]
struct DiffHunk {
    header: String,
    body: String,
}

#[derive(Debug, Default)]
struct DiffFile {
    header: String,
    hunks: Vec<DiffHunk>,
}

#[derive(Debug)]
struct PreparedDiff {
    text: String,
    compacted: bool,
}

fn parse_diff_files(diff: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut current: Option<DiffFile> = None;

    for line in diff.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            if let Some(file) = current.take() {
                files.push(file);
            }
            current = Some(DiffFile {
                header: line.to_string(),
                hunks: Vec::new(),
            });
            continue;
        }

        let file = current.get_or_insert_with(DiffFile::default);
        if line.starts_with("@@") || line == "GIT binary patch\n" {
            file.hunks.push(DiffHunk {
                header: line.to_string(),
                body: String::new(),
            });
        } else if let Some(hunk) = file.hunks.last_mut() {
            hunk.body.push_str(line);
        } else {
            file.header.push_str(line);
        }
    }
    if let Some(file) = current {
        files.push(file);
    }
    files
}

fn is_noisy_diff(file: &DiffFile) -> bool {
    let header = file.header.to_ascii_lowercase();
    const NAMES: [&str; 12] = [
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lock",
        "bun.lockb",
        "cargo.lock",
        "gemfile.lock",
        "poetry.lock",
        "uv.lock",
        "composer.lock",
        "go.sum",
        ".snap",
    ];
    NAMES.iter().any(|name| header.contains(name))
        || ["/dist/", "/vendor/", ".min.js", ".min.css", ".map"]
            .iter()
            .any(|part| header.contains(part))
}

/// Distribute a byte budget evenly at first, then give unused shares to larger
/// entries. This prevents an early large file from starving every later file.
fn fair_allocations(sizes: &[usize], budget: usize) -> Vec<usize> {
    let mut allocations = vec![0; sizes.len()];
    let mut remaining = budget;
    let mut active: Vec<usize> = sizes
        .iter()
        .enumerate()
        .filter_map(|(index, size)| (*size > 0).then_some(index))
        .collect();

    while remaining > 0 && !active.is_empty() {
        let share = (remaining / active.len()).max(1);
        let mut granted = 0;
        active.retain(|index| {
            let need = sizes[*index] - allocations[*index];
            let available = remaining.saturating_sub(granted);
            let give = need.min(share).min(available);
            allocations[*index] += give;
            granted += give;
            allocations[*index] < sizes[*index] && granted < remaining
        });
        if granted == 0 {
            break;
        }
        remaining -= granted;
    }
    allocations
}

fn utf8_prefix(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn utf8_suffix(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut start = value.len() - max_bytes;
    while start < value.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

fn sample_hunk_body(body: &str, budget: usize) -> String {
    if body.len() <= budget {
        return body.to_string();
    }
    format!(
        "{}{}{}",
        utf8_prefix(body, budget.div_ceil(2)),
        OMITTED_HUNK_MARKER,
        utf8_suffix(body, budget / 2)
    )
}

fn structure_manifest(files: &[DiffFile], budget: usize) -> String {
    let mut manifest = String::from(
        "[diff has more structural metadata than the request budget; patch bodies omitted]\n",
    );
    for file in files {
        if let Some(first) = file.header.lines().next() {
            manifest.push_str(first);
            manifest.push('\n');
        }
        for hunk in &file.hunks {
            manifest.push_str(&hunk.header);
        }
    }
    if manifest.len() <= budget {
        return manifest;
    }
    let marker = "\n[... structural manifest compacted ...]\n";
    let content_budget = budget.saturating_sub(marker.len());
    format!(
        "{}{}{}",
        utf8_prefix(&manifest, content_budget.div_ceil(2)),
        marker,
        utf8_suffix(&manifest, content_budget / 2)
    )
}

fn prepare_diff(diff: &str, budget: usize) -> PreparedDiff {
    if diff.len() <= budget {
        return PreparedDiff {
            text: diff.to_string(),
            compacted: false,
        };
    }

    let files = parse_diff_files(diff);
    let noisy: Vec<bool> = files.iter().map(is_noisy_diff).collect();
    let mut fixed_size = 0usize;
    let mut file_body_sizes = Vec::with_capacity(files.len());
    for (file, noisy) in files.iter().zip(&noisy) {
        fixed_size = fixed_size.saturating_add(file.header.len());
        let mut body_size = 0usize;
        if *noisy && file.hunks.is_empty() {
            fixed_size = fixed_size.saturating_add(OMITTED_GENERATED_MARKER.len());
        }
        for hunk in &file.hunks {
            fixed_size = fixed_size.saturating_add(hunk.header.len());
            if *noisy {
                fixed_size = fixed_size.saturating_add(OMITTED_GENERATED_MARKER.len());
            } else if !hunk.body.is_empty() {
                fixed_size = fixed_size.saturating_add(OMITTED_HUNK_MARKER.len());
                body_size = body_size.saturating_add(hunk.body.len());
            }
        }
        file_body_sizes.push(body_size);
    }

    if fixed_size >= budget {
        return PreparedDiff {
            text: structure_manifest(&files, budget),
            compacted: true,
        };
    }

    let file_allocations = fair_allocations(&file_body_sizes, budget - fixed_size);
    let mut output = String::with_capacity(budget);
    for ((file, noisy), file_budget) in files.iter().zip(&noisy).zip(file_allocations) {
        output.push_str(&file.header);
        if *noisy && file.hunks.is_empty() {
            output.push_str(OMITTED_GENERATED_MARKER);
        }
        let hunk_sizes: Vec<usize> = file.hunks.iter().map(|hunk| hunk.body.len()).collect();
        let hunk_allocations = fair_allocations(&hunk_sizes, file_budget);
        for (hunk, hunk_budget) in file.hunks.iter().zip(hunk_allocations) {
            output.push_str(&hunk.header);
            if *noisy {
                output.push_str(OMITTED_GENERATED_MARKER);
            } else {
                output.push_str(&sample_hunk_body(&hunk.body, hunk_budget));
            }
        }
    }
    PreparedDiff {
        text: output,
        compacted: true,
    }
}

fn compact_stat(stat: &str) -> String {
    if stat.len() <= STAT_BUDGET {
        return stat.to_string();
    }
    let marker = "\n[... stat compacted ...]\n";
    let budget = STAT_BUDGET - marker.len();
    format!(
        "{}{}{}",
        utf8_prefix(stat, budget.div_ceil(2)),
        marker,
        utf8_suffix(stat, budget / 2)
    )
}

fn staged_diff(repo: &str) -> Result<(String, String), String> {
    Ok((
        git_ok(repo, &["diff", "--cached", "--stat"])?,
        git_ok(
            repo,
            &[
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                "--unified=0",
            ],
        )?,
    ))
}

fn worktree_diff(repo: &str) -> Result<(String, String), String> {
    if git_has_head(repo) {
        Ok((
            git_ok(repo, &["diff", "HEAD", "--stat"])?,
            git_ok(
                repo,
                &[
                    "diff",
                    "HEAD",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--unified=0",
                ],
            )?,
        ))
    } else {
        Ok((
            git_ok(repo, &["diff", "--stat"])?,
            git_ok(
                repo,
                &["diff", "--no-ext-diff", "--no-textconv", "--unified=0"],
            )?,
        ))
    }
}

fn describe_diff_with_local_ai(
    repo: &str,
    stat: &str,
    diff: &str,
    provider: GitAiProvider,
    model: &str,
    on_chunk: Option<tauri::ipc::Channel<String>>,
) -> Result<String, String> {
    let stat = compact_stat(stat);
    let prepared = prepare_diff(diff, PRIMARY_DIFF_BUDGET);
    let prompt = commit_message_prompt(repo, &stat, &prepared.text, prepared.compacted);
    clean_commit_message(&run_ai_commit_model(
        repo, provider, model, &prompt, on_chunk,
    )?)
}

/// Generate a commit message through a locally installed CLI without staging
/// or committing. Output chunks are forwarded to the commit textarea.
#[tauri::command]
pub async fn git_ai_message(
    repo: String,
    provider: Option<String>,
    model: Option<String>,
    on_chunk: tauri::ipc::Channel<String>,
) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        let (mut stat, mut diff) = staged_diff(&repo)?;
        if diff.trim().is_empty() {
            (stat, diff) = worktree_diff(&repo)?;
        }
        if diff.trim().is_empty() {
            return Err("Nothing to describe — stage changes or edit some files first.".into());
        }
        let provider = GitAiProvider::parse(provider)?;
        let model = model.unwrap_or_else(|| provider.default_model().to_string());
        describe_diff_with_local_ai(&repo, &stat, &diff, provider, &model, Some(on_chunk))
    })
    .await
}

#[tauri::command]
pub async fn git_ai_commit(
    repo: String,
    provider: Option<String>,
    model: Option<String>,
) -> Result<String, String> {
    run_blocking(move || -> Result<String, String> {
        // Auto-stage all working changes if nothing's been staged yet.
        if git_ok(&repo, &["diff", "--cached", "--name-only"])?
            .trim()
            .is_empty()
        {
            git_ok(&repo, &["add", "-A"])?;
            if git_ok(&repo, &["diff", "--cached", "--name-only"])?
                .trim()
                .is_empty()
            {
                return Err("Nothing to commit — working tree is clean.".into());
            }
        }
        let (stat, diff) = staged_diff(&repo)?;
        let provider = GitAiProvider::parse(provider)?;
        let model = model.unwrap_or_else(|| provider.default_model().to_string());
        let message = describe_diff_with_local_ai(&repo, &stat, &diff, provider, &model, None)?;
        commit_with_message(&repo, &message)?;
        Ok(message)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::{git, init_repo};
    use std::fs;

    #[test]
    fn keeps_small_diffs_verbatim() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\n@@ -1 +1 @@\n-old\n+new ✨\n";
        let prepared = prepare_diff(diff, 1_000);

        assert!(!prepared.compacted);
        assert_eq!(prepared.text, diff);
    }

    #[test]
    fn compacted_diff_keeps_every_file_hunk_and_both_ends() {
        let large_body = |label: &str| {
            format!(
                "-{label}_BEGIN_✨{}\n+{}{label}_END_🚀\n",
                "old".repeat(220),
                "new".repeat(220)
            )
        };
        let diff = format!(
            "diff --git a/src/first.rs b/src/first.rs\n--- a/src/first.rs\n+++ b/src/first.rs\n@@ -1 +1 @@ FIRST_HUNK\n{}@@ -20 +20 @@ SECOND_HUNK\n{}diff --git a/src/last.rs b/src/last.rs\n--- a/src/last.rs\n+++ b/src/last.rs\n@@ -2 +2 @@ LAST_HUNK\n{}",
            large_body("FIRST"),
            large_body("SECOND"),
            large_body("LAST")
        );
        let budget = 1_000;
        let prepared = prepare_diff(&diff, budget);

        assert!(prepared.compacted);
        assert!(prepared.text.len() <= budget);
        for expected in [
            "diff --git a/src/first.rs b/src/first.rs",
            "FIRST_HUNK",
            "SECOND_HUNK",
            "diff --git a/src/last.rs b/src/last.rs",
            "LAST_HUNK",
            "FIRST_BEGIN_✨",
            "FIRST_END_🚀",
            "LAST_BEGIN_✨",
            "LAST_END_🚀",
        ] {
            assert!(
                prepared.text.contains(expected),
                "missing {expected}:\n{}",
                prepared.text
            );
        }
        assert!(prepared.text.contains(OMITTED_HUNK_MARKER.trim()));
    }

    #[test]
    fn compacted_diff_summarizes_lockfiles_but_keeps_their_structure() {
        let diff = format!(
            "diff --git a/package-lock.json b/package-lock.json\n--- a/package-lock.json\n+++ b/package-lock.json\n@@ -1 +1 @@ LOCK_HUNK\n-LOCK_PAYLOAD_START{}LOCK_PAYLOAD_END\n+LOCK_PAYLOAD_NEW{}\ndiff --git a/src/app.ts b/src/app.ts\n--- a/src/app.ts\n+++ b/src/app.ts\n@@ -1 +1 @@ APP_HUNK\n-old{}\n+new{}\n",
            "x".repeat(1_200),
            "y".repeat(1_200),
            "a".repeat(500),
            "b".repeat(500)
        );
        let prepared = prepare_diff(&diff, 900);

        assert!(prepared.compacted);
        assert!(prepared.text.len() <= 900);
        assert!(prepared.text.contains("package-lock.json"));
        assert!(prepared.text.contains("LOCK_HUNK"));
        assert!(prepared.text.contains(OMITTED_GENERATED_MARKER.trim()));
        assert!(!prepared.text.contains("LOCK_PAYLOAD_END"));
        assert!(prepared.text.contains("src/app.ts"));
        assert!(prepared.text.contains("APP_HUNK"));
    }

    #[test]
    fn compaction_is_utf8_safe_at_tiny_boundaries() {
        let diff = format!(
            "diff --git a/emoji.rs b/emoji.rs\n--- a/emoji.rs\n+++ b/emoji.rs\n@@ -1 +1 @@\n-{}\n+{}\n",
            "🦀".repeat(400),
            "✨".repeat(400)
        );
        let prepared = prepare_diff(&diff, 320);

        assert!(prepared.compacted);
        assert!(prepared.text.len() <= 320);
        assert!(prepared.text.contains("diff --git a/emoji.rs"));
        assert!(prepared.text.contains("@@ -1 +1 @@"));
    }

    #[test]
    fn fair_budget_does_not_starve_later_entries() {
        let allocations = fair_allocations(&[10_000, 20, 10_000], 300);

        assert_eq!(allocations.iter().sum::<usize>(), 300);
        assert_eq!(allocations[1], 20);
        assert!(allocations[0] > 0);
        assert!(allocations[2] > 0);
        assert!((allocations[0] as isize - allocations[2] as isize).abs() <= 1);
    }

    #[test]
    fn staged_ai_diff_excludes_unchanged_context() {
        let td = init_repo();
        fs::write(
            td.path().join("f.txt"),
            "keep-one\nkeep-two\nold-value\nkeep-three\nkeep-four\n",
        )
        .expect("write base");
        git(td.path(), &["add", "f.txt"]);
        git(td.path(), &["commit", "-m", "base"]);
        fs::write(
            td.path().join("f.txt"),
            "keep-one\nkeep-two\nnew-value\nkeep-three\nkeep-four\n",
        )
        .expect("write change");
        git(td.path(), &["add", "f.txt"]);

        let (_, diff) = staged_diff(td.path().to_str().expect("repo path")).expect("staged diff");

        assert!(diff.contains("-old-value"));
        assert!(diff.contains("+new-value"));
        assert!(!diff.contains(" keep-one"));
        assert!(!diff.contains(" keep-four"));
    }
}
