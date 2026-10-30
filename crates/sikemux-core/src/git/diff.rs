// Line diffs through libgit2: the editor's git gutter asks for changed line
// ranges, and the diff panes ask for the rows of a unified diff.

use git2::{DiffOptions, Patch};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
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

/// A run of deleted lines followed by the lines that replaced them, both
/// 0-based. A pure insertion deletes nothing and a pure deletion adds nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub old_start: usize,
    pub old_len: usize,
    pub new_start: usize,
    pub new_len: usize,
}

impl Change {
    pub fn kind(&self) -> DiffKind {
        match (self.old_len, self.new_len) {
            (_, 0) => DiffKind::Del,
            (0, _) => DiffKind::Add,
            _ => DiffKind::Mod,
        }
    }
}

/// The runs of lines that differ between `old` and `new`, in order.
pub fn changes(old: &str, new: &str) -> Vec<Change> {
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

/// What changed in `current` since `baseline`. Every visible editor asks on
/// each keystroke, and a multi-MB file takes tens of ms, so callers run it off
/// their async workers.
pub fn hunks(baseline: &str, current: &str) -> Vec<DiffHunk> {
    changes(baseline, current)
        .into_iter()
        .map(|change| {
            let start = change.new_start as u32;
            DiffHunk {
                kind: change.kind(),
                start,
                end: start + change.new_len as u32,
            }
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
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct DiffRow(pub u8, pub u32, pub String);

/// A code host's unified patch for one file as rows: added and unchanged lines
/// numbered in the new file, removed ones in the old, and the unchanged lines
/// between hunks, which the patch leaves out, as a hidden row of their count.
pub fn patch_rows(patch: &str) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    let (mut old_line, mut new_line, mut shown_up_to) = (0u32, 0u32, 0u32);
    for line in patch.lines() {
        if let Some((old, new)) = hunk_starts(line) {
            (old_line, new_line) = (old, new);
            let skipped = new_line.saturating_sub(1).saturating_sub(shown_up_to);
            if skipped > 0 {
                rows.push(DiffRow(ROW_HIDDEN, skipped, String::new()));
            }
            continue;
        }
        if old_line == 0 && new_line == 0 {
            continue;
        }
        let text = line.get(1..).unwrap_or_default().to_owned();
        match line.as_bytes().first() {
            Some(b'+') => {
                rows.push(DiffRow(ROW_ADDED, new_line, text));
                shown_up_to = new_line;
                new_line += 1;
            }
            Some(b'-') => {
                rows.push(DiffRow(ROW_DELETED, old_line, text));
                old_line += 1;
            }
            Some(b'\\') => {}
            _ => {
                rows.push(DiffRow(ROW_CONTEXT, new_line, text));
                shown_up_to = new_line;
                new_line += 1;
                old_line += 1;
            }
        }
    }
    rows
}

/// Where a hunk starts in the old and new file, from its `@@ -a,b +c,d @@` header.
fn hunk_starts(line: &str) -> Option<(u32, u32)> {
    let ranges = line.strip_prefix("@@ -")?.split(" @@").next()?;
    let (old, new) = ranges.split_once(" +")?;
    let start = |range: &str| range.split(',').next()?.parse().ok();
    Some((start(old)?, start(new)?))
}

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
        let spans: Vec<_> = hunks("a\nb\nc\nd\n", "a\nB\nc\nx\nd\n")
            .iter()
            .map(|h| (h.kind as u8, h.start, h.end))
            .collect();
        assert_eq!(
            spans,
            vec![(DiffKind::Mod as u8, 1, 2), (DiffKind::Add as u8, 3, 4)]
        );

        let deleted = hunks("a\nb\nc\n", "a\nc\n");
        assert!(matches!(deleted[0].kind, DiffKind::Del));
        assert_eq!((deleted[0].start, deleted[0].end), (1, 1));
        assert!(hunks("same\n", "same\n").is_empty());
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

    fn rows(rows: &[(u8, u32, &str)]) -> Vec<DiffRow> {
        rows.iter()
            .map(|&(kind, number, text)| DiffRow(kind, number, text.to_owned()))
            .collect()
    }

    #[test]
    fn a_patch_numbers_added_and_unchanged_lines_in_the_new_file_and_removed_ones_in_the_old() {
        let patch = "@@ -3,3 +3,3 @@\n keep\n-old\n+new\n tail";
        assert_eq!(
            patch_rows(patch),
            rows(&[
                (3, 2, ""),
                (0, 3, "keep"),
                (2, 4, "old"),
                (1, 4, "new"),
                (0, 5, "tail")
            ])
        );
    }

    #[test]
    fn a_patch_counts_the_unchanged_lines_between_hunks_which_it_leaves_out() {
        let patch = "@@ -1,2 +1,2 @@\n a\n-b\n+B\n@@ -10,1 +10,2 @@\n j\n+k";
        let found = patch_rows(patch);
        assert_eq!(found[3], DiffRow(ROW_HIDDEN, 7, String::new()));
        assert_eq!(found[4..], rows(&[(0, 10, "j"), (1, 11, "k")]));
    }

    #[test]
    fn a_patch_of_a_new_file_hides_nothing_before_it() {
        assert_eq!(
            patch_rows("@@ -0,0 +1,2 @@\n+one\n+two\n"),
            rows(&[(1, 1, "one"), (1, 2, "two")])
        );
    }

    #[test]
    fn a_patch_skips_the_note_about_a_missing_newline_at_the_end() {
        assert_eq!(
            patch_rows("@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+b"),
            rows(&[(2, 1, "a"), (1, 1, "b")])
        );
    }
}
