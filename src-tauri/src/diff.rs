// Line diffs through libgit2: the editor's git gutter asks for changed line
// ranges, and the diff panes ask for the rows of a unified diff.

use git2::{DiffOptions, Patch};
use serde::Serialize;
use tauri::async_runtime::spawn_blocking;

use crate::error::{AppError, AppResult};

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum DiffKind {
    Add,
    Mod,
    Del,
}

#[derive(Serialize)]
pub struct DiffHunk {
    pub kind: DiffKind,
    pub start: u32, // 0-based line in `current`
    pub end: u32,   // exclusive; for Del equals start
}

#[tauri::command]
pub async fn diff_hunks(baseline: String, current: String) -> AppResult<Vec<DiffHunk>> {
    // Every visible editor asks on each keystroke, and a multi-MB file takes
    // tens of ms, so this stays off the Tauri worker pool.
    spawn_blocking(move || diff_hunks_sync(&baseline, &current))
        .await
        .map_err(|e| AppError::Other(format!("diff join: {e}")))
}

/// A run of deleted lines followed by the lines that replaced them, both
/// 0-based. A pure insertion deletes nothing and a pure deletion adds nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Change {
    old_start: usize,
    old_len: usize,
    new_start: usize,
    new_len: usize,
}

fn changes(old: &str, new: &str) -> Vec<Change> {
    if old == new {
        return Vec::new();
    }
    let mut options = DiffOptions::new();
    options.context_lines(0).interhunk_lines(0).force_text(true);
    let Ok(patch) = Patch::from_buffers(
        old.as_bytes(),
        None,
        new.as_bytes(),
        None,
        Some(&mut options),
    ) else {
        return Vec::new();
    };
    (0..patch.num_hunks())
        .filter_map(|index| patch.hunk(index).ok())
        .map(|(hunk, _)| {
            let old_len = hunk.old_lines() as usize;
            let new_len = hunk.new_lines() as usize;
            // With no lines on a side, git names the line the change sits after.
            let start = |at: u32, len: usize| {
                if len == 0 {
                    at as usize
                } else {
                    at as usize - 1
                }
            };
            Change {
                old_start: start(hunk.old_start(), old_len),
                old_len,
                new_start: start(hunk.new_start(), new_len),
                new_len,
            }
        })
        .collect()
}

fn diff_hunks_sync(baseline: &str, current: &str) -> Vec<DiffHunk> {
    changes(baseline, current)
        .into_iter()
        .map(|change| {
            let start = change.new_start as u32;
            let end = start + change.new_len as u32;
            let kind = match (change.old_len, change.new_len) {
                (_, 0) => DiffKind::Del,
                (0, _) => DiffKind::Add,
                _ => DiffKind::Mod,
            };
            DiffHunk { kind, start, end }
        })
        .collect()
}

pub const ROW_CONTEXT: u8 = 0;
pub const ROW_ADDED: u8 = 1;
pub const ROW_DELETED: u8 = 2;
pub const ROW_HIDDEN: u8 = 3;

/// One row of a unified diff: its kind, the line number it shows, and its
/// text. A hidden row stands for unchanged lines left out, and its number is
/// how many.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct DiffRow(pub u8, pub u32, pub String);

/// Unchanged lines kept on each side of a change.
const CONTEXT_LINES: usize = 4;
/// A run of unchanged lines only this much longer than the context around it
/// is shown whole: hiding three lines behind a marker saves nothing.
const HIDE_AT_LEAST: usize = 4;

fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let body = text.strip_suffix('\n').unwrap_or(text);
    body.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

fn numbered_rows<'a>(
    kind: u8,
    lines: &'a [&str],
    from: usize,
    to: usize,
) -> impl Iterator<Item = DiffRow> + 'a {
    lines[from..to]
        .iter()
        .zip(from + 1..)
        .map(move |(text, number)| DiffRow(kind, number as u32, text.to_string()))
}

