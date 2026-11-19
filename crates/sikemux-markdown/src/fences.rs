//! Code fenced inside a list item whose lines start left of the item's text, as agents write
//! `4. ```` with the code under it at the margin. CommonMark ends the item at the first such
//! line, leaving an empty block, the code as prose and a stray fence; the author meant the code
//! to sit in the block, so its lines are indented under the item before parsing.

use crate::delimiters::Moved;

/// `text` with each such fence's lines indented to its item's text, and where each byte of the
/// result came from in `text`; none when it has no such fence.
pub(crate) fn indent_item_fences(text: &str) -> Option<(String, Moved)> {
    if !text.contains("```") && !text.contains("~~~") {
        return None;
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len());
    let mut moved = Moved::default();
    let mut item: Option<usize> = None;
    let mut at = 0;
    let mut index = 0;
    while let Some(&line) = lines.get(index) {
        out.push_str(line);
        at += line.len();
        index += 1;
        if line.trim().is_empty() {
            continue;
        }
        let indent = indent_of(line);
        let rest = line.trim_start_matches([' ', '\t']);
        let fence = match list_marker(rest) {
            Some(marker) => {
                let content = indent + marker;
                item = Some(content);
                fence_of(rest.get(marker..).unwrap_or_default())
            }
            None => {
                if item.is_some_and(|content| indent < content) {
                    item = None;
                }
                fence_of(rest).filter(|_| indent <= item.unwrap_or(0) + 3)
            }
        };
        let Some((mark, width)) = fence else {
            continue;
        };
        let shift = item.unwrap_or(0);
        let body = lines.get(index..).unwrap_or_default();
        let closer = body.iter().position(|line| closes(line, mark, width));
        let end = closer.map_or(body.len(), |closer| closer + 1);
        let fence_lines = body.get(..end).unwrap_or_default();
        let short = fence_lines
            .iter()
            .any(|line| !line.trim().is_empty() && indent_of(line) < shift);
        for line in fence_lines {
            if short && !line.trim().is_empty() {
                out.extend(std::iter::repeat_n(' ', shift));
                moved.shift(out.len(), at);
            }
            out.push_str(line);
            at += line.len();
        }
        index += end;
    }
    moved.any().then_some((out, moved))
}

/// The columns before a line's first character, a tab reaching the next multiple of four.
fn indent_of(line: &str) -> usize {
    let mut column = 0;
    for byte in line.bytes() {
        match byte {
            b' ' => column += 1,
            b'\t' => column += 4 - column % 4,
            _ => break,
        }
    }
    column
}

/// The width of the bullet or number starting `rest` with the spaces after it, which is where
/// the item's text starts.
fn list_marker(rest: &str) -> Option<usize> {
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let marker = match rest.as_bytes().get(digits)? {
        b'.' | b')' if (1..=9).contains(&digits) => digits + 1,
        b'-' | b'*' | b'+' if digits == 0 => 1,
        _ => return None,
    };
    let after = rest.get(marker..)?;
    let spaces = after.bytes().take_while(|byte| *byte == b' ').count();
    match spaces {
        0 if !after.trim().is_empty() => None,
        1..=4 => Some(marker + spaces),
        _ => Some(marker + 1),
    }
}

/// The fence character and its run's width when `rest` opens a fence.
fn fence_of(rest: &str) -> Option<(u8, usize)> {
    let mark = *rest
        .as_bytes()
        .first()
        .filter(|byte| matches!(byte, b'`' | b'~'))?;
    let width = rest.bytes().take_while(|byte| *byte == mark).count();
    let info = rest.get(width..).unwrap_or_default();
    (width >= 3 && !(mark == b'`' && info.contains('`'))).then_some((mark, width))
}

fn closes(line: &str, mark: u8, width: usize) -> bool {
    let rest = line.trim_start_matches([' ', '\t']);
    let run = rest.bytes().take_while(|byte| *byte == mark).count();
    run >= width && rest.get(run..).is_some_and(|after| after.trim().is_empty())
}
