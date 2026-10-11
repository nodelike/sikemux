//! Codex reports an edit as a diff's hunks without file headers. The chat
//! shows an edit as the file before and after, so these rebuild one side from
//! the file on disk.

struct Hunk {
    old_start: usize,
    new_start: usize,
    old: Vec<String>,
    new: Vec<String>,
}

fn range(text: &str) -> Option<(usize, usize)> {
    let mut parts = text.splitn(2, ',');
    let start = parts.next()?.parse().ok()?;
    let count = match parts.next() {
        Some(count) => count.parse().ok()?,
        None => 1,
    };
    Some((start, count))
}

fn parse(diff: &str) -> Option<Vec<Hunk>> {
    let mut hunks = Vec::new();
    let mut lines = diff.split('\n').peekable();
    while let Some(line) = lines.next() {
        let Some(header) = line.strip_prefix("@@ ") else {
            continue;
        };
        let mut fields = header.split_whitespace();
        let (old_start, mut old_left) = range(fields.next()?.strip_prefix('-')?)?;
        let (new_start, mut new_left) = range(fields.next()?.strip_prefix('+')?)?;
        let mut hunk = Hunk {
            old_start,
            new_start,
            old: Vec::new(),
            new: Vec::new(),
        };
        while old_left > 0 || new_left > 0 {
            let line = lines.next()?;
            let (mark, text) = line.split_at(line.chars().next().map_or(0, char::len_utf8));
            match mark {
                " " | "" => {
                    hunk.old.push(text.to_owned());
                    hunk.new.push(text.to_owned());
                    old_left = old_left.checked_sub(1)?;
                    new_left = new_left.checked_sub(1)?;
                }
                "-" => {
                    hunk.old.push(text.to_owned());
                    old_left = old_left.checked_sub(1)?;
                }
                "+" => {
                    hunk.new.push(text.to_owned());
                    new_left = new_left.checked_sub(1)?;
                }
                "\\" => {}
                _ => return None,
            }
        }
        while lines.peek().is_some_and(|line| line.starts_with('\\')) {
            lines.next();
        }
        hunks.push(hunk);
    }
    (!hunks.is_empty()).then_some(hunks)
}

fn apply_hunks(text: &str, hunks: &[Hunk], reversed: bool) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    let mut position = 0;
    for hunk in hunks {
        let (from, to, start) = if reversed {
            (&hunk.new, &hunk.old, hunk.new_start)
        } else {
            (&hunk.old, &hunk.new, hunk.old_start)
        };
        let expected = start.saturating_sub(1).max(position);
        let found = if from.is_empty() {
            Some(expected.min(lines.len()))
        } else {
            let fits = |at: usize| {
                at + from.len() <= lines.len()
                    && lines[at..at + from.len()]
                        .iter()
                        .zip(from)
                        .all(|(line, wanted)| line == wanted)
            };
            let reach = lines.len().max(expected);
            (0..=reach).find_map(|distance| {
                let after = expected + distance;
                if fits(after) {
                    return Some(after);
                }
                let before = expected.checked_sub(distance)?;
                (before >= position && fits(before)).then_some(before)
            })
        }?;
        out.extend_from_slice(&lines[position..found]);
        out.extend(to.iter().map(String::as_str));
        position = found + from.len();
    }
    out.extend_from_slice(&lines[position..]);
    Some(out.join("\n"))
}

fn clean(diff: &str) -> &str {
    diff.find("\n\nMoved to: ").map_or(diff, |end| &diff[..end])
}

/// `text` with the edit made.
pub(super) fn apply(text: &str, diff: &str) -> Option<String> {
    apply_hunks(text, &parse(clean(diff))?, false)
}

/// `text` as it was before the edit, when it already has it.
pub(super) fn revert(text: &str, diff: &str) -> Option<String> {
    apply_hunks(text, &parse(clean(diff))?, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEFORE: &str = "one\ntwo\nthree\nfour\nfive\n";
    const AFTER: &str = "one\ntwo\nTHREE\nfour\nfive\nsix\n";
    const DIFF: &str =
        "@@ -2,3 +2,3 @@\n two\n-three\n+THREE\n four\n@@ -5,1 +5,2 @@\n five\n+six\n";

    #[test]
    fn a_hunk_diff_applies_and_reverts() {
        assert_eq!(apply(BEFORE, DIFF).as_deref(), Some(AFTER));
        assert_eq!(revert(AFTER, DIFF).as_deref(), Some(BEFORE));
    }

    #[test]
    fn a_patched_file_does_not_take_the_patch_again() {
        assert_eq!(apply(AFTER, DIFF), None);
        assert_eq!(revert(BEFORE, DIFF), None);
    }

    #[test]
    fn hunks_are_found_where_the_file_moved_them() {
        let shifted = format!("zero\n{BEFORE}");
        assert_eq!(apply(&shifted, DIFF), Some(format!("zero\n{AFTER}")));
    }

    #[test]
    fn a_move_note_is_not_part_of_the_diff() {
        let moved = format!("{DIFF}\n\nMoved to: /tmp/elsewhere.txt");
        assert_eq!(apply(BEFORE, &moved).as_deref(), Some(AFTER));
    }

    #[test]
    fn text_that_is_not_a_diff_is_refused() {
        assert_eq!(apply(BEFORE, "just text"), None);
        assert_eq!(apply(BEFORE, "@@ -1,2 +1,2 @@\n one\n"), None);
    }
}