/// The rows of a unified diff from `old` to `new`, with long unchanged runs
/// between changes hidden unless `full` asks for every line.
pub fn unified_rows(old: &str, new: &str, full: bool) -> Vec<DiffRow> {
    let old_lines = split_lines(old);
    let new_lines = split_lines(new);
    let changes = changes(old, new);
    let mut rows = Vec::new();
    let context = |rows: &mut Vec<DiffRow>, from: usize, to: usize| {
        rows.extend(numbered_rows(ROW_CONTEXT, &new_lines, from, to));
    };

    let mut new_at = 0;
    for (index, change) in changes.iter().enumerate() {
        let gap = change.new_start - new_at;
        let keep_before = if index == 0 {
            CONTEXT_LINES
        } else {
            CONTEXT_LINES * 2
        };
        if full || gap < keep_before + HIDE_AT_LEAST {
            context(&mut rows, new_at, change.new_start);
        } else {
            if index > 0 {
                context(&mut rows, new_at, new_at + CONTEXT_LINES);
                rows.push(DiffRow(
                    ROW_HIDDEN,
                    (gap - CONTEXT_LINES * 2) as u32,
                    String::new(),
                ));
            }
            context(
                &mut rows,
                change.new_start - CONTEXT_LINES,
                change.new_start,
            );
        }
        let deleted = change.old_start..change.old_start + change.old_len;
        rows.extend(numbered_rows(
            ROW_DELETED,
            &old_lines,
            deleted.start,
            deleted.end,
        ));
        let added = change.new_start..change.new_start + change.new_len;
        rows.extend(numbered_rows(ROW_ADDED, &new_lines, added.start, added.end));
        new_at = change.new_start + change.new_len;
    }
    if !changes.is_empty() {
        let tail = new_lines.len() - new_at;
        let shown = if full || tail < CONTEXT_LINES + HIDE_AT_LEAST {
            tail
        } else {
            CONTEXT_LINES
        };
        context(&mut rows, new_at, new_at + shown);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(count: usize) -> String {
        (1..=count).map(|n| format!("line {n}\n")).collect()
    }

    fn kinds(rows: &[DiffRow]) -> String {
        rows.iter()
            .map(|row| match row.0 {
                ROW_CONTEXT => ' ',
                ROW_ADDED => '+',
                ROW_DELETED => '-',
                _ => '~',
            })
            .collect()
    }

    #[test]
    fn gutter_hunks_name_the_lines_in_the_current_text() {
        let hunks = diff_hunks_sync("a\nb\nc\nd\n", "a\nB\nc\nx\nd\n");
        let spans: Vec<_> = hunks
            .iter()
            .map(|h| (h.kind as u8, h.start, h.end))
            .collect();
        assert_eq!(
            spans,
            vec![(DiffKind::Mod as u8, 1, 2), (DiffKind::Add as u8, 3, 4)]
        );

        let deleted = diff_hunks_sync("a\nb\nc\n", "a\nc\n");
        assert!(matches!(deleted[0].kind, DiffKind::Del));
        assert_eq!((deleted[0].start, deleted[0].end), (1, 1));
        assert!(diff_hunks_sync("same\n", "same\n").is_empty());
    }

    #[test]
    fn a_change_keeps_four_lines_either_side_and_hides_the_rest() {
        let old = numbered(30);
        let new = old.replace("line 15\n", "line fifteen\n");
        let rows = unified_rows(&old, &new, false);
        assert_eq!(kinds(&rows), "    -+    ");
        assert_eq!(rows[0], DiffRow(ROW_CONTEXT, 11, "line 11".into()));
        assert_eq!(rows[4], DiffRow(ROW_DELETED, 15, "line 15".into()));
        assert_eq!(rows[5], DiffRow(ROW_ADDED, 15, "line fifteen".into()));
        assert_eq!(rows[9], DiffRow(ROW_CONTEXT, 19, "line 19".into()));
    }

    #[test]
    fn short_unchanged_runs_are_shown_whole() {
        let old = numbered(12);
        let new = old.replace("line 8\n", "line eight\n");
        assert_eq!(kinds(&unified_rows(&old, &new, false)), "       -+    ");
    }

    #[test]
    fn a_long_run_between_changes_becomes_one_hidden_row() {
        let old = numbered(40);
        let new = old
            .replace("line 5\n", "line five\n")
            .replace("line 30\n", "line thirty\n");
        let rows = unified_rows(&old, &new, false);
        assert_eq!(kinds(&rows), "    -+    ~    -+    ");
        assert_eq!(rows[10], DiffRow(ROW_HIDDEN, 16, String::new()));
        assert_eq!(unified_rows(&old, &new, true).len(), 42);
    }

    #[test]
    fn a_new_file_is_all_additions_and_identical_files_have_no_rows() {
        assert_eq!(kinds(&unified_rows("", "a\nb", false)), "++");
        assert!(unified_rows("same\n", "same\n", false).is_empty());
    }

    #[test]
    fn deletions_come_before_the_additions_that_replace_them() {
        let old = include_str!("testdata/diff_order_base.ts.txt");
        let new = include_str!("testdata/diff_order_head.ts.txt");
        let rows = unified_rows(old, new, false);

        for run in rows.split(|row| row.0 == ROW_CONTEXT || row.0 == ROW_HIDDEN) {
            let first_added = run
                .iter()
                .position(|row| row.0 == ROW_ADDED)
                .unwrap_or(run.len());
            assert!(
                run[first_added..].iter().all(|row| row.0 == ROW_ADDED),
                "{run:?}"
            );
        }
        let older = rows
            .iter()
            .position(|row| row.2.starts_with("older { AgentIcon"))
            .unwrap();
        assert_eq!(rows[older].0, ROW_DELETED);
        assert_eq!(rows[older].1, 11);
        assert_eq!((rows[older + 1].0, rows[older + 1].1), (ROW_ADDED, 12));
        assert!(rows[older + 1].2.starts_with("import { AgentIcon"));
        let comment = rows
            .iter()
            .position(|row| row.2.contains("Aolder open agent's row"))
            .unwrap();
        assert_eq!((rows[comment].0, rows[comment].1), (ROW_DELETED, 41));
        assert_eq!((rows[comment + 1].0, rows[comment + 1].1), (ROW_ADDED, 41));
    }
}
